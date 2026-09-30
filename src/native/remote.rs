//! [`Backend`] on a quipu server, over HTTP (mode 2: a shared, remote store).
//!
//! Reads are SPARQL on `POST /query` (W3C results JSON, paged so the server's
//! row ceiling can never silently truncate a snapshot). Writes are ONE
//! SPARQL 1.1 Update on `POST /update` per batch, shaped as a compare-and-set:
//!
//! ```text
//! DELETE { GRAPH <g> { <seed> ?p ?o ... } }     every fact of every seed written
//! INSERT { GRAPH <g> { ...the new facts... } }
//! WHERE  { GRAPH <g> { <seed> seeds:revision N ... }     the revisions the writer read
//!          FILTER NOT EXISTS { GRAPH <g> { <new> ?x ?y } }   what must not exist yet
//!          { <seed> ?p ?o } UNION ... UNION { } }
//! ```
//!
//! quipu runs an update under its store lock, so the `WHERE` clause is an
//! atomic precondition: if any seed moved, nothing matches and nothing is
//! written. quipu's `/update` returns no affected count, so seeds reads the
//! written seeds back and compares them with what it sent; a mismatch is a
//! conflict (exit 4), never a success.
//!
//! Two things measured while building this, both of which matter to anyone
//! running it:
//!
//! - quipu's `/update` evaluates `WHERE` only over REGISTERED named graphs. A
//!   precondition on an unregistered graph matches nothing, so a
//!   `FILTER NOT EXISTS` guard passes vacuously and a second create writes a
//!   duplicate. seeds therefore registers the project graph
//!   (`POST /graph/create`) before its first write.
//! - `/update` copies the whole store into an in-memory graph on every call,
//!   so its cost grows with the server's store. A seeds project on a dedicated
//!   quipu server is fine; pointing seeds at a large shared knowledge graph
//!   makes every write as expensive as that graph is big.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value as Json};

use crate::backend::{Backend, Ctx, WriteBatch};
use crate::error::{ErrorKind, Result, SdError};
use crate::model::{Fact, Obj, Snapshot};
use crate::validate::{self, escape_literal};
use crate::vocab::{self, term};

const PAGE: usize = 2000;

/// A project stored as a named graph on a quipu server.
pub struct RemoteBackend {
    base: String,
    graph: String,
    token: Option<String>,
    /// Signs writes on the paths quipu accepts signed (aegis-bys8d1); those
    /// then carry no bearer.
    signer: Option<super::attest::Signer>,
    agent: ureq::Agent,
    graph_registered: bool,
}

fn transport(url: &str, e: &ureq::Transport) -> SdError {
    SdError::new(
        ErrorKind::Unreachable,
        format!(
            "cannot reach quipu at {url}: {e}. seeds does not fall back to a local store, \
             because that would fork the ledger."
        ),
    )
}

fn status_error(what: &str, code: u16, body: &str) -> SdError {
    if let Some(verdict) = attestation_verdict(code, body) {
        let hint = match verdict.as_str() {
            "skew" => {
                " This machine's clock is more than 5 minutes off the server's; fix the clock."
            }
            "unbound" => {
                " The server has no binding for this key and session: run `sd key show` and have \
                 your introducer run the `quipu attest register` line it prints."
            }
            "revoked" | "expired" => {
                " This key's binding is no longer valid: `sd key init` a new key and register it."
            }
            "replay" => " The server had already seen this request's nonce; nothing was written.",
            "scope" => {
                " The key is registered for trusting shares only, not for writing: have your \
                 introducer run `quipu attest allow-write <session>` on the quipu host."
            }
            "badsig" => {
                " The signature did not verify: the key file does not match the registered key."
            }
            _ => "",
        };
        return SdError::failed(format!(
            "quipu {what}: signed write refused ({verdict}).{hint}"
        ));
    }
    let hint = if code == 401 || code == 403 {
        " The server wants a bearer token for writes: set SEEDS_QUIPU_TOKEN, or \
         [quipu] token_file in the config."
    } else {
        ""
    };
    let body: String = body.chars().take(500).collect();
    SdError::failed(format!("quipu {what} failed (HTTP {code}): {body}{hint}"))
}

/// The verdict of a refused signed write: quipu answers 401 with
/// `{"reason":"attestation_refused","verdict":...}`.
fn attestation_verdict(code: u16, body: &str) -> Option<String> {
    if code != 401 {
        return None;
    }
    let v: Json = serde_json::from_str(body).ok()?;
    (v["reason"] == "attestation_refused").then(|| v["verdict"].as_str().unwrap_or("?").to_string())
}

impl RemoteBackend {
    /// Connect to the quipu server at `base` (checked with `GET /health`).
    /// `plain_http_ok` is the user's `allow_plain_http_hosts`: hosts a token
    /// may reach over plain http.
    pub fn connect(
        base: &str,
        graph: &str,
        token: Option<String>,
        signer: Option<super::attest::Signer>,
        plain_http_ok: &[String],
    ) -> Result<Self> {
        if token.is_some() && !secure_enough(base) && !plain_http_allowed(base, plain_http_ok) {
            return Err(SdError::new(
                ErrorKind::Config,
                format!(
                    "refusing to send a bearer token to {base} over plain http; use https (or a \
                     localhost server), or, if you trust the network path to it, add its host to \
                     [quipu] allow_plain_http_hosts in ~/.config/seeds/config.toml"
                ),
            ));
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build();
        let b = Self {
            base: base.trim_end_matches('/').to_string(),
            graph: graph.to_string(),
            token,
            signer,
            agent,
            graph_registered: false,
        };
        match b.agent.get(&format!("{}/health", b.base)).call() {
            Ok(_) => Ok(b),
            Err(ureq::Error::Transport(t)) => Err(transport(&b.base, &t)),
            Err(ureq::Error::Status(code, r)) => Err(status_error(
                "health check",
                code,
                &r.into_string().unwrap_or_default(),
            )),
        }
    }

    /// The server URL.
    pub fn url(&self) -> &str {
        &self.base
    }

    fn post(&self, path: &str, content_type: &str, body: &str, auth: bool) -> Result<String> {
        self.send(path, content_type, body, auth)
            .map_err(|(_, e)| e)
    }

    /// POST, and say whether a failure leaves the outcome UNKNOWN: a
    /// transport error (the request may have reached the server) or a 5xx
    /// (the server or a proxy failed, possibly after the write). A 4xx is a
    /// definite refusal.
    fn send(
        &self,
        path: &str,
        content_type: &str,
        body: &str,
        auth: bool,
    ) -> std::result::Result<String, (bool, SdError)> {
        let url = format!("{}{path}", self.base);
        let mut req = self
            .agent
            .post(&url)
            .set("Content-Type", content_type)
            .set("Accept", "application/sparql-results+json")
            .set("X-Quipu-Client", "seeds");
        if auth {
            match &self.signer {
                // A signed write carries the attestation and never the bearer.
                Some(s) if super::attest::SIGNED_PATHS.contains(&path) => {
                    let header = s
                        .header_now("POST", path, content_type, body.as_bytes())
                        .map_err(|e| (false, e))?;
                    req = req.set(super::attest::HEADER, &header);
                }
                _ => {
                    if let Some(t) = &self.token {
                        req = req.set("Authorization", &format!("Bearer {t}"));
                    }
                }
            }
        }
        match req.send_string(body) {
            Ok(r) => r.into_string().map_err(|e| {
                (
                    true,
                    SdError::failed(format!("quipu {path}: reading the response: {e}")),
                )
            }),
            Err(ureq::Error::Transport(t)) => Err((true, transport(&self.base, &t))),
            Err(ureq::Error::Status(code, r)) => Err((
                code >= 500,
                status_error(path, code, &r.into_string().unwrap_or_default()),
            )),
        }
    }

    /// Run a SELECT and return its bindings.
    fn select(&self, sparql: &str, at: Option<u64>) -> Result<Vec<BTreeMap<String, Obj>>> {
        let mut body = json!({ "query": sparql });
        if let Some(t) = at {
            body["tx"] = json!(t);
        }
        let text = self.post("/query", "application/json", &body.to_string(), false)?;
        let v: Json = serde_json::from_str(&text).map_err(|e| {
            SdError::failed(format!(
                "quipu /query returned something that is not JSON: {e}"
            ))
        })?;
        let Some(rows) = v["results"]["bindings"].as_array() else {
            return Err(SdError::failed(format!(
                "quipu /query did not return SPARQL results JSON: {}",
                text.chars().take(300).collect::<String>()
            )));
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut m = BTreeMap::new();
            if let Some(obj) = row.as_object() {
                for (k, t) in obj {
                    if let Some(o) = term(t) {
                        m.insert(k.clone(), o);
                    }
                }
            }
            out.push(m);
        }
        Ok(out)
    }

    fn count(&self, at: Option<u64>) -> Result<usize> {
        let rows = self.select(
            &format!(
                "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{}> {{ ?s ?p ?o }} }}",
                self.graph
            ),
            at,
        )?;
        match rows.first().and_then(|r| r.get("n")) {
            Some(Obj::Int(n)) => Ok(usize::try_from(*n).unwrap_or(0)),
            Some(Obj::Str(s)) => s
                .parse()
                .map_err(|_| SdError::failed("quipu returned a non-numeric count")),
            _ => Ok(0),
        }
    }

    fn facts(&self, at: Option<u64>) -> Result<BTreeMap<String, Vec<Fact>>> {
        for _attempt in 0..3 {
            let expected = self.count(at)?;
            let mut by_subject: BTreeMap<String, Vec<Fact>> = BTreeMap::new();
            let mut total = 0;
            let mut offset = 0;
            loop {
                let rows = self.select(
                    &format!(
                        "SELECT ?s ?p ?o WHERE {{ GRAPH <{}> {{ ?s ?p ?o }} }} \
                         ORDER BY ?s ?p ?o LIMIT {PAGE} OFFSET {offset}",
                        self.graph
                    ),
                    at,
                )?;
                let n = rows.len();
                for mut r in rows {
                    if let (Some(Obj::Iri(s)), Some(Obj::Iri(p)), Some(o)) =
                        (r.remove("s"), r.remove("p"), r.remove("o"))
                    {
                        by_subject.entry(s).or_default().push((p, o));
                        total += 1;
                    }
                }
                if n < PAGE {
                    break;
                }
                offset += PAGE;
            }
            if total == expected {
                return Ok(by_subject);
            }
            // The graph changed between the count and the pages (or the server
            // capped a page): read it again rather than return a short ledger.
        }
        Err(SdError::failed(
            "could not read a consistent snapshot of the remote graph (it kept changing, \
             or the server truncates results below the page size)",
        ))
    }

    fn register_graph(&mut self) -> Result<()> {
        if self.graph_registered {
            return Ok(());
        }
        // Create only what is missing. /graphs is an open read, so a client that
        // signs its writes (no bearer) can still write to graphs that exist;
        // /graph/create needs whatever write credential the server accepts.
        let existing = self.graph_iris();
        for g in [self.graph.clone(), provenance_graph(&self.graph)] {
            if existing.as_ref().is_some_and(|e| e.contains(&g)) {
                continue;
            }
            let body = json!({ "graph": g }).to_string();
            match self.post("/graph/create", "application/json", &body, true) {
                Ok(_) => {}
                // A server older than signed /graph/create refuses the signed
                // request. Creating a graph is idempotent, so retry once with
                // the bearer when one is configured.
                Err(_) if self.signer.is_some() && self.token.is_some() => {
                    self.send_bearer("/graph/create", "application/json", &body)?;
                }
                Err(e) => return Err(e),
            }
        }
        self.graph_registered = true;
        Ok(())
    }

    /// POST with the bearer even when a signer is configured.
    fn send_bearer(&self, path: &str, content_type: &str, body: &str) -> Result<String> {
        let mut req = self
            .agent
            .post(&format!("{}{path}", self.base))
            .set("Content-Type", content_type)
            .set("X-Quipu-Client", "seeds");
        if let Some(t) = &self.token {
            req = req.set("Authorization", &format!("Bearer {t}"));
        }
        match req.send_string(body) {
            Ok(r) => r
                .into_string()
                .map_err(|e| SdError::failed(format!("quipu {path}: reading the response: {e}"))),
            Err(ureq::Error::Transport(t)) => Err(transport(&self.base, &t)),
            Err(ureq::Error::Status(code, r)) => Err(status_error(
                path,
                code,
                &r.into_string().unwrap_or_default(),
            )),
        }
    }

    /// The server's registered graph IRIs, or `None` if it would not say (an
    /// older server, or any failure): then every graph is created, as before.
    fn graph_iris(&self) -> Option<std::collections::BTreeSet<String>> {
        let text = self
            .agent
            .get(&format!("{}/graphs", self.base))
            .set("X-Quipu-Client", "seeds")
            .call()
            .ok()?
            .into_string()
            .ok()?;
        let v: Json = serde_json::from_str(&text).ok()?;
        Some(
            v["graphs"]
                .as_array()?
                .iter()
                .filter_map(|g| g["iri"].as_str().map(str::to_string))
                .collect(),
        )
    }
}

/// The host of an `http://` URL, lowercased: `(host:port, host)`. `None` for
/// any other scheme.
fn plain_http_host(url: &str) -> Option<(String, String)> {
    let u = url.trim().to_ascii_lowercase();
    let rest = u.strip_prefix("http://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .to_string();
    let bare = if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    };
    Some((authority, bare))
}

/// Whether the user allowed a token to reach `url` over plain http: its host
/// (or host:port) is listed EXACTLY in `allowed`. Never a suffix match, so
/// `quipu.internal.example` does not admit `quipu.internal.example.evil.example.org`.
pub fn plain_http_allowed(url: &str, allowed: &[String]) -> bool {
    let Some((authority, bare)) = plain_http_host(url) else {
        return false;
    };
    !bare.is_empty()
        && allowed
            .iter()
            .map(|a| a.trim())
            .any(|a| a.eq_ignore_ascii_case(&authority) || a.eq_ignore_ascii_case(&bare))
}

/// Whether a bearer may be sent to `url`: https anywhere, http only to this
/// machine.
pub fn secure_enough(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    if u.starts_with("https://") {
        return true;
    }
    let Some(rest) = u.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

/// The named graph a project's write provenance goes to (never exported).
pub fn provenance_graph(graph: &str) -> String {
    if graph.contains('#') {
        format!("{graph}-seeds-provenance")
    } else {
        format!("{graph}#seeds-provenance")
    }
}

fn term(t: &Json) -> Option<Obj> {
    let value = t["value"].as_str()?.to_string();
    match t["type"].as_str()? {
        "uri" => Some(Obj::Iri(value)),
        "literal" | "typed-literal" => {
            let dt = t["datatype"].as_str().unwrap_or("");
            if dt == "http://www.w3.org/2001/XMLSchema#integer" {
                value.parse().ok().map(Obj::Int)
            } else {
                Some(Obj::Str(value))
            }
        }
        _ => None,
    }
}

fn sparql_obj(o: &Obj) -> String {
    match o {
        Obj::Iri(i) => format!("<{i}>"),
        Obj::Str(s) => format!("\"{}\"", escape_literal(s)),
        Obj::Int(n) => format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
    }
}

impl Backend for RemoteBackend {
    fn snapshot(&self, at: Option<u64>) -> Result<Snapshot> {
        let mut snap = Snapshot::from_subjects(&self.facts(at)?);
        snap.tx = at.unwrap_or(0);
        Ok(snap)
    }

    fn ready_ids(&self, at: Option<u64>) -> Result<Vec<String>> {
        let rows = self.select(&vocab::ready_query(&self.graph), at)?;
        let mut ids: Vec<String> = rows
            .into_iter()
            .filter_map(|mut r| match r.remove("id") {
                Some(Obj::Str(s)) => Some(s),
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> Result<u64> {
        if batch.is_empty() {
            return Ok(0);
        }
        self.register_graph()?;
        // Validate first, against the server's current state.
        let current = self.facts(None)?;
        validate::validate_batch(batch, &mut |id| {
            Ok(current.get(&vocab::item_iri(id)).cloned())
        })?;
        // Check the revisions locally too, so a stale write gets a precise
        // message; the update's WHERE clause is what makes it atomic.
        let snap = Snapshot::from_subjects(&current);
        for w in &batch.seeds {
            let cur = snap.seeds.get(&w.seed.id);
            crate::quipu_backend::check_revision(
                &w.seed.id,
                w.expected_revision,
                current.contains_key(&vocab::item_iri(&w.seed.id)),
                cur.map(|s| s.revision),
            )?;
        }

        let g = &self.graph;
        let rev = term::revision();
        let mut delete = String::new();
        let mut insert = String::new();
        let mut guards = String::new();
        let mut absent = String::new();
        let mut unions: Vec<String> = Vec::new();
        let mut var = 0usize;
        let mut replace = |iri: &str, delete: &mut String, unions: &mut Vec<String>| {
            delete.push_str(&format!("<{iri}> ?p{var} ?o{var} . "));
            unions.push(format!("{{ GRAPH <{g}> {{ <{iri}> ?p{var} ?o{var} }} }}"));
            var += 1;
        };
        for w in &batch.seeds {
            let iri = vocab::item_iri(&w.seed.id);
            match w.expected_revision {
                Some(r) => {
                    guards.push_str(&format!("<{iri}> <{rev}> {r} . "));
                    replace(&iri, &mut delete, &mut unions);
                }
                None => absent.push_str(&format!(
                    "FILTER NOT EXISTS {{ GRAPH <{g}> {{ <{iri}> ?x ?y }} }} "
                )),
            }
            for (p, o) in w.seed.facts() {
                insert.push_str(&format!("<{iri}> <{p}> {} . ", sparql_obj(&o)));
            }
        }
        for (id, r) in &batch.delete_seeds {
            let iri = vocab::item_iri(id);
            guards.push_str(&format!("<{iri}> <{rev}> {r} . "));
            replace(&iri, &mut delete, &mut unions);
        }
        for (seed, index) in &batch.delete_comments {
            replace(&vocab::comment_iri(seed, *index), &mut delete, &mut unions);
        }
        for c in &batch.comments {
            let iri = vocab::comment_iri(&c.seed, c.index);
            if !batch
                .delete_comments
                .iter()
                .any(|(s, i)| *s == c.seed && *i == c.index)
            {
                absent.push_str(&format!(
                    "FILTER NOT EXISTS {{ GRAPH <{g}> {{ <{iri}> ?x ?y }} }} "
                ));
            }
            for (p, o) in c.facts() {
                insert.push_str(&format!("<{iri}> <{p}> {} . ", sparql_obj(&o)));
            }
        }
        unions.push("{ }".to_string());
        let mut update = String::new();
        if !delete.is_empty() {
            update.push_str(&format!("DELETE {{ GRAPH <{g}> {{ {delete}}} }}\n"));
        }
        // Provenance: quipu's /update records every write as the same
        // anonymous "sparql-update", so seeds says who and why itself, in a
        // side graph that exports and snapshots never read.
        let written: Vec<String> = batch
            .seeds
            .iter()
            .map(|w| w.seed.id.clone())
            .chain(batch.delete_seeds.iter().map(|(id, _)| id.clone()))
            .chain(batch.comments.iter().map(|c| c.seed.clone()))
            .collect();
        let write_iri = format!(
            "urn:seeds:write:{}",
            &crate::pendant::sha256(
                format!("{}\n{}\n{update}{insert}{delete}", ctx.now, ctx.actor).as_bytes()
            )[7..31]
        );
        let mut prov = format!(
            "<{write_iri}> <{}> <{}> ; <{}> \"{}\" ; <{}> \"{}\" ; <{}> \"{}\" ",
            vocab::RDF_TYPE,
            vocab::seeds("Write"),
            vocab::seeds("actor"),
            escape_literal(&ctx.actor),
            vocab::seeds("source"),
            escape_literal(&batch.source),
            vocab::seeds("at"),
            escape_literal(&ctx.now),
        );
        for id in &written {
            prov.push_str(&format!(
                "; <{}> <{}> ",
                vocab::seeds("wrote"),
                vocab::item_iri(id)
            ));
        }
        prov.push('.');
        update.push_str(&format!(
            "INSERT {{ GRAPH <{g}> {{ {insert}}} GRAPH <{}> {{ {prov} }} }}\n",
            provenance_graph(g)
        ));
        let guard_block = if guards.is_empty() {
            String::new()
        } else {
            format!("GRAPH <{g}> {{ {guards}}} ")
        };
        update.push_str(&format!(
            "WHERE {{ {guard_block}{absent}{} }}",
            unions.join(" UNION ")
        ));
        if let Err((ambiguous, e)) =
            self.send("/update", "application/sparql-update", &update, true)
        {
            if !ambiguous {
                return Err(e);
            }
            // The request may have landed. Look before saying anything else,
            // and never invite a blind retry: a retried create mints a
            // second seed.
            return match self.unconfirmed(batch) {
                Ok(missing) if missing.is_empty() => {
                    eprintln!(
                        "sd: the server's response was lost ({}); a read-back shows the write \
                         landed",
                        e.message
                    );
                    Ok(0)
                }
                Ok(missing) => Err(SdError::new(
                    ErrorKind::Indeterminate,
                    format!(
                        "the write's outcome is UNKNOWN: the server's response was lost ({}) and \
                         a read-back does not yet show it for {}. It may still land. Check with \
                         `sd show` / `sd comments list` on those ids before doing anything; do \
                         not simply retry.",
                        e.message,
                        missing.join(", ")
                    ),
                )),
                Err(read) => Err(SdError::new(
                    ErrorKind::Indeterminate,
                    format!(
                        "the write's outcome is UNKNOWN: the server's response was lost ({}) and \
                         the read-back failed too ({}). Check {} before doing anything; do not \
                         simply retry.",
                        e.message,
                        read.message,
                        written.join(", ")
                    ),
                )),
            };
        }

        let missing = self.unconfirmed(batch)?;
        if !missing.is_empty() {
            return Err(SdError::conflict(format!(
                "the remote store does not show this write for {} (another writer changed it \
                 first, or changed it again right after). Re-read and retry; nothing is assumed \
                 to have landed.",
                missing.join(", ")
            )));
        }
        // quipu's /update returns no transaction id, so there is none to
        // report; callers see that a write happened through `Report::wrote`.
        Ok(0)
    }
}

impl RemoteBackend {
    /// What of `batch` the server does NOT show (empty when all of it landed).
    /// /update reports no affected count, so this read-back is the only proof
    /// that the precondition held.
    fn unconfirmed(&self, batch: &WriteBatch) -> Result<Vec<String>> {
        let after = Snapshot::from_subjects(&self.facts(None)?);
        let mut missing = Vec::new();
        for w in &batch.seeds {
            if after.seeds.get(&w.seed.id) != Some(&w.seed) {
                missing.push(w.seed.id.clone());
            }
        }
        for (id, _) in &batch.delete_seeds {
            if after.seeds.contains_key(id) {
                missing.push(id.clone());
            }
        }
        for c in &batch.comments {
            if !after.comments.contains(c) {
                missing.push(format!("{} comment {}", c.seed, c.index));
            }
        }
        Ok(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_never_sent_over_plain_http_to_another_host() {
        assert!(secure_enough("https://quipu.example.org"));
        assert!(secure_enough("http://localhost:8080"));
        assert!(secure_enough("http://127.0.0.1:9/x"));
        assert!(secure_enough("http://[::1]:9"));
        assert!(!secure_enough("http://quipu.example.org"));
        assert!(!secure_enough("http://localhost.evil.example.org"));
        assert!(!secure_enough("http://user@quipu.example.org"));
        let e = RemoteBackend::connect(
            "http://quipu.example.org",
            "urn:g",
            Some("t".into()),
            None,
            &[],
        )
        .err()
        .unwrap();
        assert_eq!(e.kind, ErrorKind::Config);
        assert!(
            e.message.contains("allow_plain_http_hosts"),
            "{}",
            e.message
        );
    }

    #[test]
    fn plain_http_is_allowed_only_to_exactly_listed_hosts() {
        let ok = vec![
            "quipu.internal.example".to_string(),
            "lan.example.org:8080".to_string(),
        ];
        assert!(plain_http_allowed("http://quipu.internal.example", &ok));
        assert!(plain_http_allowed("http://QUIPU.internal.example/x", &ok));
        assert!(plain_http_allowed("http://quipu.internal.example:80/", &ok));
        assert!(plain_http_allowed("http://lan.example.org:8080/q", &ok));
        // host:port entries admit only that port
        assert!(!plain_http_allowed("http://lan.example.org:9090", &ok));
        assert!(!plain_http_allowed("http://lan.example.org", &ok));
        // never a suffix or prefix match
        assert!(!plain_http_allowed(
            "http://quipu.internal.example.evil.example.org",
            &ok
        ));
        assert!(!plain_http_allowed(
            "http://evilquipu.internal.example",
            &ok
        ));
        // userinfo cannot smuggle a listed host
        assert!(!plain_http_allowed(
            "http://quipu.internal.example@evil.example.org",
            &ok
        ));
        assert!(!plain_http_allowed("http://other.example.org", &ok));
        assert!(!plain_http_allowed("http://quipu.internal.example", &[]));
        // not an http URL: this allowance is not what decides
        assert!(!plain_http_allowed("ftp://quipu.internal.example", &ok));
    }

    #[test]
    fn a_listed_host_passes_the_token_check_and_an_unlisted_one_is_refused() {
        // Port 9 on the loopback-free TEST-NET: the token check runs first, so
        // a refusal is Config and a pass reaches the health check (Unreachable).
        let ok = vec!["192.0.2.1:9".to_string()];
        let e = RemoteBackend::connect("http://192.0.2.1:9", "urn:g", Some("t".into()), None, &ok)
            .err()
            .unwrap();
        assert_ne!(e.kind, ErrorKind::Config, "{}", e.message);
        let e = RemoteBackend::connect("http://192.0.2.2:9", "urn:g", Some("t".into()), None, &ok)
            .err()
            .unwrap();
        assert_eq!(e.kind, ErrorKind::Config);
    }
}

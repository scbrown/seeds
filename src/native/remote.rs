//! [`Backend`] on a quipu server, over HTTP (mode 2: a shared, remote store).
//!
//! Reads are SPARQL on `POST /query` (W3C results JSON, paged so the server's
//! row ceiling can never silently truncate a snapshot). Writes are ONE
//! SPARQL 1.1 Update on `POST /update` per batch, shaped as a compare-and-set:
//!
//! ```text
//! DELETE { GRAPH <g> { <seed> ?p ?o ... } }     every fact of every seed written
//! INSERT { GRAPH <g> { ...the new facts... } }
//! WHERE  { GRAPH <g> { <seed> schema:version N ... }     the versions the writer read
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
    ephemeral_registered: bool,
    /// Refuse, unsent, a write request larger than this many bytes.
    max_write_bytes: usize,
    /// Refuse, unsent, a write nesting more guard clauses than this.
    max_write_clauses: usize,
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
            ephemeral_registered: false,
            max_write_bytes: super::config::DEFAULT_MAX_WRITE_BYTES,
            max_write_clauses: super::config::DEFAULT_MAX_WRITE_CLAUSES,
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

    /// Cap write requests at `bytes` (see [`Backend::max_write_bytes`]).
    #[must_use]
    pub fn with_max_write_bytes(mut self, bytes: usize) -> Self {
        self.max_write_bytes = bytes;
        self
    }

    /// Cap the guard clauses one write may nest (see
    /// [`Backend::max_write_clauses`]).
    #[must_use]
    pub fn with_max_write_clauses(mut self, clauses: usize) -> Self {
        self.max_write_clauses = clauses;
        self
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
            // The server refused the body for its size before evaluating
            // it: definite, and the caller may split the write.
            Err(ureq::Error::Status(413, _)) => Err((
                false,
                SdError::refused(format!(
                    "{}: quipu {path} answered HTTP 413 to a {}-byte request; nothing was \
                     written. Lower SEEDS_MAX_WRITE_BYTES below the server's limit.",
                    crate::backend::TOO_LARGE,
                    body.len()
                )),
            )),
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

    fn count(&self, graph: &str, at: Option<u64>) -> Result<usize> {
        let rows = self.select(
            &format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"),
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

    /// The project graph's facts, grouped by subject.
    fn facts(&self, at: Option<u64>) -> Result<BTreeMap<String, Vec<Fact>>> {
        self.graph_facts(&self.graph, at)
    }

    /// The project's ephemeral graph's facts, grouped by subject.
    fn ephemeral_facts(&self, at: Option<u64>) -> Result<BTreeMap<String, Vec<Fact>>> {
        self.graph_facts(&crate::vocab::ephemeral_graph(&self.graph), at)
    }

    /// One graph's facts, grouped by subject, read consistently in pages.
    fn graph_facts(&self, graph: &str, at: Option<u64>) -> Result<BTreeMap<String, Vec<Fact>>> {
        for _attempt in 0..3 {
            let expected = self.count(graph, at)?;
            let mut by_subject: BTreeMap<String, Vec<Fact>> = BTreeMap::new();
            if expected == 0 {
                return Ok(by_subject);
            }
            let mut total = 0;
            let mut offset = 0;
            loop {
                let rows = self.select(
                    &format!(
                        "SELECT ?s ?p ?o WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }} \
                         ORDER BY ?s ?p ?o LIMIT {PAGE} OFFSET {offset}"
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

    fn register_graph(&mut self, ephemeral: bool) -> Result<()> {
        if self.graph_registered && (!ephemeral || self.ephemeral_registered) {
            return Ok(());
        }
        // Create only what is missing. /graphs is an open read, so a client that
        // signs its writes (no bearer) can still write to graphs that exist;
        // /graph/create needs whatever write credential the server accepts.
        let existing = self.graph_iris();
        let mut graphs = vec![self.graph.clone(), provenance_graph(&self.graph)];
        if ephemeral {
            // Created on first use, so a project that never writes an
            // ephemeral seed never has the graph.
            graphs.push(crate::vocab::ephemeral_graph(&self.graph));
        }
        for g in graphs {
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
        self.ephemeral_registered |= ephemeral;
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

/// `application/x-www-form-urlencoded` encoding of one value.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `(asserted + retracted, tx)` from quipu's `/update` report, or `None` for a
/// server that answers with no body (before quipu#411).
fn update_report(body: &str) -> Option<(u64, Option<u64>)> {
    let v: Json = serde_json::from_str(body).ok()?;
    let changed = v.get("asserted")?.as_u64()? + v.get("retracted")?.as_u64()?;
    Some((changed, v.get("tx").and_then(Json::as_u64)))
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
pub use crate::vocab::provenance_graph;

/// The answer to the old-vocabulary ASK on `graph`, from the server's
/// response body. Fails closed: anything but a JSON boolean is an error,
/// never a "no".
fn ask_answer(graph: &str, body: &str) -> Result<bool> {
    serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|v| v.get("boolean").and_then(Json::as_bool))
        .ok_or_else(|| {
            SdError::failed(format!(
                "the old-vocabulary check on graph {graph} could not be completed: the ASK \
                 answer has no boolean: {}",
                body.chars().take(200).collect::<String>()
            ))
        })
}

fn term(t: &Json) -> Option<Obj> {
    let value = t["value"].as_str()?.to_string();
    match t["type"].as_str()? {
        "uri" => Some(Obj::Iri(value)),
        "literal" | "typed-literal" => Some(Obj::literal(
            value,
            t["datatype"].as_str(),
            t["xml:lang"].as_str(),
        )),
        _ => None,
    }
}

fn sparql_obj(o: &Obj) -> String {
    match o {
        Obj::Iri(i) => format!("<{i}>"),
        Obj::Str(s) => format!("\"{}\"", escape_literal(s)),
        Obj::Int(n) => format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
        Obj::Lang { lexical, lang } => format!("\"{}\"@{lang}", escape_literal(lexical)),
        Obj::Typed { lexical, datatype } => {
            format!("\"{}\"^^<{datatype}>", escape_literal(lexical))
        }
    }
}

impl Backend for RemoteBackend {
    fn snapshot(&self, at: Option<u64>) -> Result<Snapshot> {
        let mut snap = Snapshot::from_graphs(&self.facts(at)?, &self.ephemeral_facts(at)?);
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

    fn claims_of(&self, id: &str) -> Result<Vec<(u64, crate::backend::Claims)>> {
        let rows = self.select(&vocab::claims_query(&self.graph, id), None)?;
        Ok(crate::backend::claims_rows(id, rows))
    }

    fn max_write_bytes(&self) -> Option<usize> {
        Some(self.max_write_bytes)
    }

    fn legacy_present(&self) -> Result<bool> {
        for graph in [
            self.graph.clone(),
            crate::vocab::ephemeral_graph(&self.graph),
        ] {
            let body = json!({ "query": vocab::legacy_presence_query(&graph) });
            let text = self.post("/query", "application/json", &body.to_string(), false)?;
            if ask_answer(&graph, &text)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn legacy_items(&self) -> Result<u64> {
        let mut n = 0;
        for graph in [
            self.graph.clone(),
            crate::vocab::ephemeral_graph(&self.graph),
        ] {
            let rows = self.select(&vocab::legacy_count_query(&graph), None)?;
            let counts: Vec<Option<&Obj>> = rows.iter().map(|r| r.get("n")).collect();
            n += crate::backend::legacy_count(&graph, &counts)?;
        }
        Ok(n)
    }

    fn max_write_clauses(&self) -> Option<usize> {
        Some(self.max_write_clauses)
    }

    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> Result<u64> {
        if batch.is_empty() {
            return Ok(0);
        }
        // Validate first, against the server's current state.
        let current = self.facts(None)?;
        let current_eph = self.ephemeral_facts(None)?;
        let in_eph = |id: &str| current_eph.contains_key(&vocab::item_iri(id));
        validate::validate_batch(batch, &mut |id| {
            let iri = vocab::item_iri(id);
            Ok(current.get(&iri).or_else(|| current_eph.get(&iri)).cloned())
        })?;
        validate::no_shared_edge_to_ephemeral(batch, &mut |id| Ok(in_eph(id)))?;
        // Which graph each written entity lives in (a comment with its seed).
        // A seed never moves between the shared and the ephemeral graph.
        let mut eph: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for w in &batch.seeds {
            let id = &w.seed.id;
            let in_main = current.contains_key(&vocab::item_iri(id));
            if (w.seed.ephemeral && in_main) || (!w.seed.ephemeral && in_eph(id)) {
                return Err(SdError::conflict(format!(
                    "{id} already exists in the {} graph, and a seed cannot move between the \
                     shared and the ephemeral graph; nothing was written",
                    if in_eph(id) { "ephemeral" } else { "shared" }
                )));
            }
            if w.seed.ephemeral {
                eph.insert(id.clone());
            }
        }
        for id in batch
            .delete_seeds
            .iter()
            .map(|(id, _)| id)
            .chain(batch.comments.iter().map(|c| &c.seed))
            .chain(batch.delete_comments.iter().map(|(s, _)| s))
        {
            if !batch.seeds.iter().any(|w| &w.seed.id == id) && in_eph(id) {
                eph.insert(id.clone());
            }
        }
        // Check the revisions locally too, so a stale write gets a precise
        // message; the update's WHERE clause is what makes it atomic.
        let snap = Snapshot::from_graphs(&current, &current_eph);
        for w in &batch.seeds {
            let cur = snap.seeds.get(&w.seed.id);
            let iri = vocab::item_iri(&w.seed.id);
            crate::quipu_backend::check_revision(
                &w.seed.id,
                w.expected_revision,
                current.contains_key(&iri) || current_eph.contains_key(&iri),
                cur.map(|s| s.revision),
            )?;
        }
        self.register_graph(!eph.is_empty())?;

        let main_graph = self.graph.clone();
        let eph_graph = crate::vocab::ephemeral_graph(&main_graph);
        let graph_of = |id: &str| -> &str {
            if eph.contains(id) {
                &eph_graph
            } else {
                &main_graph
            }
        };
        let g = &main_graph;
        let rev = term::revision();
        // Per graph: [0] the project graph, [1] its ephemeral graph.
        let ix = |id: &str| usize::from(graph_of(id) != main_graph.as_str());
        let graphs = [main_graph.as_str(), eph_graph.as_str()];
        let mut delete = [String::new(), String::new()];
        let mut insert = [String::new(), String::new()];
        let mut guards = [String::new(), String::new()];
        let mut absent = String::new();
        let mut unions: Vec<String> = Vec::new();
        let mut var = 0usize;
        // Replacing a seed deletes only the predicates this build models: any
        // other fact on it came from a newer sd and is carried forward
        // (aegis-w3k75d.13). Removing an entity outright deletes everything.
        let owned = crate::model::Seed::owned_extra_predicates();
        let modelled = crate::model::Seed::modelled_predicates()
            .iter()
            .chain(owned.iter())
            .map(|p| format!("<{p}>"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut replace = |gi: usize,
                           iri: &str,
                           only_modelled: bool,
                           delete: &mut [String; 2],
                           unions: &mut Vec<String>| {
            delete[gi].push_str(&format!("<{iri}> ?p{var} ?o{var} . "));
            let filter = if only_modelled {
                format!(" FILTER(?p{var} IN ({modelled}))")
            } else {
                String::new()
            };
            unions.push(format!(
                "{{ GRAPH <{}> {{ <{iri}> ?p{var} ?o{var} }}{filter} }}",
                graphs[gi]
            ));
            var += 1;
        };
        for w in &batch.seeds {
            let gi = ix(&w.seed.id);
            let iri = vocab::item_iri(&w.seed.id);
            match w.expected_revision {
                Some(r) => {
                    guards[gi].push_str(&format!("<{iri}> <{rev}> {r} . "));
                    replace(gi, &iri, true, &mut delete, &mut unions);
                }
                // A new seed must be absent from BOTH graphs: ids are unique
                // across the project, whichever graph a seed lives in.
                None => {
                    for gr in graphs {
                        absent.push_str(&format!(
                            "FILTER NOT EXISTS {{ GRAPH <{gr}> {{ <{iri}> ?x ?y }} }} "
                        ));
                    }
                }
            }
            for (p, o) in w.seed.facts() {
                insert[gi].push_str(&format!("<{iri}> <{p}> {} . ", sparql_obj(&o)));
            }
        }
        for (id, r) in &batch.delete_seeds {
            let gi = ix(id);
            let iri = vocab::item_iri(id);
            guards[gi].push_str(&format!("<{iri}> <{rev}> {r} . "));
            replace(gi, &iri, false, &mut delete, &mut unions);
        }
        for (seed, index) in &batch.delete_comments {
            replace(
                ix(seed),
                &vocab::comment_iri(seed, *index),
                false,
                &mut delete,
                &mut unions,
            );
        }
        for c in &batch.comments {
            let gi = ix(&c.seed);
            let iri = vocab::comment_iri(&c.seed, c.index);
            if !batch
                .delete_comments
                .iter()
                .any(|(s, i)| *s == c.seed && *i == c.index)
            {
                absent.push_str(&format!(
                    "FILTER NOT EXISTS {{ GRAPH <{}> {{ <{iri}> ?x ?y }} }} ",
                    graphs[gi]
                ));
            }
            for (p, o) in c.facts() {
                insert[gi].push_str(&format!("<{iri}> <{p}> {} . ", sparql_obj(&o)));
            }
        }
        unions.push("{ }".to_string());
        // A block per graph that has content. The ephemeral graph only
        // appears when this batch touches it, so a project that never uses it
        // writes exactly the update it always did.
        let blocks = |parts: &[String; 2]| -> String {
            (0..2)
                .filter(|&i| i == 0 || !parts[i].is_empty())
                .map(|i| format!("GRAPH <{}> {{ {}}} ", graphs[i], parts[i]))
                .collect()
        };
        let mut update = String::new();
        if delete.iter().any(|d| !d.is_empty()) {
            update.push_str(&format!("DELETE {{ {}}}\n", blocks(&delete)));
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
                format!(
                    "{}\n{}\n{update}{}{}{}{}",
                    ctx.now, ctx.actor, insert[0], insert[1], delete[0], delete[1]
                )
                .as_bytes()
            )[7..31]
        );
        let mut prov = String::new();
        for (iri, facts) in crate::backend::write_record(&write_iri, batch, ctx) {
            for (p, o) in facts {
                prov.push_str(&format!("<{iri}> <{p}> {} . ", sparql_obj(&o)));
            }
        }
        update.push_str(&format!(
            "INSERT {{ {}GRAPH <{}> {{ {prov} }} }}\n",
            blocks(&insert),
            provenance_graph(g)
        ));
        let guard_block: String = (0..2)
            .filter(|&i| !guards[i].is_empty())
            .map(|i| format!("GRAPH <{}> {{ {}}} ", graphs[i], guards[i]))
            .collect();
        update.push_str(&format!(
            "WHERE {{ {guard_block}{absent}{} }}",
            unions.join(" UNION ")
        ));
        // Form-encoded so the caller's actor and source travel as fields: quipu
        // records them on the transaction (>= quipu#413), and a signed write may
        // not carry a query string. Older servers ignore the extra fields.
        let form = format!(
            "update={}&actor={}&source=seeds",
            form_encode(&update),
            form_encode(&ctx.actor)
        );
        // Every branch but the closing `{ }` is a guard, and so is every
        // absence filter. quipu-server aborts on deep nesting (aegis-rq1afp).
        let clauses = absent.matches("FILTER NOT EXISTS").count() + unions.len() - 1;
        if clauses > self.max_write_clauses {
            return Err(SdError::refused(format!(
                "{}: this write nests {clauses} guard clauses and the limit is {} \
                 (SEEDS_MAX_WRITE_CLAUSES); nothing was sent. sd sync splits a larger push into \
                 batches.",
                crate::backend::TOO_LARGE,
                self.max_write_clauses
            )));
        }
        if form.len() > self.max_write_bytes {
            // Refused here, unsent: a server that refuses an oversized body can
            // close the connection mid-request, and that reads as a lost
            // response (exit 8) for a write that was never evaluated.
            return Err(SdError::refused(format!(
                "{}: this write is {} bytes and the limit is {} (SEEDS_MAX_WRITE_BYTES); nothing \
                 was sent. sd sync splits a larger push into batches; a single verb this large \
                 means one seed or comment is over the limit.",
                crate::backend::TOO_LARGE,
                form.len(),
                self.max_write_bytes
            )));
        }
        let sent = self.send("/update", "application/x-www-form-urlencoded", &form, true);
        if let Ok(body) = &sent {
            // quipu >= #411 reports what it committed. Every seeds write also
            // inserts a fresh write-provenance record, so asserted > 0 exactly
            // when the WHERE guard matched: the count, not a full re-read of the
            // project graph, decides landed vs lost (aegis-w3k75d.15).
            if let Some((changed, tx)) = update_report(body) {
                if changed == 0 {
                    return Err(SdError::conflict(self.lost_race(batch)));
                }
                return Ok(tx.unwrap_or(0));
            }
        }
        if let Err((ambiguous, e)) = sent {
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
            return Err(SdError::conflict(self.lost_race(batch)));
        }
        // quipu's /update returns no transaction id, so there is none to
        // report; callers see that a write happened through `Report::wrote`.
        Ok(0)
    }
}

impl RemoteBackend {
    /// The refusal for a write whose guard did not match: another writer moved
    /// the seed first. Names who holds each written seed NOW, so a lost claim
    /// says "claimed by X" instead of only "re-read and retry".
    fn lost_race(&self, batch: &WriteBatch) -> String {
        let holders: Vec<String> = match (self.facts(None), self.ephemeral_facts(None)) {
            (Ok(f), Ok(e)) => {
                let now = Snapshot::from_graphs(&f, &e);
                batch
                    .seeds
                    .iter()
                    .map(|w| match now.seeds.get(&w.seed.id) {
                        Some(s) => format!(
                            "{} is now {}{}",
                            s.id,
                            s.status,
                            s.assignee
                                .as_deref()
                                .map(|a| format!(", claimed by {a}"))
                                .unwrap_or_default()
                        ),
                        None => format!("{} does not exist now", w.seed.id),
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let ids: Vec<&str> = batch.seeds.iter().map(|w| w.seed.id.as_str()).collect();
        let detail = if holders.is_empty() {
            format!("for {}", ids.join(", "))
        } else {
            format!("({})", holders.join("; "))
        };
        format!(
            "another writer changed it first {detail}; nothing was written. Re-read before \
             retrying."
        )
    }

    /// What of `batch` the server does NOT show (empty when all of it landed).
    /// /update reports no affected count, so this read-back is the only proof
    /// that the precondition held.
    fn unconfirmed(&self, batch: &WriteBatch) -> Result<Vec<String>> {
        let after = Snapshot::from_graphs(&self.facts(None)?, &self.ephemeral_facts(None)?);
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

    // seeds#96 follow-up: the old-vocabulary ASK fails closed.
    #[test]
    fn the_old_vocabulary_ask_fails_closed() {
        assert!(ask_answer("urn:g", r#"{"head":{},"boolean":true}"#).unwrap());
        assert!(!ask_answer("urn:g", r#"{"head":{},"boolean":false}"#).unwrap());
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"head":{},"boolean":"false"}"#,
            r#"{"head":{},"boolean":null}"#,
            r#"{"head":{"vars":["n"]},"results":{"bindings":[]}}"#,
        ] {
            let e = ask_answer("urn:g", bad).expect_err(bad);
            assert!(
                e.message.contains("could not be completed"),
                "{bad}: {}",
                e.message
            );
        }
    }

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

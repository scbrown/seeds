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

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value as Json};

use crate::backend::{Backend, Ctx, WriteBatch};
use crate::error::{ErrorKind, Result, SdError};
use crate::model::{Fact, Obj, Snapshot};
use crate::validate::{self, escape_literal};
use crate::vocab::{self, term};

/// Subjects per targeted read, one `UNION` branch each. A seed carries a few
/// dozen facts, so a chunk stays well under quipu's 10,000-row result cap (a
/// truncated answer is split anyway), and the branch count stays far below
/// the nesting that aborts quipu-server (2,000 branches, aegis-rq1afp).
const SUBJECTS_PER_READ: usize = 100;

/// Subjects per page of a subject listing: under quipu's 10,000-row cap, so a
/// full page is never also a truncated one.
const SUBJECT_PAGE: usize = 5000;

/// quipu's `/query` row cap. Its `truncated` flag is NOT in the W3C results
/// JSON seeds asks for (the standard shape has no field for it), so a capped
/// answer arrives looking complete. A read that may exceed the cap therefore
/// carries `LIMIT ROW_CAP` and takes a full answer as a truncated one.
const ROW_CAP: usize = 10_000;

/// The classes of the subjects seeds writes into a project graph: seeds
/// (`schema:Action`) and their comments (`schema:Comment`). Every other IRI a
/// seed's facts mention (principals, shuttle runs, other seeds) is an
/// object, never a subject; a subject of any other class came from another
/// writer, and [`RemoteBackend::graph_facts`] still finds it by its count.
fn listed_classes() -> Vec<String> {
    vec![term::work_item(), term::comment()]
}

/// A read of every fact of `subjects` in `graph`: one bound pattern per
/// subject, joined by `UNION`.
fn subjects_query(graph: &str, subjects: &[&String]) -> String {
    let branches: Vec<String> = subjects
        .iter()
        .map(|s| format!("{{ BIND(<{s}> AS ?s) GRAPH <{graph}> {{ <{s}> ?p ?o }} }}"))
        .collect();
    format!(
        "SELECT ?s ?p ?o WHERE {{ {} }} LIMIT {ROW_CAP}",
        branches.join(" UNION ")
    )
}

/// Facts grouped by subject IRI.
type Facts = BTreeMap<String, Vec<Fact>>;

/// Every subject a write's checks read: the seeds it writes or removes, the
/// seeds their new edges point at (the dangling-edge check), the seeds its
/// comments and comment removals hang off (which graph each lives in), and
/// the comments themselves (for the read-back).
fn batch_subjects(batch: &WriteBatch) -> BTreeSet<String> {
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for w in &batch.seeds {
        ids.insert(&w.seed.id);
        ids.extend(w.seed.blocked_on.iter().map(String::as_str));
    }
    ids.extend(batch.delete_seeds.iter().map(|(id, _)| id.as_str()));
    ids.extend(batch.comments.iter().map(|c| c.seed.as_str()));
    ids.extend(batch.delete_comments.iter().map(|(s, _)| s.as_str()));
    let mut out: BTreeSet<String> = ids.into_iter().map(vocab::item_iri).collect();
    out.extend(
        batch
            .comments
            .iter()
            .map(|c| vocab::comment_iri(&c.seed, c.index)),
    );
    out.extend(
        batch
            .delete_comments
            .iter()
            .map(|(s, i)| vocab::comment_iri(s, *i)),
    );
    out
}

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
    /// Subjects per page of a subject listing ([`SUBJECT_PAGE`]; smaller in
    /// tests, so a small graph still spans pages).
    subject_page: usize,
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

/// Prefix of the error for a read quipu gave up on (HTTP 408 to `/query`).
/// A read changes nothing, so the failure is definite and the caller may ask
/// again for less.
const QUERY_TIMEOUT: &str = "quipu query timed out";

fn is_query_timeout(e: &SdError) -> bool {
    e.message.starts_with(QUERY_TIMEOUT)
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
            subject_page: SUBJECT_PAGE,
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
            Err(ureq::Error::Status(408, r)) if path == "/query" => Err((
                false,
                SdError::failed(format!(
                    "{QUERY_TIMEOUT}: quipu {path} answered HTTP 408: {}",
                    r.into_string().unwrap_or_default()
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
        self.select_marked(sparql, at).map(|(rows, _)| rows)
    }

    /// [`Self::select`], also returning whether quipu cut the result short
    /// (its `truncated` flag): a short answer must never read as a complete one.
    fn select_marked(
        &self,
        sparql: &str,
        at: Option<u64>,
    ) -> Result<(Vec<BTreeMap<String, Obj>>, bool)> {
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
        Ok((out, v["truncated"].as_bool().unwrap_or(false)))
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

    /// The current facts of just these subjects, in the project graph and in
    /// its ephemeral graph. A write needs only the items it touches (their
    /// revisions, the targets of their edges, which graph each lives in), and
    /// reading the whole graph instead made every batch of a large push cost
    /// more than the last (aegis-w3k75d.15).
    fn subjects_facts(&self, subjects: &BTreeSet<String>) -> Result<(Facts, Facts)> {
        let mut main = Facts::new();
        let mut eph = Facts::new();
        let eph_graph = crate::vocab::ephemeral_graph(&self.graph);
        for (graph, out) in [
            (self.graph.as_str(), &mut main),
            (eph_graph.as_str(), &mut eph),
        ] {
            self.read_all(graph, subjects, None, out)?;
        }
        Ok((main, eph))
    }

    /// The facts of every subject in `subjects`, in `graph`, as of `at`, read
    /// [`SUBJECTS_PER_READ`] at a time.
    fn read_all(
        &self,
        graph: &str,
        subjects: &BTreeSet<String>,
        at: Option<u64>,
        out: &mut Facts,
    ) -> Result<()> {
        let all: Vec<&String> = subjects.iter().collect();
        for chunk in all.chunks(SUBJECTS_PER_READ) {
            self.read_subjects(graph, chunk, at, out)?;
        }
        Ok(())
    }

    /// One read of `subjects` in `graph`, halved and retried when quipu
    /// reports the answer truncated.
    ///
    /// Shaped as a `UNION` of one bound pattern per subject, never
    /// `VALUES ?s { ... }`: quipu does not push a `VALUES` block into the
    /// pattern, so a `VALUES` read scanned the whole graph on every call
    /// (40 subjects: 6-9 s on a 28k-seed board), while each bound branch is
    /// an index lookup (the same 40: 0.02 s, the same rows).
    fn read_subjects(
        &self,
        graph: &str,
        subjects: &[&String],
        at: Option<u64>,
        out: &mut Facts,
    ) -> Result<()> {
        if subjects.is_empty() {
            return Ok(());
        }
        let answer = self.select_marked(&subjects_query(graph, subjects), at);
        // A read quipu gave up on is asked again in halves, like a truncated
        // one: a busy server times out a long read that halves answer.
        let (rows, truncated) = match answer {
            Err(e) if is_query_timeout(&e) && subjects.len() > 1 => (Vec::new(), true),
            other => other?,
        };
        let truncated = truncated || rows.len() >= ROW_CAP;
        if truncated {
            if subjects.len() == 1 {
                return Err(SdError::failed(format!(
                    "quipu truncated the facts of {} even read alone; refusing to act \
                     on a partial view",
                    subjects[0]
                )));
            }
            let (a, b) = subjects.split_at(subjects.len() / 2);
            self.read_subjects(graph, a, at, out)?;
            return self.read_subjects(graph, b, at, out);
        }
        for mut r in rows {
            if let (Some(Obj::Iri(s)), Some(Obj::Iri(p)), Some(o)) =
                (r.remove("s"), r.remove("p"), r.remove("o"))
            {
                out.entry(s).or_default().push((p, o));
            }
        }
        Ok(())
    }

    /// The IRI subjects matching `pattern` (over `?s`, inside `graph`), as
    /// of `at`, in code-point order.
    ///
    /// Paged by key (`STR(?s) > last`), never by `OFFSET`: each page is
    /// bounded and starts where the last ended, and no page can be cut short
    /// by quipu's row cap without saying so (a truncated page is refused).
    /// Ordered by `STR(?s)`, the key the filter compares: quipu orders bare
    /// IRIs by its own term ids, not by their text, and paging by one order
    /// while filtering by another skips subjects (the order is checked, and
    /// a page out of order is refused).
    fn subjects_where(&self, graph: &str, pattern: &str, at: Option<u64>) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        loop {
            let after = out.last().map_or_else(String::new, |last| {
                format!("FILTER(STR(?s) > \"{}\") ", escape_literal(last))
            });
            let (rows, truncated) = self.select_marked(
                &format!(
                    "SELECT ?s WHERE {{ GRAPH <{graph}> {{ {pattern} }} \
                     FILTER(isIRI(?s)) {after}}} \
                     ORDER BY STR(?s) LIMIT {}",
                    self.subject_page
                ),
                at,
            )?;
            if truncated {
                return Err(SdError::failed(format!(
                    "quipu truncated a page of {} subjects of {graph}; refusing a partial \
                     snapshot",
                    self.subject_page
                )));
            }
            let n = rows.len();
            let before = out.len();
            for mut r in rows {
                let Some(Obj::Iri(s)) = r.remove("s") else {
                    continue;
                };
                // No DISTINCT (it made a page ~80x slower on a 28k-seed
                // board): a subject matching more than once arrives as a run
                // of equal rows, kept once.
                if out.last().is_some_and(|last| *last == s) {
                    continue;
                }
                // The next page starts after the last subject, so the pages
                // must come in the order the key compares; anything else
                // could skip a subject without a sign.
                if out.last().is_some_and(|last| s.as_str() < last.as_str()) {
                    return Err(SdError::failed(format!(
                        "quipu listed the subjects of {graph} out of order ({s} after {}); \
                         refusing a snapshot that could skip one",
                        out.last().map_or("", String::as_str)
                    )));
                }
                out.push(s);
            }
            if n < self.subject_page {
                return Ok(out);
            }
            if out.len() == before {
                return Err(SdError::failed(format!(
                    "quipu answered a full page of subjects of {graph} with no new one; \
                     refusing a snapshot that cannot page past it"
                )));
            }
        }
    }

    /// One graph's facts, grouped by subject, as of `at`: exactly the facts
    /// of a whole-graph read, without one.
    ///
    /// A whole-graph `ORDER BY ?s ?p ?o LIMIT/OFFSET` scan re-sorted every
    /// triple for every page (726k triples on a 28k-seed board: ~363 pages of
    /// ~5.4 s). Instead the subjects are listed by the classes seeds writes
    /// ([`listed_classes`], bound patterns, keyset-paged) and their facts read
    /// in [`SUBJECTS_PER_READ`] batches. The graph's triple count is the
    /// check: facts left over belong to subjects of no listed class (written
    /// by something other than seeds), and only then are those listed, with
    /// a full scan, so the answer equals the whole-graph read either way.
    fn graph_facts(&self, graph: &str, at: Option<u64>) -> Result<Facts> {
        self.graph_facts_listing(graph, at, &listed_classes())
            .map(|(facts, _)| facts)
    }

    /// [`Self::graph_facts`] over the subjects of `classes`, also returning
    /// how many subjects only the fallback scan found.
    fn graph_facts_listing(
        &self,
        graph: &str,
        at: Option<u64>,
        classes: &[String],
    ) -> Result<(Facts, usize)> {
        let total = |f: &Facts| f.values().map(Vec::len).sum::<usize>();
        for _attempt in 0..3 {
            let expected = self.count(graph, at)?;
            let mut by_subject = Facts::new();
            if expected == 0 {
                return Ok((by_subject, 0));
            }
            let mut listed = BTreeSet::new();
            for class in classes {
                listed.extend(self.subjects_where(graph, &format!("?s a <{class}>"), at)?);
            }
            self.read_all(graph, &listed, at, &mut by_subject)?;
            let mut unlisted = 0;
            if total(&by_subject) < expected {
                let not_listed: String = classes
                    .iter()
                    .map(|c| format!("FILTER NOT EXISTS {{ ?s a <{c}> }} "))
                    .collect();
                let others: BTreeSet<String> = self
                    .subjects_where(graph, &format!("?s ?p ?o {not_listed}"), at)?
                    .into_iter()
                    .filter(|s| !listed.contains(s))
                    .collect();
                unlisted = others.len();
                self.read_all(graph, &others, at, &mut by_subject)?;
            }
            if total(&by_subject) == expected {
                return Ok((by_subject, unlisted));
            }
            // The graph changed between the count and the reads (or the
            // server capped a read): read it again rather than return a
            // short ledger.
        }
        Err(SdError::failed(
            "could not read a consistent snapshot of the remote graph (it kept changing, \
             or the server truncates results below the page size)",
        ))
    }

    /// The old whole-graph read, kept as the reference the batched read is
    /// tested against.
    #[cfg(test)]
    fn graph_facts_scan(&self, graph: &str, at: Option<u64>) -> Result<Facts> {
        const PAGE: usize = 2000;
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
        }
        Err(SdError::failed("inconsistent scan"))
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
        let (current, current_eph) = self.subjects_facts(&batch_subjects(batch))?;
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
        let holders: Vec<String> = match self.subjects_facts(&batch_subjects(batch)) {
            Ok((f, e)) => {
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
        let (f, e) = self.subjects_facts(&batch_subjects(batch))?;
        let after = Snapshot::from_graphs(&f, &e);
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

    /// A one-thread HTTP server: `/health` is 200, a `/query` binding more
    /// than one subject (or a subject named `slow`) is HTTP 408 (quipu's
    /// query timeout), and a one-subject `/query` returns one fact for it.
    /// Returns the base URL and the number of `/query` requests seen.
    fn timing_out_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let queries = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = std::sync::Arc::clone(&queries);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                let (status, reply) = if first.contains("/query") {
                    seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let subjects: Vec<&str> = body
                        .split("BIND(<")
                        .skip(1)
                        .filter_map(|rest| rest.split('>').next())
                        .collect();
                    if subjects.len() > 1 || subjects.iter().any(|s| s.contains("slow")) {
                        (
                            "408 Request Timeout",
                            r#"{"error":"query timeout"}"#.to_string(),
                        )
                    } else {
                        let s = subjects.first().copied().unwrap_or("");
                        (
                            "200 OK",
                            format!(
                                r#"{{"results":{{"bindings":[{{"s":{{"type":"uri","value":"{s}"}},"p":{{"type":"uri","value":"urn:p"}},"o":{{"type":"literal","value":"v"}}}}]}}}}"#
                            ),
                        )
                    }
                } else {
                    ("200 OK", r#"{"status":"ok"}"#.to_string())
                };
                let mut stream = reader.into_inner();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        (base, queries)
    }

    #[test]
    fn a_read_quipu_times_out_is_asked_again_in_halves() {
        let (base, queries) = timing_out_server();
        let remote = RemoteBackend::connect(&base, "urn:g", None, None, &[]).unwrap();
        let ids: Vec<String> = (0..4).map(|i| format!("urn:s{i}")).collect();
        let refs: Vec<&String> = ids.iter().collect();
        let mut out = Facts::new();
        remote
            .read_subjects("urn:g", &refs, None, &mut out)
            .unwrap();
        assert_eq!(
            out.len(),
            4,
            "every subject read once the halves fit: {out:?}"
        );
        // 4 -> 2+2 -> 1+1+1+1: three timed-out reads, then four that answer.
        assert_eq!(queries.load(std::sync::atomic::Ordering::SeqCst), 7);

        // One subject that still times out is a definite error, never a gap.
        let lone = String::from("urn:slow");
        let e = remote
            .read_subjects("urn:g", &[&lone], None, &mut Facts::new())
            .unwrap_err();
        assert!(is_query_timeout(&e), "{}", e.message);
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

    /// A quipu-server of our own on a free loopback port, killed on drop.
    struct Server {
        child: std::process::Child,
        base: String,
        dir: std::path::PathBuf,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Start `SEEDS_TEST_QUIPU_SERVER`, or `None` when it is not set.
    fn quipu_server() -> Option<Server> {
        let bin = std::env::var("SEEDS_TEST_QUIPU_SERVER").ok()?;
        for attempt in 0..5 {
            let dir = std::env::temp_dir().join(format!(
                "seeds-remote-test-{}-{attempt}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let child = std::process::Command::new(&bin)
                .args(["--db", dir.join("q.db").to_str().unwrap()])
                .args(["--bind", &format!("127.0.0.1:{port}")])
                .current_dir(&dir)
                .env("HOME", &dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let mut server = Server {
                child,
                base: format!("http://127.0.0.1:{port}"),
                dir,
            };
            for _ in 0..100 {
                if !matches!(server.child.try_wait(), Ok(None)) {
                    break; // ours exited: the port was taken
                }
                let healthy = ureq::get(&format!("{}/health", server.base))
                    .timeout(Duration::from_secs(2))
                    .call()
                    .is_ok();
                if healthy && matches!(server.child.try_wait(), Ok(None)) {
                    return Some(server);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        panic!("quipu-server did not start");
    }

    /// Run a SPARQL update on the server; the transaction it committed.
    fn update(remote: &RemoteBackend, sparql: &str) -> u64 {
        let body = remote
            .post(
                "/update",
                "application/x-www-form-urlencoded",
                &format!("update={}", form_encode(sparql)),
                true,
            )
            .unwrap();
        update_report(&body)
            .and_then(|(_, tx)| tx)
            .unwrap_or_else(|| panic!("no tx in {body}"))
    }

    /// Facts with each subject's facts in one order, so two reads compare.
    fn sorted(mut f: Facts) -> Facts {
        for v in f.values_mut() {
            v.sort();
        }
        f
    }

    // aegis-aane52 S1: the batched read returns exactly the facts the old
    // whole-graph scan did, now and as of an earlier transaction, including
    // subjects seeds never types, and lists every seed and comment by class
    // (a fallback scan finds only the subjects of no listed class).
    #[test]
    fn the_batched_graph_read_equals_the_whole_graph_scan() {
        let Some(server) = quipu_server() else {
            eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
            return;
        };
        let graph = "https://seeds.local/project/equiv";
        let mut remote = RemoteBackend::connect(&server.base, graph, None, None, &[]).unwrap();
        // Pages of 7 subjects, so 250 seeds and their comments span many.
        remote.subject_page = 7;
        remote.register_graph(false).unwrap();

        let action = term::work_item();
        let comment = term::comment();
        let mut nt = String::new();
        let seeds = 250;
        let mut comments = 0;
        for i in 0..seeds {
            // Ids whose IRIs sort differently by byte and by number.
            let id = format!("eq-{i}{}", if i % 3 == 0 { "a.b" } else { "" });
            let s = vocab::item_iri(&id);
            nt.push_str(&format!(
                "<{s}> a <{action}> ; <{}> \"{id}\" ; <{}> \"title {i}\" ; \
                 <{}> {i} ; <{}> \"line one\\nline \\\"two\\\"\" .\n",
                term::identifier(),
                term::name(),
                term::priority(),
                term::description(),
            ));
            if i > 0 {
                nt.push_str(&format!(
                    "<{s}> <{}> <{}> .\n",
                    term::blocked_on(),
                    vocab::item_iri(&format!("eq-{}", i - 1))
                ));
            }
            for n in 1..=(i % 3) {
                let c = vocab::comment_iri(&id, n as u64);
                nt.push_str(&format!(
                    "<{c}> a <{comment}> ; <{}> <{s}> ; <{}> {n} ; <{}> \"c{n}\"@en .\n",
                    term::comment_on(),
                    term::comment_index(),
                    term::text(),
                ));
                comments += 1;
            }
        }
        // Subjects seeds never writes: one untyped (and not ASCII), one of
        // another class.
        nt.push_str("<urn:x:annotation-\u{e9}> <urn:x:note> \"from another writer\" ; <urn:x:on> <urn:x:thing> .\n");
        nt.push_str("<urn:x:event> a <https://schema.org/Event> ; <urn:x:when> \"2026-10-07\"^^<http://www.w3.org/2001/XMLSchema#date> .\n");
        let tx1 = update(
            &remote,
            &format!("INSERT DATA {{ GRAPH <{graph}> {{ {nt} }} }}"),
        );
        assert!(comments > 100, "enough comments to matter: {comments}");

        let (batched, unlisted) = remote
            .graph_facts_listing(graph, None, &listed_classes())
            .unwrap();
        let scanned = remote.graph_facts_scan(graph, None).unwrap();
        assert_eq!(scanned.len(), seeds + comments + 2);
        assert_eq!(sorted(batched.clone()), sorted(scanned));
        // Only the two foreign subjects needed the fallback: every seed and
        // every comment was listed by its class.
        assert_eq!(unlisted, 2, "subjects found only by the fallback scan");
        assert_eq!(
            remote.graph_facts(graph, None).unwrap().len(),
            batched.len()
        );

        // Change the graph, then read as of before the change.
        let first = vocab::item_iri("eq-1");
        update(
            &remote,
            &format!(
                "DELETE {{ GRAPH <{graph}> {{ <{first}> ?p ?o }} }} \
                 INSERT {{ GRAPH <{graph}> {{ <urn:x:late> <urn:x:p> \"late\" . }} }} \
                 WHERE {{ GRAPH <{graph}> {{ <{first}> ?p ?o }} }}"
            ),
        );
        let then = sorted(remote.graph_facts(graph, Some(tx1)).unwrap());
        assert_eq!(
            then,
            sorted(remote.graph_facts_scan(graph, Some(tx1)).unwrap())
        );
        assert_eq!(
            then,
            sorted(batched),
            "as of tx {tx1}, the graph as written"
        );
        let now = sorted(remote.graph_facts(graph, None).unwrap());
        assert_eq!(now, sorted(remote.graph_facts_scan(graph, None).unwrap()));
        assert!(!now.contains_key(&first) && now.contains_key("urn:x:late"));

        // The targeted read (writes' checks) answers the same facts.
        let some: BTreeSet<String> = now.keys().take(150).cloned().collect();
        let (main, eph) = remote.subjects_facts(&some).unwrap();
        assert!(eph.is_empty());
        let want: Facts = now.into_iter().filter(|(k, _)| some.contains(k)).collect();
        assert_eq!(sorted(main), want);

        // An empty graph reads as empty, not as an error.
        let empty = "https://seeds.local/project/empty";
        assert!(remote.graph_facts(empty, None).unwrap().is_empty());

        // quipu caps /query at 10,000 rows and, in the W3C JSON seeds reads,
        // says nothing about it: a subject with more facts than that must be
        // refused, never read as complete.
        let fat_graph = "https://seeds.local/project/fat";
        remote
            .post(
                "/graph/create",
                "application/json",
                &json!({ "graph": fat_graph }).to_string(),
                true,
            )
            .unwrap();
        let fat = vocab::item_iri("fat-1");
        let small = vocab::item_iri("fat-2");
        let mut nt = format!("<{small}> a <{action}> . <{fat}> a <{action}> .\n");
        for n in 0..ROW_CAP {
            nt.push_str(&format!("<{fat}> <urn:x:n> {n} .\n"));
        }
        update(
            &remote,
            &format!("INSERT DATA {{ GRAPH <{fat_graph}> {{ {nt} }} }}"),
        );
        let e = remote
            .read_subjects(fat_graph, &[&small, &fat], None, &mut Facts::new())
            .unwrap_err();
        assert!(e.message.contains("even read alone"), "{}", e.message);
        assert!(remote.graph_facts(fat_graph, None).is_err());
        let mut alone = Facts::new();
        remote
            .read_subjects(fat_graph, &[&small], None, &mut alone)
            .unwrap();
        assert_eq!(alone[&small].len(), 1);
    }
}

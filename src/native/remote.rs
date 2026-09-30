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
    let hint = if code == 401 || code == 403 {
        " The server wants a bearer token for writes: set SEEDS_QUIPU_TOKEN, or \
         [quipu] token_file in the config."
    } else {
        ""
    };
    let body: String = body.chars().take(500).collect();
    SdError::failed(format!("quipu {what} failed (HTTP {code}): {body}{hint}"))
}

impl RemoteBackend {
    /// Connect to the quipu server at `base` (checked with `GET /health`).
    pub fn connect(base: &str, graph: &str, token: Option<String>) -> Result<Self> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build();
        let b = Self {
            base: base.trim_end_matches('/').to_string(),
            graph: graph.to_string(),
            token,
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
        let url = format!("{}{path}", self.base);
        let mut req = self
            .agent
            .post(&url)
            .set("Content-Type", content_type)
            .set("Accept", "application/sparql-results+json")
            .set("X-Quipu-Client", "seeds");
        if auth {
            if let Some(t) = &self.token {
                req = req.set("Authorization", &format!("Bearer {t}"));
            }
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
        self.post(
            "/graph/create",
            "application/json",
            &json!({ "graph": self.graph }).to_string(),
            true,
        )?;
        self.graph_registered = true;
        Ok(())
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

    fn commit(&mut self, batch: &WriteBatch, _ctx: &Ctx) -> Result<u64> {
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
        if !insert.is_empty() {
            update.push_str(&format!("INSERT {{ GRAPH <{g}> {{ {insert}}} }}\n"));
        }
        let guard_block = if guards.is_empty() {
            String::new()
        } else {
            format!("GRAPH <{g}> {{ {guards}}} ")
        };
        update.push_str(&format!(
            "WHERE {{ {guard_block}{absent}{} }}",
            unions.join(" UNION ")
        ));
        self.post("/update", "application/sparql-update", &update, true)?;

        // Read back: /update reports no affected count, so the only proof
        // that the precondition held is that the store now says what we sent.
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
        if !missing.is_empty() {
            return Err(SdError::conflict(format!(
                "the remote store does not show this write for {} (another writer changed it \
                 first, or changed it again right after). Re-read and retry; nothing is assumed \
                 to have landed.",
                missing.join(", ")
            )));
        }
        Ok(0)
    }
}

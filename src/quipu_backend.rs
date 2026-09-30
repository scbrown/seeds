//! [`Backend`] on a quipu store.
//!
//! One project is one named graph. A seed is an entity in that graph; each
//! field is a fact (see [`Seed::facts`]). A write retracts the facts that
//! changed and asserts their replacements in ONE quipu transaction, so the
//! transaction id is the cursor and `--at <tx>` reads any seed as it stood.
//!
//! The store is target-agnostic: the native CLI opens a file, wasm32 and the
//! tests open one in memory. Nothing here touches the filesystem or a clock.

use std::collections::BTreeMap;

use quipu::sparql::{query_temporal, TemporalContext};
use quipu::store::{Datum, Store};
use quipu::types::{Op, Value};

use crate::backend::{Backend, Ctx, WriteBatch};
use crate::error::{Result, SdError};
use crate::model::{Comment, Fact, Obj, Seed, Snapshot};
use crate::vocab::{self, term};

/// A project stored as a named graph in a quipu store.
pub struct QuipuBackend {
    store: Store,
    graph_iri: String,
    writable: bool,
    validate: bool,
}

impl QuipuBackend {
    /// Wrap a store opened for writing, registering the project graph.
    pub fn writable(store: Store, graph_iri: &str) -> Result<Self> {
        store.graph_create(graph_iri)?;
        Ok(Self {
            store,
            graph_iri: graph_iri.to_string(),
            writable: true,
            validate: cfg!(feature = "shacl"),
        })
    }

    /// Wrap a store opened read-only. [`Backend::commit`] refuses.
    pub fn read_only(store: Store, graph_iri: &str) -> Self {
        Self {
            store,
            graph_iri: graph_iri.to_string(),
            writable: false,
            validate: false,
        }
    }

    /// A fresh in-memory store: the wasm32 backend, and the tests'.
    pub fn in_memory(graph_iri: &str) -> Result<Self> {
        Self::writable(Store::open_in_memory()?, graph_iri)
    }

    /// Whether writes are validated against [`vocab::SHAPES_TURTLE`]. True in
    /// builds with the `shacl` feature (the native CLI); the wasm build has no
    /// SHACL engine and says so here rather than claiming a check.
    pub fn validates(&self) -> bool {
        self.validate
    }

    /// The project graph IRI.
    pub fn graph_iri(&self) -> &str {
        &self.graph_iri
    }

    /// The underlying store (for callers that want quipu's own surfaces).
    pub fn store(&self) -> &Store {
        &self.store
    }

    fn ctx(at: Option<u64>) -> Result<TemporalContext> {
        let as_of_tx = match at {
            None => None,
            Some(t) => Some(
                i64::try_from(t)
                    .map_err(|_| SdError::usage(format!("--at {t} is out of range")))?,
            ),
        };
        Ok(TemporalContext {
            as_of_tx,
            ..TemporalContext::default()
        })
    }

    fn obj_of(&self, v: &Value) -> Result<Option<Obj>> {
        Ok(match v {
            Value::Ref(id) => Some(Obj::Iri(self.store.resolve(*id)?)),
            Value::Str(s) => Some(Obj::Str(s.clone())),
            Value::Int(n) => Some(Obj::Int(*n)),
            _ => None,
        })
    }

    fn value_of(&self, o: &Obj) -> Result<Value> {
        Ok(match o {
            Obj::Iri(iri) => Value::Ref(self.store.intern(iri)?),
            Obj::Str(s) => Value::Str(s.clone()),
            Obj::Int(n) => Value::Int(*n),
        })
    }

    /// Whether `entity` is typed `aegis:WorkItem` in the project graph now.
    #[cfg(not(feature = "shacl"))]
    fn is_work_item(&self, g: i64, iri: &str) -> Result<bool> {
        let Some(e) = self.store.lookup(iri)? else {
            return Ok(false);
        };
        let ty = self.store.intern(vocab::RDF_TYPE)?;
        let wi = self.store.intern(&term::work_item())?;
        Ok(self
            .store
            .entity_facts_in_graph(e, g)?
            .iter()
            .any(|f| f.attribute == ty && f.value == Value::Ref(wi)))
    }

    #[cfg(feature = "shacl")]
    fn validate_post_state(&self, g: i64, batch: &WriteBatch) -> Result<()> {
        let mut nt = String::new();
        let written: std::collections::BTreeSet<&str> =
            batch.seeds.iter().map(|w| w.seed.id.as_str()).collect();
        let mut included = std::collections::BTreeSet::new();
        for w in &batch.seeds {
            let s = vocab::item_iri(&w.seed.id);
            for (p, o) in w.seed.facts() {
                push_ntriple(&mut nt, &s, &p, &o);
            }
            // sh:class on blockedOn needs the target in the data graph. Add
            // each target's CURRENT facts (a target is itself a WorkItem, so
            // it is validated whole, not as a bare type triple). A dangling
            // edge adds nothing and fails the shape instead of passing it.
            for b in &w.seed.blocked_on {
                if written.contains(b.as_str()) || !included.insert(b.clone()) {
                    continue;
                }
                let t = vocab::item_iri(b);
                if let Some(e) = self.store.lookup(&t)? {
                    for f in self.store.entity_facts_in_graph(e, g)? {
                        if let Some(o) = self.obj_of(&f.value)? {
                            let p = self.store.resolve(f.attribute)?;
                            push_ntriple(&mut nt, &t, &p, &o);
                        }
                    }
                }
            }
        }
        let feedback = quipu::validate_shapes(vocab::SHAPES_TURTLE, &nt)?;
        if feedback.conforms {
            return Ok(());
        }
        let reasons: Vec<String> = feedback
            .results
            .iter()
            .map(|r| {
                format!(
                    "{} {}: {}",
                    r.focus_node,
                    r.path.clone().unwrap_or_default(),
                    r.message.clone().unwrap_or_else(|| r.component.clone())
                )
            })
            .collect();
        Err(SdError::refused(format!(
            "the write does not conform to the WorkItem shapes; nothing was written:\n  {}",
            reasons.join("\n  ")
        )))
    }

    #[cfg(not(feature = "shacl"))]
    fn validate_post_state(&self, g: i64, batch: &WriteBatch) -> Result<()> {
        // No SHACL engine in this build. The one structural rule the shapes
        // carry that the engine does not already enforce is blockedOn pointing
        // at a real WorkItem, so check that one by hand.
        for w in &batch.seeds {
            for b in &w.seed.blocked_on {
                let in_batch = batch.seeds.iter().any(|x| &x.seed.id == b);
                if !in_batch && !self.is_work_item(g, &vocab::item_iri(b))? {
                    return Err(SdError::refused(format!(
                        "{} is blocked on {b}, which is not a seed in this project",
                        w.seed.id
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "shacl")]
fn push_ntriple(out: &mut String, s: &str, p: &str, o: &Obj) {
    let obj = match o {
        Obj::Iri(i) => format!("<{i}>"),
        Obj::Str(v) => format!("\"{}\"", escape_literal(v)),
        Obj::Int(n) => format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
    };
    out.push_str(&format!("<{s}> <{p}> {obj} .\n"));
}

#[cfg(feature = "shacl")]
fn escape_literal(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

impl Backend for QuipuBackend {
    fn snapshot(&self, at: Option<u64>) -> Result<Snapshot> {
        let q = format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <{}> {{ ?s ?p ?o }} }}",
            self.graph_iri
        );
        let result = query_temporal(&self.store, &q, &Self::ctx(at)?)?;
        let mut by_subject: BTreeMap<String, Vec<Fact>> = BTreeMap::new();
        let mut names: BTreeMap<i64, String> = BTreeMap::new();
        for row in result.rows() {
            let (Some(Value::Ref(s)), Some(Value::Ref(p)), Some(o)) =
                (row.get("s"), row.get("p"), row.get("o"))
            else {
                continue;
            };
            let Some(o) = self.obj_of(o)? else { continue };
            let s = match names.get(s) {
                Some(n) => n.clone(),
                None => {
                    let n = self.store.resolve(*s)?;
                    names.insert(*s, n.clone());
                    n
                }
            };
            let p = self.store.resolve(*p)?;
            by_subject.entry(s).or_default().push((p, o));
        }
        let mut snap = Snapshot {
            tx: u64::try_from(self.store.transaction_head()?).unwrap_or(0),
            ..Snapshot::default()
        };
        if let Some(at) = at {
            snap.tx = snap.tx.min(at);
        }
        for facts in by_subject.values() {
            if let Some(seed) = Seed::from_facts(facts) {
                snap.seeds.insert(seed.id.clone(), seed);
            } else if let Some(c) = Comment::from_facts(facts) {
                snap.comments.push(c);
            }
        }
        snap.comments
            .sort_by(|a, b| (&a.seed, a.index).cmp(&(&b.seed, b.index)));
        Ok(snap)
    }

    fn ready_ids(&self, at: Option<u64>) -> Result<Vec<String>> {
        let result = query_temporal(
            &self.store,
            &vocab::ready_query(&self.graph_iri),
            &Self::ctx(at)?,
        )?;
        let mut ids: Vec<String> = result
            .rows()
            .iter()
            .filter_map(|r| match r.get("id") {
                Some(Value::Str(s)) => Some(s.clone()),
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> Result<u64> {
        if !self.writable {
            return Err(SdError::failed(
                "this store was opened read-only; a write needs a writable store",
            ));
        }
        let g = self.store.graph_create(&self.graph_iri)?;
        let rev_attr = self.store.intern(&term::revision())?;

        // 1. Every precondition, before anything is staged.
        let mut current = Vec::with_capacity(batch.seeds.len());
        for w in &batch.seeds {
            let e = self.store.intern(&vocab::item_iri(&w.seed.id))?;
            let facts = self.store.entity_facts_in_graph(e, g)?;
            let rev = facts.iter().find_map(|f| match (&f.value, f.attribute) {
                (Value::Int(n), a) if a == rev_attr => u64::try_from(*n).ok(),
                _ => None,
            });
            match (w.expected_revision, facts.is_empty(), rev) {
                (None, true, _) => {}
                (None, false, _) => {
                    return Err(SdError::conflict(format!(
                        "{} already exists; nothing was written",
                        w.seed.id
                    )))
                }
                (Some(_), true, _) => return Err(SdError::not_found(&w.seed.id)),
                (Some(want), false, got) if got == Some(want) => {}
                (Some(want), false, got) => {
                    return Err(SdError::conflict(format!(
                        "{} changed since it was read (read at revision {want}, now {}); \
                         nothing was written. Re-read it and retry.",
                        w.seed.id,
                        got.map_or_else(|| "unknown".to_string(), |r| r.to_string())
                    )))
                }
            }
            current.push((e, facts));
        }
        for c in &batch.comments {
            if let Some(e) = self.store.lookup(&vocab::comment_iri(&c.seed, c.index))? {
                if !self.store.entity_facts_in_graph(e, g)?.is_empty() {
                    return Err(SdError::conflict(format!(
                        "comment {} on {} already exists; nothing was written",
                        c.index, c.seed
                    )));
                }
            }
        }
        self.validate_post_state(g, batch)?;

        // 2. The diff: retract what changed, assert its replacement.
        let mut datums = Vec::new();
        for (w, (e, facts)) in batch.seeds.iter().zip(&current) {
            let mut desired = Vec::new();
            for (p, o) in w.seed.facts() {
                desired.push((self.store.intern(&p)?, self.value_of(&o)?));
            }
            for f in facts {
                if !desired
                    .iter()
                    .any(|(a, v)| *a == f.attribute && *v == f.value)
                {
                    datums.push(Datum {
                        entity: *e,
                        attribute: f.attribute,
                        value: f.value.clone(),
                        valid_from: f.valid_from.clone(),
                        valid_to: None,
                        op: Op::Retract,
                    });
                }
            }
            for (a, v) in desired {
                if !facts.iter().any(|f| f.attribute == a && f.value == v) {
                    datums.push(Datum {
                        entity: *e,
                        attribute: a,
                        value: v,
                        valid_from: ctx.now.clone(),
                        valid_to: None,
                        op: Op::Assert,
                    });
                }
            }
        }
        for c in &batch.comments {
            let e = self.store.intern(&vocab::comment_iri(&c.seed, c.index))?;
            for (p, o) in c.facts() {
                datums.push(Datum {
                    entity: e,
                    attribute: self.store.intern(&p)?,
                    value: self.value_of(&o)?,
                    valid_from: ctx.now.clone(),
                    valid_to: None,
                    op: Op::Assert,
                });
            }
        }
        if datums.is_empty() {
            return Ok(u64::try_from(self.store.transaction_head()?).unwrap_or(0));
        }
        let tx = self.store.transact_to_graph(
            &datums,
            &ctx.now,
            Some(&ctx.actor),
            Some(&batch.source),
            g,
        )?;
        Ok(u64::try_from(tx).unwrap_or(0))
    }
}

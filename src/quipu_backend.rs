//! [`Backend`] on a quipu store.
//!
//! One project is one named graph. A seed is an entity in that graph; each
//! field is a fact (see [`crate::model::Seed::facts`]). A write retracts the facts that
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
use crate::model::{Fact, Obj, Snapshot};
use crate::validate;
use crate::vocab::{self, term};

/// The name the seeds shapes are registered under in a store, so a pendant
/// exported from it carries them in `shapes.ttl`.
pub const SHAPES_NAME: &str = "seeds";

/// A project stored as a named graph in a quipu store.
pub struct QuipuBackend {
    store: Store,
    graph_iri: String,
    writable: bool,
}

impl QuipuBackend {
    /// Wrap a store opened for writing: registers the project graph and the
    /// seeds shapes (once; a store that already holds these exact shapes is
    /// left alone).
    pub fn writable(store: Store, graph_iri: &str) -> Result<Self> {
        store.graph_create(graph_iri)?;
        let current = store
            .list_shapes()?
            .into_iter()
            .find(|(name, _, _)| name == SHAPES_NAME)
            .map(|(_, turtle, _)| turtle);
        if current.as_deref() != Some(vocab::SHAPES_TURTLE) {
            store.load_shapes(SHAPES_NAME, vocab::SHAPES_TURTLE, "1970-01-01T00:00:00Z")?;
        }
        Ok(Self {
            store,
            graph_iri: graph_iri.to_string(),
            writable: true,
        })
    }

    /// Wrap a store opened read-only. [`Backend::commit`] refuses.
    pub fn read_only(store: Store, graph_iri: &str) -> Self {
        Self {
            store,
            graph_iri: graph_iri.to_string(),
            writable: false,
        }
    }

    /// A fresh in-memory store: the wasm32 backend, and the tests'.
    pub fn in_memory(graph_iri: &str) -> Result<Self> {
        Self::writable(Store::open_in_memory()?, graph_iri)
    }

    /// Whether writes are validated against [`vocab::SHAPES_TURTLE`]. True in
    /// builds with the `shacl` feature (the native CLI); the wasm build has no
    /// SHACL engine and says so here rather than claiming a check. The
    /// structural rules in [`crate::validate`] run in every build.
    pub fn validates(&self) -> bool {
        validate::runs_shapes()
    }

    /// The project graph IRI.
    pub fn graph_iri(&self) -> &str {
        &self.graph_iri
    }

    /// The underlying store (for callers that want quipu's own surfaces, such
    /// as exporting a pendant).
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The underlying store, mutably (for loading a pendant into it).
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
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

    /// The current facts of one entity in the project graph, as seeds facts.
    /// `None` when it holds none.
    fn facts_of(&self, g: i64, iri: &str) -> Result<Option<Vec<Fact>>> {
        let Some(e) = self.store.lookup(iri)? else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for f in self.store.entity_facts_in_graph(e, g)? {
            if let Some(o) = self.obj_of(&f.value)? {
                out.push((self.store.resolve(f.attribute)?, o));
            }
        }
        Ok((!out.is_empty()).then_some(out))
    }

    fn retract_all(&self, g: i64, iri: &str, out: &mut Vec<Datum>) -> Result<()> {
        if let Some(e) = self.store.lookup(iri)? {
            for f in self.store.entity_facts_in_graph(e, g)? {
                out.push(Datum {
                    entity: e,
                    attribute: f.attribute,
                    value: f.value.clone(),
                    valid_from: f.valid_from.clone(),
                    valid_to: None,
                    op: Op::Retract,
                });
            }
        }
        Ok(())
    }

    fn revision_of(&self, g: i64, iri: &str) -> Result<(bool, Option<u64>)> {
        let Some(e) = self.store.lookup(iri)? else {
            return Ok((false, None));
        };
        let rev_attr = self.store.intern(&term::revision())?;
        let facts = self.store.entity_facts_in_graph(e, g)?;
        let rev = facts.iter().find_map(|f| match (&f.value, f.attribute) {
            (Value::Int(n), a) if a == rev_attr => u64::try_from(*n).ok(),
            _ => None,
        });
        Ok((!facts.is_empty(), rev))
    }
}

/// Refuse a write whose seeds are not at the revisions the writer read.
pub(crate) fn check_revision(
    id: &str,
    expected: Option<u64>,
    exists: bool,
    current: Option<u64>,
) -> Result<()> {
    match (expected, exists) {
        (None, false) => Ok(()),
        (None, true) => Err(SdError::conflict(format!(
            "{id} already exists; nothing was written"
        ))),
        (Some(_), false) => Err(SdError::not_found(id)),
        (Some(want), true) if current == Some(want) => Ok(()),
        (Some(want), true) => Err(SdError::conflict(format!(
            "{id} changed since it was read (read at revision {want}, now {}); \
             nothing was written. Re-read it and retry.",
            current.map_or_else(|| "unknown".to_string(), |r| r.to_string())
        ))),
    }
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
        let mut snap = Snapshot::from_subjects(&by_subject);
        snap.tx = u64::try_from(self.store.transaction_head()?).unwrap_or(0);
        if let Some(at) = at {
            snap.tx = snap.tx.min(at);
        }
        Ok(snap)
    }

    fn claims_of(&self, id: &str) -> Result<Vec<(u64, crate::backend::Claims)>> {
        let result = query_temporal(
            &self.store,
            &vocab::claims_query(&self.graph_iri, id),
            &Self::ctx(None)?,
        )?;
        let rows = result
            .rows()
            .iter()
            .map(|r| {
                r.iter()
                    .filter_map(|(k, v)| match v {
                        Value::Str(s) => Some((k.clone(), crate::model::Obj::Str(s.clone()))),
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        Ok(crate::backend::claims_rows(id, rows))
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

        // 1. Every precondition, before anything is staged.
        for w in &batch.seeds {
            let (exists, rev) = self.revision_of(g, &vocab::item_iri(&w.seed.id))?;
            check_revision(&w.seed.id, w.expected_revision, exists, rev)?;
        }
        for (id, want) in &batch.delete_seeds {
            let (exists, rev) = self.revision_of(g, &vocab::item_iri(id))?;
            check_revision(id, Some(*want), exists, rev)?;
        }
        for c in &batch.comments {
            if self
                .facts_of(g, &vocab::comment_iri(&c.seed, c.index))?
                .is_some()
                && !batch
                    .delete_comments
                    .iter()
                    .any(|(s, i)| *s == c.seed && *i == c.index)
            {
                return Err(SdError::conflict(format!(
                    "comment {} on {} already exists; nothing was written",
                    c.index, c.seed
                )));
            }
        }
        validate::validate_batch(batch, &mut |id| self.facts_of(g, &vocab::item_iri(id)))?;

        // 2. The diff: retract what changed, assert its replacement.
        let mut datums = Vec::new();
        for (id, _) in &batch.delete_seeds {
            self.retract_all(g, &vocab::item_iri(id), &mut datums)?;
        }
        for (seed, index) in &batch.delete_comments {
            // A comment deleted and re-added in the same batch is diffed below.
            if !batch
                .comments
                .iter()
                .any(|c| c.seed == *seed && c.index == *index)
            {
                self.retract_all(g, &vocab::comment_iri(seed, *index), &mut datums)?;
            }
        }
        let mut entities: Vec<(String, Vec<Fact>)> = batch
            .seeds
            .iter()
            .map(|w| (vocab::item_iri(&w.seed.id), w.seed.facts()))
            .collect();
        entities.extend(
            batch
                .comments
                .iter()
                .map(|c| (vocab::comment_iri(&c.seed, c.index), c.facts())),
        );
        for (iri, new_facts) in &entities {
            let e = self.store.intern(iri)?;
            let facts = self.store.entity_facts_in_graph(e, g)?;
            let mut desired = Vec::new();
            for (p, o) in new_facts {
                desired.push((self.store.intern(p)?, self.value_of(o)?));
            }
            for f in &facts {
                if !desired
                    .iter()
                    .any(|(a, v)| *a == f.attribute && *v == f.value)
                {
                    datums.push(Datum {
                        entity: e,
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
                        entity: e,
                        attribute: a,
                        value: v,
                        valid_from: ctx.now.clone(),
                        valid_to: None,
                        op: Op::Assert,
                    });
                }
            }
        }
        if datums.is_empty() {
            return Ok(u64::try_from(self.store.transaction_head()?).unwrap_or(0));
        }
        // The same provenance record remote mode writes, in the same
        // transaction: who, why, when, which seeds, and any attribution claims.
        let pg = self
            .store
            .graph_create(&crate::vocab::provenance_graph(&self.graph_iri))?;
        let write_iri = format!(
            "urn:seeds:write:{}",
            &crate::pendant::sha256(
                format!("{}\n{}\n{}\n{datums:?}", ctx.now, ctx.actor, batch.source).as_bytes()
            )[7..31]
        );
        let mut prov = Vec::new();
        for (iri, facts) in crate::backend::write_record(&write_iri, batch, ctx) {
            let e = self.store.intern(&iri)?;
            for (p, o) in &facts {
                prov.push(Datum {
                    entity: e,
                    attribute: self.store.intern(p)?,
                    value: self.value_of(o)?,
                    valid_from: ctx.now.clone(),
                    valid_to: None,
                    op: Op::Assert,
                });
            }
        }
        self.store.transact_graph_batches(
            // Provenance first, so the head afterwards is the data write's
            // transaction: the tx sd reports and `--at` reads.
            &[(pg, prov), (g, datums)],
            &ctx.now,
            Some(&ctx.actor),
            Some(&batch.source),
        )?;
        Ok(u64::try_from(self.store.transaction_head()?).unwrap_or(0))
    }
}

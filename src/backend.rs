//! The storage seam.
//!
//! The core (verbs, the ready computation, JSON output) talks to storage only
//! through [`Backend`]. The one implementation today is
//! [`crate::quipu_backend::QuipuBackend`], which runs on a quipu store: a local
//! file in the native `sd` CLI, an in-memory store on wasm32. A backend never
//! reads a clock or the filesystem on its own behalf; time arrives in [`Ctx`].

use crate::error::Result;
use std::collections::BTreeSet;

use crate::model::{Comment, Fact, Obj, Seed, Snapshot};

/// Who is acting, and when. Injected by the caller: the core never reads a
/// clock, so it runs unchanged on wasm32.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// The current instant, ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
    pub now: String,
    /// The actor, recorded on every transaction and used by `--claim`.
    pub actor: String,
    /// The id prefix for new seeds.
    pub prefix: String,
    /// What the writer SAYS it is (br's tier-1 attribution). Self-asserted and
    /// unverified: recorded as `declared`, never as the actor.
    pub claims: Claims,
}

/// Self-asserted attribution for a write: `--agent-name`, `--harness`,
/// `--model`. Nothing checks it, so it is stored as a claim beside the write's
/// actor and never stands in for it (or for a signed principal).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// The agent's name, as it gives it.
    pub agent_name: Option<String>,
    /// The harness it says it runs in.
    pub harness: Option<String>,
    /// The model it says it is.
    pub model: Option<String>,
    /// The session it says it writes from (br's `--session`).
    pub session: Option<String>,
}

impl Claims {
    /// Whether no claim was made.
    pub fn is_empty(&self) -> bool {
        self.agent_name.is_none()
            && self.harness.is_none()
            && self.model.is_none()
            && self.session.is_none()
    }
}

/// The provenance record for one write, identical in every storage mode: a
/// `seeds:Write` node (the actor, source, time and every seed written) and,
/// when the writer made claims, a `seeds:AttributionClaim` node tagged
/// `aegis:sourceKind "declared"`. Written to [`crate::vocab::provenance_graph`],
/// which snapshots and exports never read.
pub fn write_record(write_iri: &str, batch: &WriteBatch, ctx: &Ctx) -> Vec<(String, Vec<Fact>)> {
    use crate::vocab::{self, AEGIS, RDF_TYPE};
    let s = |t: &str| Obj::Str(t.to_string());
    let mut w: Vec<Fact> = vec![
        (RDF_TYPE.into(), Obj::Iri(vocab::seeds("Write"))),
        (vocab::seeds("actor"), s(&ctx.actor)),
        (vocab::seeds("source"), s(&batch.source)),
        (vocab::seeds("at"), s(&ctx.now)),
    ];
    let written = batch
        .seeds
        .iter()
        .map(|x| x.seed.id.clone())
        .chain(batch.delete_seeds.iter().map(|(id, _)| id.clone()))
        .chain(batch.comments.iter().map(|c| c.seed.clone()));
    for id in written {
        let f = (vocab::seeds("wrote"), Obj::Iri(vocab::item_iri(&id)));
        if !w.contains(&f) {
            w.push(f);
        }
    }
    // Each seed version this write produced, as `<id>@<revision>`, so a claim
    // is matched to exactly the version it wrote (a timestamp is not unique:
    // several writes land in one second).
    for x in &batch.seeds {
        w.push((
            vocab::seeds("version"),
            s(&format!("{}@{}", x.seed.id, x.seed.revision)),
        ));
    }
    let mut out = Vec::new();
    if !ctx.claims.is_empty() {
        let claim_iri = format!("{write_iri}-claim");
        w.push((vocab::seeds("claimed"), Obj::Iri(claim_iri.clone())));
        let mut c: Vec<Fact> = vec![
            (RDF_TYPE.into(), Obj::Iri(vocab::seeds("AttributionClaim"))),
            (format!("{AEGIS}sourceKind"), s("declared")),
        ];
        for (p, v) in [
            ("agentName", &ctx.claims.agent_name),
            ("harness", &ctx.claims.harness),
            ("model", &ctx.claims.model),
            ("session", &ctx.claims.session),
        ] {
            if let Some(v) = v {
                c.push((vocab::seeds(p), s(v)));
            }
        }
        out.push((claim_iri, c));
    }
    out.insert(0, (write_iri.to_string(), w));
    out
}

/// What a listing's filters say about which seeds can match, in the terms a
/// backend can answer from an index ([`Backend::snapshot_where`]). Every
/// field narrows; an empty query matches every seed. Values are compared the
/// way [`Seed::from_facts`] reads them, so a backend can push each down as a
/// bound pattern; [`SeedQuery::matches`] is the exact meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedQuery {
    /// Only seeds whose status is one of these. Statuses outside
    /// [`crate::model::STATUSES`] are matched too when `other_statuses`.
    pub statuses: Option<BTreeSet<String>>,
    /// With `statuses`: also seeds whose status is none of
    /// [`crate::model::STATUSES`] (a value some other writer stored).
    pub other_statuses: bool,
    /// Only this type.
    pub issue_type: Option<String>,
    /// Only this assignee.
    pub assignee: Option<String>,
    /// Only seeds carrying every one of these labels.
    pub labels: BTreeSet<String>,
    /// Only this priority.
    pub priority: Option<u8>,
    /// Only children of this seed.
    pub parent: Option<String>,
}

impl SeedQuery {
    /// Whether the query constrains nothing (every seed matches).
    pub fn is_unconstrained(&self) -> bool {
        self.statuses.is_none()
            && self.issue_type.is_none()
            && self.assignee.is_none()
            && self.labels.is_empty()
            && self.priority.is_none()
            && self.parent.is_none()
    }

    /// Whether `s` matches: the exact meaning a backend's answer must cover.
    pub fn matches(&self, s: &Seed) -> bool {
        let known = crate::model::STATUSES.contains(&s.status.as_str());
        self.statuses
            .as_ref()
            .is_none_or(|w| w.contains(&s.status) || (self.other_statuses && !known))
            && self.issue_type.as_ref().is_none_or(|t| &s.issue_type == t)
            && self
                .assignee
                .as_ref()
                .is_none_or(|a| s.assignee.as_ref() == Some(a))
            && self.labels.is_subset(&s.labels)
            && self.priority.is_none_or(|p| s.priority == p)
            && self
                .parent
                .as_ref()
                .is_none_or(|p| s.parent.as_ref() == Some(p))
    }
}

/// Add `more` to `snap`: its seeds (a later read of a seed replaces the
/// earlier one; both are current) and the comments `snap` does not hold yet,
/// so scoped reads of overlapping items never duplicate a comment.
pub fn merge_snapshots(snap: &mut Snapshot, more: Snapshot) {
    snap.seeds.extend(more.seeds);
    let have: BTreeSet<(String, u64)> = snap
        .comments
        .iter()
        .map(|c| (c.seed.clone(), c.index))
        .collect();
    snap.comments.extend(
        more.comments
            .into_iter()
            .filter(|c| !have.contains(&(c.seed.clone(), c.index))),
    );
    snap.comments
        .sort_by(|a, b| (&a.seed, a.index).cmp(&(&b.seed, b.index)));
}

/// One seed to write, with the revision the writer read it at.
#[derive(Debug, Clone)]
pub struct SeedWrite {
    /// The complete post-state of the seed.
    pub seed: Seed,
    /// The revision the writer based this on: `None` for a new seed (which
    /// must not exist yet), `Some(r)` for an update (which must still be at
    /// revision `r`). A mismatch is a [`crate::error::ErrorKind::Conflict`]
    /// and nothing in the batch is written.
    pub expected_revision: Option<u64>,
}

/// Everything one verb writes. A batch is one transaction: all of it lands, or
/// none of it does. (A sync too large for the server is pushed as several
/// batches, each its own transaction; see [`crate::sync_batch::split_for_cap`].)
#[derive(Debug, Clone, Default)]
pub struct WriteBatch {
    /// Seeds to create or replace.
    pub seeds: Vec<SeedWrite>,
    /// New comments (a comment is never edited).
    pub comments: Vec<Comment>,
    /// Seeds to remove entirely, each with the revision the writer read. Only
    /// import and sync remove seeds (to match a ledger that no longer holds
    /// them); no verb deletes.
    pub delete_seeds: Vec<(String, u64)>,
    /// Comments to remove, as (seed id, index). Only sync uses this, to
    /// renumber a comment that collided with one written elsewhere.
    pub delete_comments: Vec<(String, u64)>,
    /// The transaction source tag, e.g. `seeds:update`.
    pub source: String,
}

impl WriteBatch {
    /// A batch that changes nothing.
    pub fn is_empty(&self) -> bool {
        self.seeds.is_empty()
            && self.comments.is_empty()
            && self.delete_seeds.is_empty()
            && self.delete_comments.is_empty()
    }
}

/// Storage for one project.
pub trait Backend {
    /// Every seed and comment, as of transaction `at` (current state when
    /// `None`).
    fn snapshot(&self, at: Option<u64>) -> Result<Snapshot>;

    /// Current named items and their comments, for edits that need no graph-wide
    /// context. Backends without an indexed read retain the full-snapshot path.
    /// Graph-aware edits must load their complete validation context separately;
    /// this method alone cannot prove that a claim is unblocked or an edge acyclic.
    fn snapshot_items(&self, _ids: &[String]) -> Result<Snapshot> {
        self.snapshot(None)
    }

    /// Whether the scoped reads below are answered from an index, so a command
    /// that answers from a few items should ask for those rather than take a
    /// whole snapshot. `false` (the default) keeps every command on
    /// [`Backend::snapshot`]: a local store's snapshot is fast. A remote
    /// store's is not (a 28k-seed board: ~13 s), so it says `true`
    /// (aegis-aane52 S2).
    fn scoped_reads(&self) -> bool {
        false
    }

    /// The current state of the named seeds, WITHOUT the guarantee that
    /// their comments are included (a listing never prints them). Like every
    /// scoped read it may return more than asked (the default returns
    /// [`Backend::snapshot_items`]); callers only look up what they asked for.
    fn snapshot_seeds(&self, ids: &[String]) -> Result<Snapshot> {
        self.snapshot_items(ids)
    }

    /// The current state of every seed that may match `q`: a SUPERSET of the
    /// seeds that do (the caller applies its exact filter again), never a
    /// subset. Comments are not guaranteed. The default is the whole
    /// snapshot, the largest superset.
    fn snapshot_where(&self, _q: &SeedQuery) -> Result<Snapshot> {
        self.snapshot(None)
    }

    /// The current state of every seed that declares a dependency of any type
    /// (blocks, parent-child, related, discovered-from) on one of `ids`, so
    /// [`Snapshot::dependents`] answers for those ids from the result. A
    /// superset again; comments are not guaranteed. The default is the whole
    /// snapshot.
    fn snapshot_dependents(&self, _ids: &[String]) -> Result<Snapshot> {
        self.snapshot(None)
    }

    /// The ids the ready definition ([`crate::vocab::ready_query`]) selects, as
    /// of `at`. Filters and the defer date are applied by the caller.
    fn ready_ids(&self, at: Option<u64>) -> Result<Vec<String>>;

    /// Apply a batch atomically, checking every [`SeedWrite::expected_revision`]
    /// first. Returns the transaction id, or the current head when the batch
    /// changed nothing.
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> Result<u64>;

    /// The attribution claims recorded on writes that produced a version of
    /// seed `id`, as `(revision, claims)` ([`crate::vocab::claims_query`]).
    fn claims_of(&self, id: &str) -> Result<Vec<(u64, Claims)>>;

    /// The largest write this backend accepts, in request bytes, or `None`
    /// when it has no limit (a local store). A write over the limit is
    /// refused before anything is sent, with a message starting
    /// [`TOO_LARGE`], so the caller knows nothing landed and can split it.
    fn max_write_bytes(&self) -> Option<usize> {
        None
    }

    /// The most guard clauses (`FILTER NOT EXISTS` and `UNION` branches, see
    /// [`crate::sync_batch::clause_count`]) one write may nest, or `None`
    /// when unlimited. Over it, the write is refused unsent like
    /// [`Backend::max_write_bytes`].
    fn max_write_clauses(&self) -> Option<usize> {
        None
    }

    /// Whether the project (or its ephemeral graph) holds ANY data in
    /// seeds' OLD vocabulary ([`crate::vocab::legacy_presence_query`]): a
    /// store this build would otherwise read as empty. One bounded existence
    /// check per graph, so every command can afford to ask. Fails closed: an
    /// answer that is not a clear yes or no is an error.
    fn legacy_present(&self) -> Result<bool> {
        Ok(false)
    }

    /// How many subjects hold the old vocabulary
    /// ([`crate::vocab::legacy_count_query`]). Exact and slower; `sd doctor`
    /// reports it.
    fn legacy_items(&self) -> Result<u64> {
        Ok(0)
    }
}

/// The old-vocabulary count from the rows of
/// [`crate::vocab::legacy_count_query`] on `graph`. Fails CLOSED: anything but
/// exactly one row with one non-negative integer `n` (no row, several rows, a
/// missing or non-numeric count) is an error, never a clean zero, because a
/// zero read from a broken answer is the silent-empty failure this check
/// exists to prevent.
pub fn legacy_count(graph: &str, rows: &[Option<&Obj>]) -> Result<u64> {
    let bad = |why: &str| {
        crate::error::SdError::failed(format!(
            "the old-vocabulary check on graph {graph} could not be completed: {why}"
        ))
    };
    let [only] = rows else {
        return Err(bad(&format!("expected one count row, got {}", rows.len())));
    };
    match only {
        Some(Obj::Int(n)) => u64::try_from(*n).map_err(|_| bad(&format!("negative count {n}"))),
        Some(Obj::Str(s) | Obj::Typed { lexical: s, .. }) => s
            .parse()
            .map_err(|_| bad(&format!("the count {s:?} is not a number"))),
        Some(other) => Err(bad(&format!("the count is not a literal: {other:?}"))),
        None => Err(bad("the answer has no count")),
    }
}

/// How a refusal for exceeding [`Backend::max_write_bytes`] begins. The kind
/// is [`crate::error::ErrorKind::Refused`]: the write was never sent.
pub const TOO_LARGE: &str = "write too large for the server";

/// Whether `e` is a [`TOO_LARGE`] refusal: definite, nothing was written.
pub fn is_too_large(e: &crate::error::SdError) -> bool {
    e.kind == crate::error::ErrorKind::Refused && e.message.starts_with(TOO_LARGE)
}

/// Rows of [`crate::vocab::claims_query`] for seed `id` as `(revision,
/// claims)`, by revision.
pub fn claims_rows(
    id: &str,
    rows: Vec<std::collections::BTreeMap<String, Obj>>,
) -> Vec<(u64, Claims)> {
    let get = |r: &std::collections::BTreeMap<String, Obj>, k: &str| match r.get(k) {
        Some(Obj::Str(v)) => Some(v.clone()),
        _ => None,
    };
    let mut out: Vec<(u64, Claims)> = rows
        .iter()
        .filter_map(|r| {
            let rev = get(r, "version")?
                .strip_prefix(&format!("{id}@"))?
                .parse()
                .ok()?;
            Some((
                rev,
                Claims {
                    agent_name: get(r, "agent"),
                    harness: get(r, "harness"),
                    model: get(r, "model"),
                    session: get(r, "session"),
                },
            ))
        })
        .collect();
    out.sort_by_key(|a| a.0);
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // seeds#96 follow-up (sattler): the old-vocabulary count fails CLOSED.
    // Reading any of these as 0 would report a broken answer as a clean,
    // new-vocabulary ledger.
    #[test]
    fn the_old_vocabulary_count_fails_closed() {
        let g = "urn:g";
        let int = |n| Obj::Int(n);
        let s = |v: &str| Obj::Str(v.into());
        assert_eq!(legacy_count(g, &[Some(&int(0))]).unwrap(), 0);
        assert_eq!(legacy_count(g, &[Some(&int(3))]).unwrap(), 3);
        assert_eq!(legacy_count(g, &[Some(&s("2"))]).unwrap(), 2);
        let typed = Obj::Typed {
            lexical: "4".into(),
            datatype: crate::model::XSD_INTEGER.into(),
        };
        assert_eq!(legacy_count(g, &[Some(&typed)]).unwrap(), 4);
        let iri = Obj::Iri("urn:x".into());
        for (what, rows) in [
            ("no row", vec![]),
            ("no count in the row", vec![None]),
            ("two rows", vec![Some(&int(0)), Some(&int(0))]),
            ("a negative count", vec![Some(&int(-1))]),
            ("a non-numeric count", vec![Some(&s("lots"))]),
            ("an empty count", vec![Some(&s(""))]),
            ("an IRI for a count", vec![Some(&iri)]),
        ] {
            let e = legacy_count(g, &rows).expect_err(what);
            assert!(
                e.message.contains("could not be completed") && e.message.contains(g),
                "{what}: {}",
                e.message
            );
        }
    }
}

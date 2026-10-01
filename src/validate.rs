//! Checking a write, or a whole ledger, before it lands.
//!
//! Two layers, both backend-independent so the local store, the remote store
//! and pendant import share one definition:
//!
//! - **the shapes** ([`crate::vocab::SHAPES_TURTLE`]: camayoc's WorkItem shape
//!   plus seeds'), run by quipu's SHACL engine in builds with the `shacl`
//!   feature (the native CLI). The wasm build has no SHACL engine.
//! - **structural rules that need no engine**, run everywhere: a `blocks` edge
//!   must point at a seed that exists, and a single-valued field must hold one
//!   value (the check that catches a git merge that kept both sides of a
//!   status change).

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::WriteBatch;
use crate::error::{Result, SdError};
use crate::model::{Fact, Obj};
use crate::vocab::{self, term};

/// Predicates a seed holds at most one value of.
pub fn functional_predicates() -> Vec<String> {
    vec![
        term::identifier(),
        vocab::RDFS_LABEL.to_string(),
        term::status(),
        term::priority(),
        term::issue_type(),
        term::created_at(),
        term::updated_at(),
        term::revision(),
        term::description(),
        term::notes(),
        term::design(),
        term::agent_context(),
        term::acceptance_criteria(),
        term::external_ref(),
        term::due_at(),
        term::estimated_minutes(),
        term::owner(),
        term::closed_at(),
        term::close_reason(),
        term::defer_until(),
        term::created_by(),
        term::assigned_to(),
        term::child_of(),
        term::outcome(),
        term::source_kind(),
        term::workflow_run(),
        term::comment_on(),
        term::comment_index(),
        term::author(),
        term::text(),
    ]
}

/// Render facts about `subject` as N-Triples.
pub fn push_ntriples(out: &mut String, subject: &str, facts: &[Fact]) {
    for (p, o) in facts {
        let obj = match o {
            Obj::Iri(i) => format!("<{i}>"),
            Obj::Str(v) => format!("\"{}\"", escape_literal(v)),
            Obj::Int(n) => format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            Obj::Lang { lexical, lang } => format!("\"{}\"@{lang}", escape_literal(lexical)),
            Obj::Typed { lexical, datatype } => {
                format!("\"{}\"^^<{datatype}>", escape_literal(lexical))
            }
        };
        out.push_str(&format!("<{subject}> <{p}> {obj} .\n"));
    }
}

/// Escape a string for an N-Triples or SPARQL string literal.
pub fn escape_literal(v: &str) -> String {
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

/// Problems with single-valued predicates, per subject.
pub fn functional_problems(by_subject: &BTreeMap<String, Vec<Fact>>) -> Vec<String> {
    let functional: BTreeSet<String> = functional_predicates().into_iter().collect();
    let mut problems = Vec::new();
    for (s, facts) in by_subject {
        let mut seen: BTreeMap<&str, Vec<&Obj>> = BTreeMap::new();
        for (p, o) in facts {
            if functional.contains(p) {
                seen.entry(p.as_str()).or_default().push(o);
            }
        }
        for (p, vals) in seen {
            if vals.len() > 1 {
                let shown: Vec<String> = vals
                    .iter()
                    .map(|o| match o {
                        Obj::Iri(i) => format!("<{i}>"),
                        Obj::Str(v) => format!("{v:?}"),
                        Obj::Int(n) => n.to_string(),
                        Obj::Lang { lexical, lang } => format!("{lexical:?}@{lang}"),
                        Obj::Typed { lexical, datatype } => format!("{lexical:?}^^<{datatype}>"),
                    })
                    .collect();
                problems.push(format!(
                    "{} has {} values for {}: {}",
                    vocab::item_id(s).unwrap_or_else(|| s.clone()),
                    vals.len(),
                    p.rsplit(['/', '#']).next().unwrap_or(p),
                    shown.join(" | ")
                ));
            }
        }
    }
    problems
}

/// Validate the post-state of a batch. `current_facts(id)` returns the current
/// facts of a seed OUTSIDE the batch (`None` when it does not exist); it is
/// asked only for the targets of `blocks` edges.
pub fn validate_batch(
    batch: &WriteBatch,
    current_facts: &mut dyn FnMut(&str) -> Result<Option<Vec<Fact>>>,
) -> Result<()> {
    let written: BTreeSet<&str> = batch.seeds.iter().map(|w| w.seed.id.as_str()).collect();
    let deleted: BTreeSet<&str> = batch
        .delete_seeds
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    let mut nt = String::new();
    let mut targets: BTreeMap<String, Vec<Fact>> = BTreeMap::new();
    let mut dangling = Vec::new();
    for w in &batch.seeds {
        push_ntriples(&mut nt, &vocab::item_iri(&w.seed.id), &w.seed.facts());
        for b in &w.seed.blocked_on {
            if written.contains(b.as_str()) || targets.contains_key(b) {
                continue;
            }
            match current_facts(b)? {
                Some(f) if !deleted.contains(b.as_str()) => {
                    targets.insert(b.clone(), f);
                }
                _ => dangling.push(format!(
                    "{} is blocked on {b}, which is not a seed here",
                    w.seed.id
                )),
            }
        }
    }
    if !dangling.is_empty() {
        return Err(SdError::refused(format!(
            "the write does not conform; nothing was written:\n  {}",
            dangling.join("\n  ")
        )));
    }
    // Targets are context, not writes: they are here so the batch's new
    // blockedOn edges resolve to WorkItems. Their own edges to seeds outside
    // this graph were checked when they were written, and keeping them would
    // make the shapes refuse the target for pointing at a seed not loaded.
    let loaded: BTreeSet<&str> = written
        .iter()
        .copied()
        .chain(targets.keys().map(String::as_str))
        .collect();
    for (id, facts) in &targets {
        let kept: Vec<Fact> = facts
            .iter()
            .filter(|(_, o)| match o {
                Obj::Iri(t) => vocab::item_id(t).is_none_or(|t| loaded.contains(t.as_str())),
                _ => true,
            })
            .cloned()
            .collect();
        push_ntriples(&mut nt, &vocab::item_iri(id), &kept);
    }
    shapes(&nt)
}

/// Validate a whole ledger given as N-Triples (a pendant's `export.nt`, or a
/// merge result). Returns every problem found, empty when it conforms.
pub fn validate_ledger(nt: &str, by_subject: &BTreeMap<String, Vec<Fact>>) -> Vec<String> {
    let mut problems = functional_problems(by_subject);
    // Dangling blocks edges.
    let items: BTreeSet<&str> = by_subject
        .iter()
        .filter(|(_, f)| {
            f.iter()
                .any(|(p, o)| p == vocab::RDF_TYPE && *o == Obj::Iri(term::work_item()))
        })
        .map(|(s, _)| s.as_str())
        .collect();
    for (s, facts) in by_subject {
        for (p, o) in facts {
            if *p == term::blocked_on() {
                if let Obj::Iri(t) = o {
                    if !items.contains(t.as_str()) {
                        problems.push(format!(
                            "{} is blocked on {}, which is not in the ledger",
                            vocab::item_id(s).unwrap_or_else(|| s.clone()),
                            vocab::item_id(t).unwrap_or_else(|| t.clone())
                        ));
                    }
                }
            }
        }
    }
    if let Err(e) = shapes(nt) {
        problems.push(e.message);
    }
    problems
}

#[cfg(feature = "shacl")]
fn shapes(nt: &str) -> Result<()> {
    let feedback = quipu::validate_shapes(vocab::SHAPES_TURTLE, nt)?;
    if feedback.conforms {
        return Ok(());
    }
    let reasons: Vec<String> = feedback
        .results
        .iter()
        .map(|r| {
            format!(
                "{} {}: {}",
                vocab::item_id(&r.focus_node).unwrap_or_else(|| r.focus_node.clone()),
                r.path
                    .as_deref()
                    .map(|p| p
                        .rsplit(['/', '#'])
                        .next()
                        .unwrap_or(p)
                        .trim_end_matches('>'))
                    .unwrap_or_default(),
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
fn shapes(_nt: &str) -> Result<()> {
    Ok(())
}

/// Whether this build runs the SHACL shapes.
pub fn runs_shapes() -> bool {
    cfg!(feature = "shacl")
}

/// Refuse an edge FROM a shared (non-ephemeral) seed TO an ephemeral one, of
/// any dependency type. The edge would be shared while its target is not, so
/// the ledger would carry an id no clone can resolve: br accepts such an
/// edge, and its own `sync --import-only` then refuses the export (rc 6,
/// observed; aegis-lq3eqt). Lead ruling W3K75D13-EPH-CROSSDEP on
/// aegis-w3k75d.13. Edges from an ephemeral seed, to anything, are fine:
/// they live in the ephemeral graph, which is never shared.
pub fn no_shared_edge_to_ephemeral(
    batch: &WriteBatch,
    is_ephemeral: &mut dyn FnMut(&str) -> Result<bool>,
) -> Result<()> {
    let in_batch: BTreeMap<&str, bool> = batch
        .seeds
        .iter()
        .map(|w| (w.seed.id.as_str(), w.seed.ephemeral))
        .collect();
    let mut bad = Vec::new();
    for w in batch.seeds.iter().filter(|w| !w.seed.ephemeral) {
        for (target, ty) in w.seed.dependencies() {
            let eph = match in_batch.get(target.as_str()) {
                Some(e) => *e,
                None => is_ephemeral(&target)?,
            };
            if eph {
                bad.push(format!("{} -> {target} ({ty})", w.seed.id));
            }
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    Err(SdError::usage(format!(
        "a shared seed cannot depend on an ephemeral one: the edge would be shared and \
         its target would not, so the ledger would name a seed no clone can resolve. \
         Nothing was written:\n  {}",
        bad.join("\n  ")
    )))
}

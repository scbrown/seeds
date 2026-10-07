//! Checking a write, or a whole ledger, before it lands.
//!
//! Two layers, both backend-independent so the local store, the remote store
//! and pendant import share one definition:
//!
//! - **the shapes** ([`crate::vocab::SHAPES_TURTLE`]: the whole seeds profile
//!   over `schema:Action`, `schema:Comment` and the calendar aka), run by
//!   quipu's SHACL engine in builds with the `shacl` feature (the native
//!   CLI). The wasm build has no SHACL engine.
//! - **structural rules that need no engine**, run everywhere: a `blocks` edge
//!   must point at a seed that exists, a single-valued field must hold one
//!   value (the check that catches a git merge that kept both sides of a
//!   status change), and every time must already be a valid `xsd:date` or
//!   `xsd:dateTime` lexical form (nothing is coerced).

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::WriteBatch;
use crate::error::{Result, SdError};
use crate::model::{Fact, Obj};
use crate::vocab::{self, term};

/// Predicates a seed holds at most one value of.
pub fn functional_predicates() -> Vec<String> {
    vec![
        term::identifier(),
        term::name(),
        vocab::RDFS_LABEL.to_string(),
        term::status(),
        term::action_status(),
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
    time_problems(batch)?;
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
    // A comment's shape needs the seed it belongs to loaded, as a
    // `schema:Action`. A comment on a seed that is not here is refused.
    for c in &batch.comments {
        push_ntriples(&mut nt, &vocab::comment_iri(&c.seed, c.index), &c.facts());
        if written.contains(c.seed.as_str()) || targets.contains_key(&c.seed) {
            continue;
        }
        match current_facts(&c.seed)? {
            Some(f) if !deleted.contains(c.seed.as_str()) => {
                targets.insert(c.seed.clone(), f);
            }
            _ => dangling.push(format!(
                "comment {} is on {}, which is not a seed here",
                c.index, c.seed
            )),
        }
    }
    if !dangling.is_empty() {
        return Err(SdError::refused(format!(
            "the write does not conform; nothing was written:\n  {}",
            dangling.join("\n  ")
        )));
    }
    // Targets are context, not writes: they are here so the batch's new
    // blockedOn edges and comments resolve to seeds. Their own edges to seeds outside
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

/// Refuse a batch carrying a time that is not already a valid `xsd:date` /
/// `xsd:dateTime` lexical form, naming every bad field and value (a usage
/// error: the value is the caller's, and nothing is coerced).
fn time_problems(batch: &WriteBatch) -> Result<()> {
    let bad: Vec<String> = batch
        .seeds
        .iter()
        .flat_map(|w| w.seed.time_problems())
        .chain(batch.comments.iter().flat_map(|c| c.time_problems()))
        .collect();
    if bad.is_empty() {
        return Ok(());
    }
    Err(SdError::usage(format!(
        "{} time value(s) are not valid xsd:date/xsd:dateTime; nothing was written:\n  {}",
        bad.len(),
        bad.join("\n  ")
    )))
}

/// Validate a whole ledger given as N-Triples (a pendant's `export.nt`, or a
/// merge result). Returns every problem found, empty when it conforms.
pub fn validate_ledger(nt: &str, by_subject: &BTreeMap<String, Vec<Fact>>) -> Vec<String> {
    let mut problems = functional_problems(by_subject);
    for facts in by_subject.values() {
        if let Some(s) = crate::model::Seed::from_facts(facts) {
            problems.extend(s.time_problems());
        } else if let Some(c) = crate::model::Comment::from_facts(facts) {
            problems.extend(c.time_problems());
        }
    }
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
        "the write does not conform to the seeds shapes; nothing was written:\n  {}",
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

/// The profile's negative controls, run through sd's own SHACL gate (aegis-bqgdr3
/// table v1.1). Each mutant of a conforming board must be refused, and the
/// probes prove the engine evaluates the constraint kinds the profile leans on:
/// a SHACL engine that silently skipped `sh:xone` would pass a status that
/// disagrees with its actionStatus.
#[cfg(all(test, feature = "shacl"))]
mod shape_tests {
    use super::*;
    use crate::model::{Comment, Seed};

    const XSD_DT: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

    fn seed(id: &str, status: &str) -> Seed {
        Seed {
            id: id.into(),
            title: format!("title {id}"),
            status: status.into(),
            priority: 2,
            issue_type: "task".into(),
            created_at: "2026-10-01T09:00:00.123456789Z".into(),
            updated_at: "2026-10-02T09:00:00Z".into(),
            created_by: Some("ian".into()),
            owner: Some("ian@example.org".into()),
            assignee: Some("wu".into()),
            due_at: Some("2026-10-09".into()),
            defer_until: Some("2026-10-03T14:00:00Z".into()),
            estimated_minutes: Some(90),
            revision: 3,
            ..Seed::default()
        }
    }

    /// A conforming board: an open seed blocked on a closed one, plus a
    /// comment, as (subject IRI, facts).
    fn board() -> Vec<(String, Vec<Fact>)> {
        let mut blocker = seed("sd-b", "closed");
        blocker.closed_at = Some("2026-10-02T09:00:00Z".into());
        blocker.outcome = Some("abandoned".into());
        let mut open = seed("sd-a", "open");
        open.blocked_on.insert("sd-b".into());
        open.labels.insert("upstream".into());
        let c = Comment {
            seed: "sd-a".into(),
            index: 1,
            author: "ian".into(),
            text: "first".into(),
            created_at: "2026-10-02T10:00:00Z".into(),
            extra: Default::default(),
        };
        vec![
            (vocab::item_iri("sd-a"), open.facts()),
            (vocab::item_iri("sd-b"), blocker.facts()),
            (vocab::comment_iri("sd-a", 1), c.facts()),
        ]
    }

    fn nt(b: &[(String, Vec<Fact>)]) -> String {
        let mut out = String::new();
        for (s, f) in b {
            push_ntriples(&mut out, s, f);
        }
        out
    }

    /// Mutate the facts of subject `i` and return the SHACL verdict.
    fn mutant(i: usize, f: impl FnOnce(&mut Vec<Fact>)) -> Result<()> {
        let mut b = board();
        f(&mut b[i].1);
        shapes(&nt(&b))
    }

    fn refused(r: Result<()>, what: &str, needle: &str) {
        let e = r.expect_err(&format!("{what}: the shapes must refuse it"));
        assert!(
            e.message.contains(needle),
            "{what}: refused, but not for {needle:?}: {}",
            e.message
        );
    }

    #[test]
    fn the_positive_control_conforms() {
        shapes(&nt(&board())).expect("the conforming board must pass");
        // And through the whole-ledger gate, structural rules included.
        let by: BTreeMap<String, Vec<Fact>> = board().into_iter().collect();
        assert_eq!(validate_ledger(&nt(&board()), &by), Vec::<String>::new());
    }

    #[test]
    fn no_source_kind_is_refused() {
        refused(
            mutant(0, |f| f.retain(|(p, _)| *p != term::source_kind())),
            "no sourceKind",
            "sourceKind",
        );
    }

    #[test]
    fn two_versions_on_the_cas_field_are_refused() {
        refused(
            mutant(0, |f| f.push((term::revision(), Obj::Int(99)))),
            "two schema:version values",
            "schema:version",
        );
    }

    #[test]
    fn status_and_action_status_disagreeing_is_refused() {
        // Only sh:xone catches this: ActiveActionStatus is a legal value.
        refused(
            mutant(1, |f| {
                f.retain(|(p, _)| *p != term::action_status());
                f.push((
                    term::action_status(),
                    Obj::Iri(vocab::schema("ActiveActionStatus")),
                ));
            }),
            "status/actionStatus disagree",
            "Xone",
        );
    }

    #[test]
    fn blocked_on_a_non_seed_is_refused() {
        refused(
            mutant(0, |f| {
                f.push((term::blocked_on(), Obj::Iri(vocab::item_iri("nope"))))
            }),
            "blockedOn a non-seed",
            "blockedOn",
        );
    }

    #[test]
    fn an_untyped_date_is_refused() {
        refused(
            mutant(0, |f| {
                f.retain(|(p, _)| *p != term::created_at());
                f.push((term::created_at(), Obj::Str("2026-10-01T09:00:00Z".into())));
            }),
            "untyped dateCreated",
            "dateCreated",
        );
        // sh:or: a defer that is neither xsd:date nor xsd:dateTime.
        refused(
            mutant(0, |f| {
                f.retain(|(p, _)| *p != term::defer_until());
                f.push((term::defer_until(), Obj::Str("2026-10-03".into())));
            }),
            "untyped scheduledTime",
            "scheduledTime",
        );
    }

    #[test]
    fn a_comment_without_a_position_is_refused() {
        refused(
            mutant(2, |f| f.retain(|(p, _)| *p != term::comment_index())),
            "comment without position",
            "position",
        );
    }

    #[test]
    fn a_label_differing_from_the_name_is_refused() {
        // Replaced, not added: still one label, so only sh:equals sees it.
        refused(
            mutant(0, |f| {
                f.retain(|(p, _)| p != vocab::RDFS_LABEL);
                f.push((vocab::RDFS_LABEL.into(), Obj::Str("different".into())));
            }),
            "label != name",
            "EqualsConstraintComponent",
        );
    }

    /// Validate `data` against a one-constraint shape on `ex:T`.
    fn probe(constraint: &str, data: &str) -> bool {
        let shapes = format!(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
             @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
             @prefix ex: <https://example.org/> .\n\
             ex:S a sh:NodeShape ; sh:targetClass ex:T ; {constraint} .\n"
        );
        let data = format!(
            "<https://example.org/n> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <https://example.org/T> .\n{data}"
        );
        quipu::validate_shapes(&shapes, &data).unwrap().conforms
    }

    #[test]
    fn the_engine_evaluates_xone_or_equals_and_has_value() {
        let p = |o: &str| format!("<https://example.org/n> <https://example.org/p> {o} .\n");
        let q = |o: &str| format!("<https://example.org/n> <https://example.org/q> {o} .\n");
        // sh:hasValue
        let c = "sh:property [ sh:path ex:p ; sh:hasValue \"a\" ]";
        assert!(probe(c, &p("\"a\"")), "hasValue: a matching value conforms");
        assert!(!probe(c, &p("\"b\"")), "hasValue is not evaluated");
        // sh:equals
        let c = "sh:property [ sh:path ex:p ; sh:equals ex:q ]";
        assert!(
            probe(c, &(p("\"a\"") + &q("\"a\""))),
            "equals: equal sets conform"
        );
        assert!(
            !probe(c, &(p("\"a\"") + &q("\"b\""))),
            "equals is not evaluated"
        );
        // sh:or
        let c = "sh:property [ sh:path ex:p ; sh:or ( [ sh:datatype xsd:dateTime ] [ sh:datatype xsd:date ] ) ]";
        assert!(probe(
            c,
            &p(&format!("\"2026-10-03\"^^<{}>", vocab::XSD_DATE))
        ));
        assert!(probe(
            c,
            &p(&format!("\"2026-10-03T00:00:00Z\"^^<{XSD_DT}>"))
        ));
        assert!(!probe(c, &p("\"2026-10-03\"")), "or is not evaluated");
        // sh:xone: exactly one branch, so neither none nor both conform.
        let c = "sh:xone ( [ sh:property [ sh:path ex:p ; sh:minCount 1 ] ] \
                           [ sh:property [ sh:path ex:q ; sh:minCount 1 ] ] )";
        assert!(probe(c, &p("\"a\"")), "xone: one branch conforms");
        assert!(!probe(c, ""), "xone: no branch must not conform");
        assert!(
            !probe(c, &(p("\"a\"") + &q("\"b\""))),
            "xone: two branches must not conform (evaluated as or?)"
        );
        // sh:class, which blockedOn and parentItem lean on.
        let c = "sh:property [ sh:path ex:p ; sh:class ex:T ]";
        assert!(probe(c, &p("<https://example.org/n>")));
        assert!(
            !probe(c, &p("<https://example.org/other>")),
            "class is not evaluated"
        );
    }
}

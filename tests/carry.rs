//! Facts this sd does not model must survive every path that moves a ledger
//! (aegis-w3k75d.14, step 1b): pendant import (merge and --replace), sync,
//! and a comment renumbered by a sync merge. A NEWER sd wrote them; an older
//! sd moving the ledger must not be the reason they disappear.

use quipu::store::Datum;
use quipu::types::{Op, Value};
use seeds::backend::{Backend, Ctx};
use seeds::engine::{self, CreateReq};
use seeds::model::Snapshot;
use seeds::pendant;
use seeds::quipu_backend::QuipuBackend;
use seeds::sync;

const FUTURE: &str = "https://seeds.local/ontology/fieldFromTheFuture";

fn backend(graph: &str) -> QuipuBackend {
    QuipuBackend::in_memory(graph).unwrap()
}

fn ctx(n: u32) -> Ctx {
    Ctx {
        now: format!("2026-10-01T00:00:{:02}Z", n % 60),
        actor: "tester".into(),
        prefix: "sd".into(),
        claims: Default::default(),
    }
}

fn mk(b: &mut QuipuBackend, title: &str, n: u32) -> String {
    engine::create(
        b,
        &ctx(n),
        &CreateReq {
            title: title.into(),
            ..CreateReq::default()
        },
    )
    .unwrap()
    .0
    .id
}

/// Write `FUTURE "value"` on `iri` directly, as a newer sd would.
fn plant(b: &mut QuipuBackend, iri: &str, value: &str) {
    let graph = b.graph_iri().to_string();
    let st = b.store_mut();
    let g = st.graph_create(&graph).unwrap();
    let e = st.intern(iri).unwrap();
    let a = st.intern(FUTURE).unwrap();
    st.transact_to_graph(
        &[Datum {
            entity: e,
            attribute: a,
            value: Value::Str(value.into()),
            valid_from: "2026-10-01T00:00:00Z".into(),
            valid_to: None,
            op: Op::Assert,
        }],
        "2026-10-01T00:00:00Z",
        Some("newer-sd"),
        Some("test"),
        g,
    )
    .unwrap();
}

/// The `FUTURE` values on `iri` in `b`'s project graph.
fn future_on(b: &QuipuBackend, iri: &str) -> Vec<String> {
    let st = b.store();
    let (Some(g), Some(e), Some(a)) = (
        st.graph_create(b.graph_iri()).ok(),
        st.lookup(iri).unwrap(),
        st.lookup(FUTURE).unwrap(),
    ) else {
        return vec![];
    };
    st.entity_facts_in_graph(e, g)
        .unwrap()
        .into_iter()
        .filter(|f| f.attribute == a)
        .filter_map(|f| match f.value {
            Value::Str(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// A source store holding one seed with a planted future fact.
fn source() -> (QuipuBackend, String, String) {
    let mut a = backend("https://seeds.local/project/carry");
    let id = mk(&mut a, "carried", 1);
    let iri = seeds::vocab::item_iri(&id);
    plant(&mut a, &iri, "kept");
    assert_eq!(future_on(&a, &iri), ["kept"], "control: planted");
    (a, id, iri)
}

fn ledger_of(b: &QuipuBackend) -> Snapshot {
    pendant::read(&pendant::export(b).unwrap())
        .unwrap()
        .snapshot
}

#[test]
fn the_pendant_itself_carries_the_fact() {
    // Control for the import arms: the export is a raw graph dump, so the
    // fact IS in export.nt. Any loss below is on the way in.
    let (a, _, _) = source();
    let p = pendant::export(&a).unwrap();
    assert!(p.export_nt().unwrap().contains(FUTURE));
}

#[test]
fn import_merge_carries_unmodelled_facts() {
    let (a, _, iri) = source();
    let mut b = backend("https://seeds.local/project/carry");
    sync::import(&mut b, &ctx(2), &ledger_of(&a), None, false).unwrap();
    assert_eq!(future_on(&b, &iri), ["kept"]);
}

#[test]
fn import_replace_carries_unmodelled_facts() {
    let (a, _, iri) = source();
    let mut b = backend("https://seeds.local/project/carry");
    mk(&mut b, "only in the target, replaced away", 1);
    sync::import(&mut b, &ctx(2), &ledger_of(&a), None, true).unwrap();
    assert_eq!(future_on(&b, &iri), ["kept"]);
}

#[test]
fn import_replace_keeps_a_targets_own_unmodelled_fact() {
    // The target already has the seed with its fact; the incoming copy differs
    // in a modelled field, so replace rewrites the seed.
    let (mut a, id, iri) = source();
    let mut b = backend("https://seeds.local/project/carry");
    sync::import(&mut b, &ctx(2), &ledger_of(&a), None, false).unwrap();
    plant(&mut b, &iri, "kept");
    engine::update(
        &mut a,
        &ctx(3),
        std::slice::from_ref(&id),
        &engine::UpdateReq {
            priority: Some("0".into()),
            ..Default::default()
        },
    )
    .unwrap();
    sync::import(&mut b, &ctx(4), &ledger_of(&a), None, true).unwrap();
    assert_eq!(
        b.snapshot(None).unwrap().seeds[&id].priority,
        0,
        "control: rewritten"
    );
    assert_eq!(future_on(&b, &iri), ["kept"]);
}

#[test]
fn sync_carries_unmodelled_facts_to_the_other_side() {
    let (mut local, _, iri) = source();
    let mut remote = backend("https://seeds.local/project/carry");
    sync::sync(
        &Snapshot::default(),
        &mut local,
        &mut remote,
        &ctx(2),
        false,
    )
    .unwrap();
    assert_eq!(future_on(&local, &iri), ["kept"], "control: local keeps it");
    assert_eq!(future_on(&remote, &iri), ["kept"]);
}

#[test]
fn a_comment_renumbered_by_sync_keeps_its_unmodelled_facts() {
    let mut local = backend("https://seeds.local/project/carry");
    let id = mk(&mut local, "commented", 1);
    let mut remote = backend("https://seeds.local/project/carry");
    sync::sync(
        &Snapshot::default(),
        &mut local,
        &mut remote,
        &ctx(2),
        false,
    )
    .unwrap();
    let base = local.snapshot(None).unwrap();
    // Both sides add comment 1; the merge renumbers the local one to 2.
    engine::comment_add(&mut remote, &ctx(3), &id, "remote's", Some("r")).unwrap();
    engine::comment_add(&mut local, &ctx(4), &id, "local's", Some("l")).unwrap();
    plant(
        &mut local,
        &seeds::vocab::comment_iri(&id, 1),
        "on the comment",
    );
    sync::sync(&base, &mut local, &mut remote, &ctx(5), false).unwrap();
    let moved = seeds::vocab::comment_iri(&id, 2);
    let after = local.snapshot(None).unwrap();
    assert!(
        after
            .comments
            .iter()
            .any(|c| c.index == 2 && c.text == "local's"),
        "control: the local comment was renumbered to 2"
    );
    assert_eq!(future_on(&local, &moved), ["on the comment"], "local");
    assert_eq!(future_on(&remote, &moved), ["on the comment"], "remote");
}

#[test]
fn a_field_by_field_merge_keeps_unmodelled_facts() {
    // Both sides change the seed (different fields), so sync merges it field
    // by field (merge_seed), not by taking one side's copy whole.
    // The fact is added on the local side AFTER the base, so the remote can
    // only get it through this merge (step 1 already keeps a fact a store
    // holds; this is one the remote does not hold yet).
    let mut local = backend("https://seeds.local/project/carry");
    let id = mk(&mut local, "merged", 1);
    let iri = seeds::vocab::item_iri(&id);
    let mut remote = backend("https://seeds.local/project/carry");
    sync::sync(
        &Snapshot::default(),
        &mut local,
        &mut remote,
        &ctx(2),
        false,
    )
    .unwrap();
    let base = local.snapshot(None).unwrap();
    plant(&mut local, &iri, "kept");
    let set = |b: &mut QuipuBackend, n: u32, req: engine::UpdateReq| {
        engine::update(b, &ctx(n), std::slice::from_ref(&id), &req).unwrap();
    };
    set(
        &mut local,
        3,
        engine::UpdateReq {
            priority: Some("0".into()),
            ..Default::default()
        },
    );
    set(
        &mut remote,
        4,
        engine::UpdateReq {
            assignee: Some("r".into()),
            ..Default::default()
        },
    );
    sync::sync(&base, &mut local, &mut remote, &ctx(5), false).unwrap();
    let merged = remote.snapshot(None).unwrap().seeds[&id].clone();
    assert_eq!(
        (merged.priority, merged.assignee.as_deref()),
        (0, Some("r")),
        "control: a real field-by-field merge"
    );
    assert_eq!(future_on(&remote, &iri), ["kept"]);
    assert_eq!(future_on(&local, &iri), ["kept"]);
}

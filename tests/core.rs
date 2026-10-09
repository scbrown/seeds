//! The core verbs against an in-memory quipu store. No clock, no filesystem:
//! the same file runs natively (`cargo test`) and on wasm32
//! (`just wasm-test`), where each test is a `wasm_bindgen_test`.
#![allow(clippy::unwrap_used)]

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

use seeds::backend::{Backend, Ctx, SeedWrite, WriteBatch};
use seeds::engine::{self, CountReq, CreateReq, Filter, ListReq, ReadyReq, UpdateReq};
use seeds::error::ErrorKind;
use seeds::quipu_backend::QuipuBackend;

const GRAPH: &str = "https://seeds.local/project/sd";

fn backend() -> QuipuBackend {
    QuipuBackend::in_memory(GRAPH).unwrap()
}

fn ctx(n: u32) -> Ctx {
    Ctx {
        now: format!("2026-09-30T00:00:{:02}Z", n % 60),
        actor: "tester".into(),
        prefix: "sd".into(),
        claims: Default::default(),
    }
}

#[test]
fn item_snapshot_matches_full_state_without_unrelated_seeds() {
    let mut b = backend();
    let first = mk(&mut b, "first", 1);
    let other = mk(&mut b, "other", 2);
    engine::comment_add(&mut b, &ctx(3), &first, "one", Some("alice")).unwrap();
    engine::comment_add(&mut b, &ctx(4), &first, "two", Some("bob")).unwrap();
    engine::comment_add(&mut b, &ctx(5), &other, "unrelated", None).unwrap();
    let full = b.snapshot(None).unwrap();
    let scoped = b.snapshot_items(std::slice::from_ref(&first)).unwrap();
    assert_eq!(scoped.seeds.len(), 1);
    assert_eq!(scoped.get(&first).unwrap(), full.get(&first).unwrap());
    assert_eq!(
        scoped.comments,
        full.comments_on(&first)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(scoped.tx, full.tx);
    assert!(b
        .snapshot_items(&["absent".into()])
        .unwrap()
        .seeds
        .is_empty());
    engine::update(
        &mut b,
        &ctx(6),
        std::slice::from_ref(&first),
        &UpdateReq {
            title: Some("edited".into()),
            transition_comment: Some("transition".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let after = b.snapshot(None).unwrap();
    assert_eq!(after.get(&other).unwrap(), full.get(&other).unwrap());
    assert_eq!(after.comments_on(&first).len(), 3);
    assert_eq!(after.comments_on(&first)[2].index, 3);
}

#[test]
fn exported_dependency_keeps_its_own_actor_and_creation_time() {
    let mut b = backend();
    let source = mk(&mut b, "source", 1);
    let target = mk(&mut b, "target", 2);
    let edge_ctx = Ctx {
        actor: "edge-author".into(),
        ..ctx(10)
    };
    engine::dep_add(&mut b, &edge_ctx, &source, &target, "related").unwrap();
    engine::update(
        &mut b,
        &ctx(20),
        std::slice::from_ref(&source),
        &UpdateReq {
            title: Some("later unrelated edit".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    let records = seeds::beads::encode(&b.snapshot(None).unwrap()).unwrap();
    let edge = &records[&source]["dependencies"][0];
    assert_eq!(edge["created_by"], "edge-author");
    assert_eq!(edge["created_at"], edge_ctx.now);
    let imported = seeds::beads::decode(&records).unwrap();
    assert_eq!(seeds::beads::encode(&imported).unwrap(), records);
    // A peer correction to imported metadata is carried, never overwritten by
    // the native provenance facts carried alongside it.
    let mut corrected = records.clone();
    corrected.get_mut(&source).unwrap()["dependencies"][0]["created_by"] =
        serde_json::json!("peer-correction");
    assert_eq!(
        seeds::beads::encode(&seeds::beads::decode(&corrected).unwrap()).unwrap(),
        corrected
    );
    // Persist/import first, then remove and re-add: the old raw JSON and the
    // earlier origin must not override the new edge's actor/time.
    let mut restored = backend();
    seeds::sync::import(&mut restored, &ctx(21), &imported, None, false).unwrap();
    engine::dep_remove(&mut restored, &ctx(22), &source, &target, "related").unwrap();
    let new_ctx = Ctx {
        actor: "new-edge-author".into(),
        ..ctx(23)
    };
    engine::dep_add(&mut restored, &new_ctx, &source, &target, "related").unwrap();
    let again = seeds::beads::encode(&restored.snapshot(None).unwrap()).unwrap();
    assert_eq!(
        again[&source]["dependencies"][0]["created_by"],
        "new-edge-author"
    );
    assert_eq!(again[&source]["dependencies"][0]["created_at"], new_ctx.now);
    let mut unproven = b.snapshot(None).unwrap();
    unproven
        .seeds
        .get_mut(&source)
        .unwrap()
        .extra
        .retain(|(p, _)| *p != seeds::vocab::seeds("dependencyOrigin"));
    assert!(seeds::beads::encode(&unproven)
        .unwrap_err()
        .to_string()
        .contains("lacks creation provenance"));
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

fn ready_ids(b: &QuipuBackend, n: u32) -> Vec<String> {
    engine::ready(b, &ctx(n), &ReadyReq::default(), None)
        .unwrap()
        .issues
        .into_iter()
        .map(|s| s.id)
        .collect()
}

#[test]
fn create_then_show_round_trips_every_field() {
    let mut b = backend();
    let (seed, tx) = engine::create(
        &mut b,
        &ctx(1),
        &CreateReq {
            title: "Ship the parser".into(),
            description: Some("line one\n\"quoted\" line two".into()),
            issue_type: Some("bug".into()),
            priority: Some("P1".into()),
            assignee: Some("ian".into()),
            labels: vec!["core,parser".into(), "p0".into()],
            ..CreateReq::default()
        },
    )
    .unwrap();
    assert!(tx > 0);
    assert!(seed.id.starts_with("sd-"));
    let shown = engine::show(&b, std::slice::from_ref(&seed.id), None).unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].seed, seed);
    assert_eq!(seed.priority, 1);
    assert_eq!(seed.issue_type, "bug");
    assert_eq!(
        seed.labels.iter().cloned().collect::<Vec<_>>(),
        vec!["core", "p0", "parser"]
    );
    assert_eq!(seed.revision, 1);
}

#[test]
fn show_of_a_missing_id_is_not_found() {
    let b = backend();
    let e = engine::show(&b, &["sd-nope".into()], None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn ready_excludes_a_seed_blocked_by_an_open_dep_and_includes_it_once_the_dep_closes() {
    let mut b = backend();
    let a = mk(&mut b, "write the parser", 1);
    let blocker = mk(&mut b, "design the grammar", 2);
    assert_eq!(
        sorted(ready_ids(&b, 3)),
        sorted(vec![a.clone(), blocker.clone()])
    );

    engine::dep_add(&mut b, &ctx(3), &a, &blocker, "blocks").unwrap();
    assert_eq!(ready_ids(&b, 4), vec![blocker.clone()]);

    engine::close(
        &mut b,
        &ctx(5),
        std::slice::from_ref(&blocker),
        Some("done"),
        false,
    )
    .unwrap();
    assert_eq!(ready_ids(&b, 6), vec![a.clone()]);

    // Reopening the blocker blocks again: ready is computed, never stored.
    engine::update(
        &mut b,
        &ctx(7),
        std::slice::from_ref(&blocker),
        &UpdateReq {
            status: Some("open".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    assert_eq!(ready_ids(&b, 8), vec![blocker]);
}

#[test]
fn related_and_parent_edges_do_not_block() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let other = mk(&mut b, "other", 2);
    engine::dep_add(&mut b, &ctx(3), &a, &other, "related").unwrap();
    assert!(ready_ids(&b, 4).contains(&a));
}

#[test]
fn ready_by_sparql_agrees_with_ready_by_model() {
    // Two independent implementations of "ready": the SPARQL the backend runs
    // and a direct walk of the snapshot. They must agree at every step.
    let mut b = backend();
    let ids: Vec<String> = (0..6).map(|i| mk(&mut b, &format!("s{i}"), i)).collect();
    let steps: Vec<(&str, usize, usize)> = vec![
        ("dep", 0, 1),
        ("dep", 1, 2),
        ("dep", 3, 2),
        ("close", 2, 0),
        ("dep", 4, 5),
        ("close", 5, 0),
        ("reopen", 2, 0),
        ("close", 1, 0),
    ];
    for (n, (op, x, y)) in steps.into_iter().enumerate() {
        let c = ctx(10 + n as u32);
        match op {
            "dep" => {
                engine::dep_add(&mut b, &c, &ids[x], &ids[y], "blocks").unwrap();
            }
            "close" => {
                engine::close(&mut b, &c, &[ids[x].clone()], Some("r"), true).unwrap();
            }
            _ => {
                engine::update(
                    &mut b,
                    &c,
                    &[ids[x].clone()],
                    &UpdateReq {
                        status: Some("open".into()),
                        ..UpdateReq::default()
                    },
                )
                .unwrap();
            }
        }
        let snap = b.snapshot(None).unwrap();
        assert_eq!(
            sorted(ready_ids(&b, 50)),
            sorted(engine::ready_by_model(&snap, &c.now)),
            "step {n} ({op} {x} {y})"
        );
    }
}

#[test]
fn deferred_seeds_are_not_ready_until_the_date() {
    let mut b = backend();
    let a = mk(&mut b, "later", 1);
    engine::update(
        &mut b,
        &ctx(2),
        std::slice::from_ref(&a),
        &UpdateReq {
            defer: Some("2026-10-01".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    assert!(ready_ids(&b, 3).is_empty());
    let later = Ctx {
        now: "2026-10-01T09:00:00Z".into(),
        ..ctx(0)
    };
    let p = engine::ready(&b, &later, &ReadyReq::default(), None).unwrap();
    assert_eq!(p.issues.len(), 1);
}

#[test]
fn ready_is_not_truncated_by_default_and_says_so_when_limited() {
    let mut b = backend();
    for i in 0..7 {
        mk(&mut b, &format!("s{i}"), i);
    }
    let all = engine::ready(&b, &ctx(9), &ReadyReq::default(), None).unwrap();
    assert_eq!((all.issues.len(), all.total, all.has_more), (7, 7, false));
    let cut = engine::ready(
        &b,
        &ctx(9),
        &ReadyReq {
            limit: Some(3),
            ..ReadyReq::default()
        },
        None,
    )
    .unwrap();
    assert_eq!((cut.issues.len(), cut.total, cut.has_more), (3, 7, true));
}

#[test]
fn list_filters_and_pages() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let c = mk(&mut b, "c", 2);
    mk(&mut b, "d", 3);
    engine::update(
        &mut b,
        &ctx(4),
        std::slice::from_ref(&a),
        &UpdateReq {
            add_labels: vec!["x".into()],
            assignee: Some("ian".into()),
            priority: Some("0".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    engine::close(&mut b, &ctx(5), std::slice::from_ref(&c), Some("r"), false).unwrap();

    let open = engine::list(&b, &ListReq::default(), None).unwrap();
    assert_eq!(open.total, 2, "closed seeds are hidden by default");
    assert_eq!(open.issues[0].id, a, "sorted by priority");

    let all = engine::list(
        &b,
        &ListReq {
            all: true,
            ..ListReq::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(all.total, 3);

    let labelled = engine::list(
        &b,
        &ListReq {
            filter: Filter {
                labels: vec!["x".into()],
                assignee: Some("ian".into()),
                ..Filter::default()
            },
            ..ListReq::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(labelled.issues.len(), 1);

    let closed = engine::list(
        &b,
        &ListReq {
            filter: Filter {
                status: Some("closed".into()),
                ..Filter::default()
            },
            ..ListReq::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(closed.issues[0].id, c);

    let page = engine::list(
        &b,
        &ListReq {
            limit: Some(1),
            ..ListReq::default()
        },
        None,
    )
    .unwrap();
    assert!(page.has_more);
    assert_eq!(page.issues.len(), 1);
}

#[test]
fn count_totals_and_groups() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    mk(&mut b, "b", 2);
    engine::close(&mut b, &ctx(3), std::slice::from_ref(&a), Some("r"), false).unwrap();
    let c = engine::count(&b, &CountReq::default(), None).unwrap();
    assert_eq!(c.total, 1);
    let g = engine::count(
        &b,
        &CountReq {
            by: Some("status".into()),
            include_closed: true,
            ..CountReq::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        g.groups.unwrap(),
        vec![("closed".to_string(), 1), ("open".to_string(), 1)]
    );
}

#[test]
fn update_round_trips_and_bumps_the_revision() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let (seeds, _) = engine::update(
        &mut b,
        &ctx(2),
        std::slice::from_ref(&a),
        &UpdateReq {
            title: Some("renamed".into()),
            notes: Some("n".into()),
            status: Some("in_progress".into()),
            add_labels: vec!["x".into(), "y".into()],
            ..UpdateReq::default()
        },
    )
    .unwrap();
    assert_eq!(seeds[0].revision, 2);
    engine::update(
        &mut b,
        &ctx(3),
        std::slice::from_ref(&a),
        &UpdateReq {
            remove_labels: vec!["x".into()],
            ..UpdateReq::default()
        },
    )
    .unwrap();
    let s = &engine::show(&b, std::slice::from_ref(&a), None).unwrap()[0].seed;
    assert_eq!(s.title, "renamed");
    assert_eq!(s.status, "in_progress");
    assert_eq!(s.notes.as_deref(), Some("n"));
    assert_eq!(s.labels.iter().cloned().collect::<Vec<_>>(), vec!["y"]);
    assert_eq!(s.revision, 3);
}

#[test]
fn claim_is_a_compare_and_set() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let me = ctx(2);
    let (s, _) = engine::update(
        &mut b,
        &me,
        std::slice::from_ref(&a),
        &UpdateReq {
            claim: true,
            ..UpdateReq::default()
        },
    )
    .unwrap();
    assert_eq!(s[0].assignee.as_deref(), Some("tester"));
    assert_eq!(s[0].status, "in_progress");
    let rival = Ctx {
        actor: "rival".into(),
        ..ctx(3)
    };
    let e = engine::update(
        &mut b,
        &rival,
        std::slice::from_ref(&a),
        &UpdateReq {
            claim: true,
            ..UpdateReq::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    // Re-claiming your own claim is a no-op, not a conflict.
    engine::update(
        &mut b,
        &me,
        std::slice::from_ref(&a),
        &UpdateReq {
            claim: true,
            ..UpdateReq::default()
        },
    )
    .unwrap();
}

#[test]
fn a_stale_write_is_a_conflict_and_writes_nothing() {
    // Two writers read revision 1. The first commits; the second's batch still
    // says "based on revision 1" and must be refused whole, not merged over.
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let snap = b.snapshot(None).unwrap();
    let base = snap.get(&a).unwrap().clone();

    let mut first = base.clone();
    first.title = "first".into();
    first.revision = 2;
    b.commit(
        &WriteBatch {
            seeds: vec![SeedWrite {
                seed: first,
                expected_revision: Some(1),
            }],
            comments: vec![],
            source: "test".into(),
            ..WriteBatch::default()
        },
        &ctx(2),
    )
    .unwrap();

    let mut second = base;
    second.labels.insert("mine".into());
    second.revision = 2;
    let e = b
        .commit(
            &WriteBatch {
                seeds: vec![SeedWrite {
                    seed: second,
                    expected_revision: Some(1),
                }],
                comments: vec![],
                source: "test".into(),
                ..WriteBatch::default()
            },
            &ctx(3),
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    let now = b.snapshot(None).unwrap();
    let s = now.get(&a).unwrap();
    assert_eq!(s.title, "first", "the first write survived");
    assert!(s.labels.is_empty(), "the stale write left nothing behind");
}

#[test]
fn close_requires_blockers_closed_unless_forced_and_warns_without_a_reason() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let blocker = mk(&mut b, "b", 2);
    engine::dep_add(&mut b, &ctx(3), &a, &blocker, "blocks").unwrap();
    let e = engine::close(&mut b, &ctx(4), std::slice::from_ref(&a), Some("r"), false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    let (seeds, _, warnings) =
        engine::close(&mut b, &ctx(5), std::slice::from_ref(&a), None, true).unwrap();
    assert_eq!(seeds[0].status, "closed");
    assert_eq!(seeds[0].closed_at.as_deref(), Some("2026-09-30T00:00:05Z"));
    assert!(warnings.iter().any(|w| w.contains("--reason")));
}

#[test]
fn close_is_all_or_nothing_across_ids() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let e = engine::close(
        &mut b,
        &ctx(2),
        &[a.clone(), "sd-missing".into()],
        Some("r"),
        false,
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    let s = &engine::show(&b, std::slice::from_ref(&a), None).unwrap()[0].seed;
    assert_eq!(s.status, "open");
}

#[test]
fn dep_add_remove_list_round_trip_and_refuse_cycles() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let c = mk(&mut b, "c", 2);
    let d = mk(&mut b, "d", 3);
    assert_eq!(
        engine::dep_add(&mut b, &ctx(4), &a, &c, "blocks")
            .unwrap()
            .action,
        "added"
    );
    assert_eq!(
        engine::dep_add(&mut b, &ctx(4), &a, &c, "blocks")
            .unwrap()
            .action,
        "unchanged"
    );
    engine::dep_add(&mut b, &ctx(5), &c, &d, "blocks").unwrap();
    let e = engine::dep_add(&mut b, &ctx(6), &d, &a, "blocks").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    let e = engine::dep_add(&mut b, &ctx(6), &a, &a, "blocks").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    let e = engine::dep_add(&mut b, &ctx(6), &a, "sd-missing", "blocks").unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);

    let down = engine::dep_list(&b, &a, false, None).unwrap();
    assert_eq!(down.len(), 1);
    assert_eq!(down[0].depends_on_id, c);
    let up = engine::dep_list(&b, &c, true, None).unwrap();
    assert_eq!(up[0].issue_id, a);

    engine::dep_remove(&mut b, &ctx(7), &a, &c, "blocks").unwrap();
    assert!(engine::dep_list(&b, &a, false, None).unwrap().is_empty());
    let e = engine::dep_remove(&mut b, &ctx(8), &a, &c, "blocks").unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn create_with_parent_mints_a_child_id_and_deps() {
    let mut b = backend();
    let p = mk(&mut b, "epic", 1);
    let blocker = mk(&mut b, "blocker", 2);
    let (child, _) = engine::create(
        &mut b,
        &ctx(3),
        &CreateReq {
            title: "child".into(),
            parent: Some(p.clone()),
            deps: vec![blocker.clone(), format!("related:{p}")],
            ..CreateReq::default()
        },
    )
    .unwrap();
    assert_eq!(child.id, format!("{p}.1"));
    assert!(child.blocked_on.contains(&blocker));
    assert!(child.related.contains(&p));
    assert!(!ready_ids(&b, 4).contains(&child.id));
}

#[test]
fn comments_add_and_list_round_trip_in_order() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    engine::comment_add(&mut b, &ctx(2), &a, "first", None).unwrap();
    engine::comment_add(&mut b, &ctx(3), &a, "second\nline", Some("ian")).unwrap();
    let cs = engine::comment_list(&b, &a, None).unwrap();
    assert_eq!(cs.len(), 2);
    assert_eq!(
        (cs[0].index, cs[0].text.as_str(), cs[0].author.as_str()),
        (1, "first", "tester")
    );
    assert_eq!(
        (cs[1].index, cs[1].text.as_str(), cs[1].author.as_str()),
        (2, "second\nline", "ian")
    );
    let e = engine::comment_add(&mut b, &ctx(4), "sd-missing", "x", None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}

#[test]
fn at_pins_a_seed_to_an_earlier_transaction() {
    let mut b = backend();
    let (s, tx1) = engine::create(
        &mut b,
        &ctx(1),
        &CreateReq {
            title: "Ship the parser".into(),
            priority: Some("1".into()),
            ..CreateReq::default()
        },
    )
    .unwrap();
    let (_, tx2) = engine::update(
        &mut b,
        &ctx(2),
        std::slice::from_ref(&s.id),
        &UpdateReq {
            title: Some("Ship the streaming parser".into()),
            priority: Some("0".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    assert!(tx2 > tx1);
    let now = &engine::show(&b, std::slice::from_ref(&s.id), None).unwrap()[0].seed;
    let then = &engine::show(&b, std::slice::from_ref(&s.id), Some(tx1)).unwrap()[0].seed;
    assert_eq!(
        (now.title.as_str(), now.priority),
        ("Ship the streaming parser", 0)
    );
    assert_eq!((then.title.as_str(), then.priority), ("Ship the parser", 1));
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn raw_write(b: &mut QuipuBackend, seed: seeds::model::Seed) -> seeds::error::SdError {
    b.commit(
        &WriteBatch {
            seeds: vec![SeedWrite {
                seed,
                expected_revision: None,
            }],
            comments: vec![],
            source: "test".into(),
            ..WriteBatch::default()
        },
        &ctx(1),
    )
    .unwrap_err()
}

fn raw_seed(id: &str) -> seeds::model::Seed {
    seeds::model::Seed {
        id: id.into(),
        title: "raw".into(),
        status: "open".into(),
        priority: 2,
        issue_type: "task".into(),
        created_at: "2026-09-30T00:00:00Z".into(),
        updated_at: "2026-09-30T00:00:00Z".into(),
        revision: 1,
        ..Default::default()
    }
}

#[test]
fn a_blocked_on_edge_to_a_missing_seed_is_refused_by_the_backend() {
    // The engine checks this too; the backend must hold on its own, because
    // another caller of the Backend trait may not go through the engine.
    let mut b = backend();
    let mut s = raw_seed("sd-raw");
    s.blocked_on.insert("sd-ghost".into());
    assert_eq!(raw_write(&mut b, s).kind, ErrorKind::Refused);
    assert!(
        b.snapshot(None).unwrap().seeds.is_empty(),
        "nothing was written"
    );
}

#[cfg(feature = "shacl")]
#[test]
fn the_shapes_refuse_a_seed_the_engine_would_never_build() {
    let mut b = backend();
    assert!(b.validates());
    let mut s = raw_seed("sd-raw");
    s.status = "done".into(); // not a status
    let e = raw_write(&mut b, s);
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(e.message.contains("status"), "{}", e.message);
    assert!(b.snapshot(None).unwrap().seeds.is_empty());
}

// ---------------------------------------------------------------- pendants and sync

use seeds::pendant::{self, Seal};
use seeds::sync::{self, Prefer};

fn board(b: &mut QuipuBackend) -> (String, String) {
    let a = mk(b, "write the parser", 1);
    let g = mk(b, "design the grammar", 2);
    engine::dep_add(b, &ctx(3), &a, &g, "blocks").unwrap();
    engine::comment_add(b, &ctx(4), &a, "needs the grammar first", None).unwrap();
    engine::update(
        b,
        &ctx(5),
        std::slice::from_ref(&g),
        &UpdateReq {
            add_labels: vec!["design".into()],
            ..UpdateReq::default()
        },
    )
    .unwrap();
    (a, g)
}

#[test]
fn a_pendant_round_trips_into_a_second_store_and_ready_agrees() {
    let mut a = backend();
    board(&mut a);
    let p = pendant::export(&a).unwrap();
    assert_eq!(pendant::seal(&p).unwrap(), Seal::Intact);
    for f in pendant::FILES {
        assert!(p.files.contains_key(f), "{f}");
    }

    let ledger = pendant::read(&p).unwrap();
    let mut b = QuipuBackend::in_memory("https://seeds.local/project/other").unwrap();
    let r = sync::import(&mut b, &ctx(9), &ledger.snapshot, None, false).unwrap();
    assert_eq!(r.created.len(), 2);
    assert_eq!(r.comments_added, 1);

    let (sa, sb) = (a.snapshot(None).unwrap(), b.snapshot(None).unwrap());
    assert_eq!(sa.seeds, sb.seeds, "seeds survive the round trip exactly");
    assert_eq!(sa.comments, sb.comments);
    assert_eq!(ready_ids(&a, 10), ready_ids(&b, 10));

    // Deterministic: the second store exports the same ledger bytes.
    let p2 = pendant::export(&b).unwrap();
    assert_eq!(p.export_nt(), p2.export_nt());

    // Importing the same pendant again changes nothing.
    let again = sync::import(&mut b, &ctx(11), &ledger.snapshot, None, false).unwrap();
    assert_eq!(
        (again.created.len(), again.updated.len(), again.tx),
        (0, 0, 0)
    );
}

#[test]
fn an_edited_pendant_breaks_its_seal_but_good_data_still_reads() {
    let mut a = backend();
    board(&mut a);
    let mut p = pendant::export(&a).unwrap();
    let nt = p.files.get_mut(pendant::EXPORT_NT).unwrap();
    // A hand edit: reorder two lines. Still valid, no longer canonical.
    let mut lines: Vec<&str> = nt.lines().collect();
    lines.swap(0, 1);
    *nt = lines.join("\n") + "\n";
    assert!(matches!(pendant::seal(&p).unwrap(), Seal::Broken(_)));
    let ledger = pendant::read(&p).unwrap();
    assert_eq!(ledger.snapshot.seeds.len(), 2);
}

#[test]
fn a_merge_that_kept_both_statuses_is_reported_not_resolved() {
    let mut a = backend();
    let (ida, _) = board(&mut a);
    let mut p = pendant::export(&a).unwrap();
    let nt = p.files.get_mut(pendant::EXPORT_NT).unwrap();
    nt.push_str(&format!(
        "<{}> <https://seeds.local/ontology/status> \"closed\" .\n",
        seeds::vocab::item_iri(&ida)
    ));
    let e = pendant::read(&p).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    assert!(
        e.message.contains(&ida) && e.message.contains("status"),
        "{}",
        e.message
    );
}

#[test]
fn import_reports_conflicts_and_writes_nothing_unless_a_side_is_named() {
    let mut a = backend();
    let (ida, _) = board(&mut a);
    let p = pendant::read(&pendant::export(&a).unwrap()).unwrap();
    let mut b = backend();
    sync::import(&mut b, &ctx(9), &p.snapshot, None, false).unwrap();
    // Diverge: the store changes the title, the incoming ledger the priority.
    engine::update(
        &mut b,
        &ctx(10),
        std::slice::from_ref(&ida),
        &UpdateReq {
            title: Some("store title".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    engine::update(
        &mut a,
        &ctx(10),
        std::slice::from_ref(&ida),
        &UpdateReq {
            priority: Some("0".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    let incoming = pendant::read(&pendant::export(&a).unwrap())
        .unwrap()
        .snapshot;
    let before = b.snapshot(None).unwrap();
    let e = sync::import(&mut b, &ctx(11), &incoming, None, false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    assert!(e.message.contains(&ida), "{}", e.message);
    assert_eq!(
        b.snapshot(None).unwrap().seeds,
        before.seeds,
        "nothing was written"
    );

    let kept = sync::import(&mut b, &ctx(12), &incoming, Some(Prefer::Existing), false).unwrap();
    assert_eq!(kept.updated.len(), 0);
    let took = sync::import(&mut b, &ctx(13), &incoming, Some(Prefer::Incoming), false).unwrap();
    assert_eq!(took.updated, vec![ida.clone()]);
    let s = b.snapshot(None).unwrap();
    let s = s.get(&ida).unwrap();
    assert_eq!((s.title.as_str(), s.priority), ("write the parser", 0));
    assert!(
        s.revision > before.get(&ida).unwrap().revision,
        "revision moved forward"
    );
}

#[test]
fn replace_makes_the_store_exactly_the_ledger() {
    let mut a = backend();
    board(&mut a);
    let incoming = pendant::read(&pendant::export(&a).unwrap())
        .unwrap()
        .snapshot;
    let mut b = backend();
    let stray = mk(&mut b, "only in the store", 1);
    let r = sync::import(&mut b, &ctx(9), &incoming, None, true).unwrap();
    assert_eq!(r.removed, vec![stray]);
    assert_eq!(b.snapshot(None).unwrap().seeds, incoming.seeds);
}

#[test]
fn sync_merges_changes_from_both_sides_field_by_field() {
    let mut local = backend();
    let (a, g) = board(&mut local);
    let base = pendant::read(&pendant::export(&local).unwrap())
        .unwrap()
        .snapshot;
    let mut remote = QuipuBackend::in_memory("https://seeds.local/project/remote").unwrap();
    sync::import(&mut remote, &ctx(9), &base, None, true).unwrap();

    // Local closes the grammar; remote relabels the parser and comments on it.
    engine::close(
        &mut local,
        &ctx(10),
        std::slice::from_ref(&g),
        Some("done"),
        false,
    )
    .unwrap();
    engine::update(
        &mut remote,
        &ctx(10),
        std::slice::from_ref(&a),
        &UpdateReq {
            add_labels: vec!["remote".into()],
            ..UpdateReq::default()
        },
    )
    .unwrap();
    engine::comment_add(&mut remote, &ctx(11), &a, "from the remote", None).unwrap();
    engine::comment_add(&mut local, &ctx(11), &a, "from local", None).unwrap();

    let (merged, _lr, _rr) = sync::sync(&base, &mut local, &mut remote, &ctx(12), false).unwrap();
    let (l, r) = (
        local.snapshot(None).unwrap(),
        remote.snapshot(None).unwrap(),
    );
    assert_eq!(l.seeds, r.seeds, "both sides agree after sync");
    assert_eq!(l.comments, r.comments);
    assert_eq!(l.seeds, merged.seeds);
    assert_eq!(l.get(&g).unwrap().status, "closed");
    assert!(l.get(&a).unwrap().labels.contains("remote"));
    let texts: Vec<&str> = l.comments_on(&a).iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts.len(), 3, "both new comments kept: {texts:?}");
    assert_eq!(ready_ids(&local, 13), ready_ids(&remote, 13));
    assert_eq!(ready_ids(&local, 13), vec![a.clone()]);

    // A second sync with nothing new is a no-op.
    let new_base = local.snapshot(None).unwrap();
    let (_, lr, rr) = sync::sync(&new_base, &mut local, &mut remote, &ctx(14), false).unwrap();
    assert!(!lr.wrote && !rr.wrote, "nothing written on either side");
}

#[test]
fn sync_reports_a_field_both_sides_changed_and_writes_nothing() {
    let mut local = backend();
    let (a, _) = board(&mut local);
    let base = local.snapshot(None).unwrap();
    let mut remote = backend();
    sync::import(&mut remote, &ctx(9), &base, None, true).unwrap();
    let set_status = |b: &mut QuipuBackend, st: &str| {
        engine::update(
            b,
            &ctx(10),
            std::slice::from_ref(&a),
            &UpdateReq {
                status: Some(st.into()),
                ..UpdateReq::default()
            },
        )
        .unwrap();
    };
    set_status(&mut local, "in_progress");
    set_status(&mut remote, "blocked");
    let (lb, rb) = (
        local.snapshot(None).unwrap(),
        remote.snapshot(None).unwrap(),
    );
    let e = sync::sync(&base, &mut local, &mut remote, &ctx(11), false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    assert!(e.message.contains("status"), "{}", e.message);
    assert_eq!(local.snapshot(None).unwrap().seeds, lb.seeds);
    assert_eq!(remote.snapshot(None).unwrap().seeds, rb.seeds);
}

// A backend whose commit LANDS and then reports an error: a lost response.
struct LandsThenErrors<'a> {
    inner: &'a mut QuipuBackend,
    fail_next: bool,
}

impl Backend for LandsThenErrors<'_> {
    fn snapshot(&self, at: Option<u64>) -> seeds::error::Result<seeds::model::Snapshot> {
        self.inner.snapshot(at)
    }
    fn ready_ids(&self, at: Option<u64>) -> seeds::error::Result<Vec<String>> {
        self.inner.ready_ids(at)
    }
    fn claims_of(&self, id: &str) -> seeds::error::Result<Vec<(u64, seeds::backend::Claims)>> {
        self.inner.claims_of(id)
    }
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> seeds::error::Result<u64> {
        let tx = self.inner.commit(batch, ctx)?;
        if self.fail_next {
            self.fail_next = false;
            return Err(seeds::error::SdError::failed(
                "connection reset (response lost)",
            ));
        }
        Ok(tx)
    }
}

#[test]
fn a_sync_retried_after_a_lost_response_does_not_duplicate_comments() {
    // wu's blocker-2 repro: both sides add a comment to the same seed; the
    // remote write lands but its response is lost; the sync is retried.
    let mut local = backend();
    let (a, _) = board(&mut local);
    let base = local.snapshot(None).unwrap();
    let mut remote_store = backend();
    sync::import(&mut remote_store, &ctx(9), &base, None, true).unwrap();
    engine::comment_add(&mut remote_store, &ctx(10), &a, "from remote", None).unwrap();
    engine::comment_add(&mut local, &ctx(10), &a, "from local", None).unwrap();

    let mut flaky = LandsThenErrors {
        inner: &mut remote_store,
        fail_next: true,
    };
    assert!(sync::sync(&base, &mut local, &mut flaky, &ctx(11), false).is_err());
    // Local and base are unchanged; retry.
    sync::sync(&base, &mut local, &mut flaky, &ctx(12), false).unwrap();
    for side in [
        local.snapshot(None).unwrap(),
        remote_store.snapshot(None).unwrap(),
    ] {
        let mut texts: Vec<&str> = side
            .comments_on(&a)
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        texts.sort();
        assert_eq!(
            texts,
            vec!["from local", "from remote", "needs the grammar first"],
            "exactly one copy each"
        );
    }
}

#[test]
fn sync_refuses_removals_unless_allowed() {
    let mut local = backend();
    board(&mut local);
    let base = local.snapshot(None).unwrap();
    let mut remote = backend(); // empty: as if reset
    let e = sync::sync(&base, &mut local, &mut remote, &ctx(9), false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(
        e.message.contains("remove 2 seed(s) from the local store"),
        "{}",
        e.message
    );
    assert_eq!(local.snapshot(None).unwrap().seeds.len(), 2);
    sync::sync(&base, &mut local, &mut remote, &ctx(10), true).unwrap();
    assert!(local.snapshot(None).unwrap().seeds.is_empty());
}

#[test]
fn label_add_and_remove_report_br_statuses_in_one_transaction() {
    let mut b = backend();
    let (x, y) = (mk(&mut b, "x", 1), mk(&mut b, "y", 2));
    let ids = vec![x.clone(), y.clone()];
    let (changes, tx) = engine::label_change(&mut b, &ctx(3), &ids, "infra", true).unwrap();
    assert!(tx > 0);
    assert_eq!(
        changes.iter().map(|c| c.status).collect::<Vec<_>>(),
        ["added", "added"]
    );
    // Adding again writes nothing and says so; the transaction does not move.
    let (again, tx2) = engine::label_change(&mut b, &ctx(4), &ids, "infra", true).unwrap();
    assert_eq!(again[0].status, "exists");
    assert_eq!(tx2, tx);
    let (gone, _) =
        engine::label_change(&mut b, &ctx(5), std::slice::from_ref(&x), "infra", false).unwrap();
    assert_eq!(gone[0].status, "removed");
    let (none, _) =
        engine::label_change(&mut b, &ctx(6), std::slice::from_ref(&x), "infra", false).unwrap();
    assert_eq!(none[0].status, "not_found");
    assert_eq!(engine::labels(&b, Some(&y), None).unwrap(), ["infra"]);
    assert!(engine::labels(&b, Some(&x), None).unwrap().is_empty());
}

#[test]
fn label_change_with_an_unknown_id_writes_nothing() {
    let mut b = backend();
    let x = mk(&mut b, "x", 1);
    let err = engine::label_change(&mut b, &ctx(2), &[x.clone(), "sd-nope".into()], "l", true)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert!(engine::labels(&b, Some(&x), None).unwrap().is_empty());
}

#[test]
fn a_label_must_be_one_non_empty_label() {
    let mut b = backend();
    let x = mk(&mut b, "x", 1);
    for bad in ["", "  ", "a,b"] {
        let err =
            engine::label_change(&mut b, &ctx(2), std::slice::from_ref(&x), bad, true).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{bad:?}");
    }
}

#[test]
fn label_list_and_counts_include_closed_seeds_and_rename_moves_every_carrier() {
    let mut b = backend();
    let (x, y) = (mk(&mut b, "x", 1), mk(&mut b, "y", 2));
    engine::label_change(&mut b, &ctx(3), &[x.clone(), y.clone()], "old", true).unwrap();
    engine::label_change(&mut b, &ctx(4), std::slice::from_ref(&y), "other", true).unwrap();
    engine::close(
        &mut b,
        &ctx(5),
        std::slice::from_ref(&y),
        Some("done"),
        false,
    )
    .unwrap();
    assert_eq!(engine::labels(&b, None, None).unwrap(), ["old", "other"]);
    assert_eq!(
        engine::label_counts(&b, None).unwrap(),
        [("old".to_string(), 2), ("other".to_string(), 1)]
    );
    let (n, _) = engine::label_rename(&mut b, &ctx(6), "old", "new").unwrap();
    assert_eq!(n, 2);
    assert_eq!(engine::labels(&b, None, None).unwrap(), ["new", "other"]);
    let (n, _) = engine::label_rename(&mut b, &ctx(7), "absent", "z").unwrap();
    assert_eq!(n, 0);
}

#[test]
fn blocked_lists_open_blockers_and_ignores_a_blocked_status_alone() {
    let mut b = backend();
    let (x, y, z) = (
        mk(&mut b, "blocker", 1),
        mk(&mut b, "blocked", 2),
        mk(&mut b, "status only", 3),
    );
    engine::dep_add(&mut b, &ctx(4), &y, &x, "blocks").unwrap();
    engine::update(
        &mut b,
        &ctx(5),
        std::slice::from_ref(&z),
        &UpdateReq {
            status: Some("blocked".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    let page = engine::blocked(&b, &engine::BlockedReq::default(), None).unwrap();
    assert_eq!(
        page.page
            .issues
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        [y.as_str()]
    );
    assert_eq!(page.blocked_by[&y], [x.as_str()]);
    // A type filter that excludes it, then closing the blocker, both empty the list.
    let bugs = engine::BlockedReq {
        types: vec!["bug".into()],
        ..engine::BlockedReq::default()
    };
    assert!(engine::blocked(&b, &bugs, None)
        .unwrap()
        .page
        .issues
        .is_empty());
    engine::close(
        &mut b,
        &ctx(6),
        std::slice::from_ref(&x),
        Some("done"),
        false,
    )
    .unwrap();
    assert!(engine::blocked(&b, &engine::BlockedReq::default(), None)
        .unwrap()
        .page
        .issues
        .is_empty());
}

#[test]
fn reopen_only_reopens_closed_seeds_and_stores_the_reason_in_the_same_tx() {
    let mut b = backend();
    let (x, y) = (mk(&mut b, "x", 1), mk(&mut b, "y", 2));
    engine::close(
        &mut b,
        &ctx(3),
        std::slice::from_ref(&x),
        Some("done"),
        false,
    )
    .unwrap();
    let (done, skipped, tx) =
        engine::reopen(&mut b, &ctx(4), &[x.clone(), y.clone()], Some("not done")).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].seed.status, "open");
    assert_eq!(done[0].previous_status, "closed");
    assert_eq!(done[0].seed.closed_at, None);
    assert_eq!(skipped[0].id, y);
    assert_eq!(skipped[0].reason, "already open");
    let cs = engine::comment_list(&b, &x, None).unwrap();
    assert_eq!(cs.last().unwrap().text, "Reopened: not done");
    // The comment and the status change are one transaction: pinned before it,
    // neither exists.
    assert!(engine::comment_list(&b, &x, Some(tx - 1))
        .unwrap()
        .is_empty());
}

#[test]
fn defer_and_undefer_move_status_and_date_and_skip_the_wrong_states() {
    let mut b = backend();
    let (x, closed) = (mk(&mut b, "x", 1), mk(&mut b, "closed", 2));
    engine::close(
        &mut b,
        &ctx(3),
        std::slice::from_ref(&closed),
        Some("done"),
        false,
    )
    .unwrap();
    let (done, skipped, _) =
        engine::defer(&mut b, &ctx(4), &[x.clone(), closed.clone()], Some("+1d")).unwrap();
    assert_eq!(done[0].seed.status, "deferred");
    assert_eq!(
        done[0].seed.defer_until.as_deref(),
        Some("2026-10-01T00:00:04Z")
    );
    assert_eq!(skipped[0].reason, "cannot defer closed issue");
    assert!(!ready_ids(&b, 5).contains(&x));
    let (done, _, _) = engine::undefer(&mut b, &ctx(6), std::slice::from_ref(&x)).unwrap();
    assert_eq!(done[0].seed.status, "open");
    assert_eq!(done[0].seed.defer_until, None);
    assert!(ready_ids(&b, 7).contains(&x));
    let (_, skipped, _) = engine::undefer(&mut b, &ctx(8), std::slice::from_ref(&x)).unwrap();
    assert_eq!(skipped[0].reason, "not deferred (status: open)");
}

#[test]
fn until_accepts_brs_forms_and_rolls_over_months_and_years() {
    let now = "2026-12-31T23:30:00Z";
    let p = |v| engine::parse_until(v, now).unwrap();
    assert_eq!(p("+30m"), "2027-01-01T00:00:00Z");
    assert_eq!(p("+2h"), "2027-01-01T01:30:00Z");
    assert_eq!(p("+1w"), "2027-01-07T23:30:00Z");
    assert_eq!(p("tomorrow"), "2027-01-01");
    assert_eq!(p("2027-03-01"), "2027-03-01");
    assert_eq!(p("2027-03-01T09:00:00Z"), "2027-03-01T09:00:00Z");
    assert_eq!(
        engine::parse_until("+1d", "2028-02-28T12:00:00Z").unwrap(),
        "2028-02-29T12:00:00Z"
    );
    for bad in ["", "soon", "+1y", "+d", "2027-13-01", "2027-3-1"] {
        assert_eq!(
            engine::parse_until(bad, now).unwrap_err().kind,
            ErrorKind::Usage,
            "{bad:?}"
        );
    }
}

fn found(b: &QuipuBackend, q: &str, all: bool) -> (usize, usize) {
    let r = engine::search(
        b,
        &engine::SearchReq {
            query: q.into(),
            full: true,
            all,
            ..engine::SearchReq::default()
        },
        None,
    )
    .unwrap();
    (r.page.issues.len(), r.hidden_closed)
}

#[test]
fn search_matches_id_title_description_and_comments_but_not_notes() {
    let mut b = backend();
    let (seed, _) = engine::create(
        &mut b,
        &ctx(1),
        &CreateReq {
            title: "Parser handles UTF-8".into(),
            description: Some("the lexer chokes on multibyte input".into()),
            ..CreateReq::default()
        },
    )
    .unwrap();
    let id = seed.id;
    engine::comment_add(&mut b, &ctx(2), &id, "zebra in a comment", None).unwrap();
    engine::update(
        &mut b,
        &ctx(3),
        std::slice::from_ref(&id),
        &UpdateReq {
            notes: Some("quokka in the notes".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    for q in ["parser", "CHOKES ON", "zebra", &id[..4]] {
        assert_eq!(found(&b, q, false), (1, 0), "{q}");
    }
    assert_eq!(
        found(&b, "quokka", false),
        (0, 0),
        "notes are not searched, as in br"
    );
    engine::close(
        &mut b,
        &ctx(4),
        std::slice::from_ref(&id),
        Some("done"),
        false,
    )
    .unwrap();
    assert_eq!(
        found(&b, "parser", false),
        (0, 1),
        "closed hits are hidden and counted"
    );
    assert_eq!(found(&b, "parser", true), (1, 0));
    let err = engine::search(&b, &engine::SearchReq::default(), None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

/// A backend whose FIRST commit is preceded by another writer's commit of
/// the same batch: the race a keyed create must survive, made deterministic.
/// The inner store's own compare-and-set is what refuses the second write.
struct RacedBackend {
    inner: QuipuBackend,
    raced: bool,
}

impl Backend for RacedBackend {
    fn snapshot(&self, at: Option<u64>) -> seeds::error::Result<seeds::model::Snapshot> {
        self.inner.snapshot(at)
    }
    fn ready_ids(&self, at: Option<u64>) -> seeds::error::Result<Vec<String>> {
        self.inner.ready_ids(at)
    }
    fn claims_of(&self, id: &str) -> seeds::error::Result<Vec<(u64, seeds::backend::Claims)>> {
        self.inner.claims_of(id)
    }
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> seeds::error::Result<u64> {
        if !self.raced {
            self.raced = true;
            self.inner.commit(batch, ctx)?; // the other writer wins
        }
        self.inner.commit(batch, ctx)
    }
}

#[test]
fn a_keyed_create_that_loses_the_race_returns_the_winners_seed() {
    let req = CreateReq {
        title: "raced".into(),
        workflow_run: Some("r1".into()),
        step: Some("s".into()),
        ..CreateReq::default()
    };
    let mut b = RacedBackend {
        inner: backend(),
        raced: false,
    };
    let (seed, tx) = engine::create(&mut b, &ctx(1), &req).unwrap();
    assert_eq!(tx, 0, "our write was refused; the seed is the winner's");
    let snap = b.snapshot(None).unwrap();
    assert_eq!(snap.seeds.len(), 1);
    assert!(snap.seeds.contains_key(&seed.id));

    // CONTROL: an UNKEYED create under the same race is not idempotent. It
    // must surface the conflict, so the test above is not passing vacuously.
    let mut b = RacedBackend {
        inner: backend(),
        raced: false,
    };
    let plain = CreateReq {
        title: "raced".into(),
        ..CreateReq::default()
    };
    let e = engine::create(&mut b, &ctx(1), &plain).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
}

fn at_time(now: &str) -> Ctx {
    Ctx {
        now: now.into(),
        ..ctx(0)
    }
}

#[test]
fn stale_lists_untouched_non_closed_seeds_oldest_first() {
    let mut b = backend();
    let make = |b: &mut QuipuBackend, t: &str, now: &str| {
        engine::create(
            b,
            &at_time(now),
            &CreateReq {
                title: t.into(),
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let old = make(&mut b, "old", "2026-08-01T00:00:00Z");
    let older = make(&mut b, "older", "2026-07-01T00:00:00Z");
    let fresh = make(&mut b, "fresh", "2026-09-29T00:00:00Z");
    let gone = make(&mut b, "gone", "2026-06-01T00:00:00Z");
    engine::close(
        &mut b,
        &at_time("2026-06-02T00:00:00Z"),
        std::slice::from_ref(&gone),
        Some("done"),
        false,
    )
    .unwrap();
    let now = at_time("2026-09-30T00:00:00Z");
    let ids = |v: Vec<seeds::model::Seed>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(
        ids(engine::stale(&b, &now, 30, &[], None).unwrap()),
        [older.clone(), old.clone()]
    );
    assert_eq!(
        ids(engine::stale(&b, &now, 0, &[], None).unwrap()),
        [older.clone(), old.clone(), fresh]
    );
    // Closed only when asked for, and statuses may be comma-separated.
    assert_eq!(
        ids(engine::stale(&b, &now, 30, &["closed,open".into()], None).unwrap()),
        [gone, older, old]
    );
    let err = engine::stale(&b, &now, 30, &["nope".into()], None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn stats_counts_statuses_ready_lead_time_epics_and_breakdowns() {
    let mut b = backend();
    let mk_at = |b: &mut QuipuBackend, t: &str, ty: &str, parent: Option<&str>, now: &str| {
        engine::create(
            b,
            &at_time(now),
            &CreateReq {
                title: t.into(),
                issue_type: Some(ty.into()),
                parent: parent.map(str::to_string),
                labels: if t == "a" { vec!["ops".into()] } else { vec![] },
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let epic = mk_at(&mut b, "epic", "epic", None, "2026-09-01T00:00:00Z");
    let child = mk_at(&mut b, "child", "task", Some(&epic), "2026-09-01T00:00:00Z");
    let a = mk_at(&mut b, "a", "bug", None, "2026-09-01T00:00:00Z");
    // Child closed 10h after creation: the epic becomes eligible, lead time 10h.
    engine::close(
        &mut b,
        &at_time("2026-09-01T10:00:00Z"),
        std::slice::from_ref(&child),
        Some("done"),
        false,
    )
    .unwrap();
    let st = engine::stats(
        &b,
        &at_time("2026-09-02T00:00:00Z"),
        engine::StatsReq {
            by_type: true,
            by_label: true,
            ..engine::StatsReq::default()
        },
        None,
    )
    .unwrap();
    assert_eq!((st.total, st.open, st.closed, st.ready), (3, 2, 1, 2));
    assert_eq!(st.epics_eligible_for_closure, 1);
    assert!((st.average_lead_time_hours - 10.0).abs() < 1e-9);
    assert_eq!(st.breakdowns[0].0, "type");
    assert_eq!(
        st.breakdowns[0].1,
        [
            ("bug".to_string(), 1),
            ("epic".to_string(), 1),
            ("task".to_string(), 1)
        ]
    );
    assert_eq!(
        st.breakdowns[1].1,
        [("(no labels)".to_string(), 2), ("ops".to_string(), 1)]
    );
    let _ = a;
}

#[test]
fn epic_status_and_close_eligible_follow_br() {
    let mut b = backend();
    let epic = |b: &mut QuipuBackend, t: &str, n: u32| {
        engine::create(
            b,
            &ctx(n),
            &CreateReq {
                title: t.into(),
                issue_type: Some("epic".into()),
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let (done, empty) = (epic(&mut b, "done", 1), epic(&mut b, "empty", 2));
    let kid = engine::create(
        &mut b,
        &ctx(3),
        &CreateReq {
            title: "kid".into(),
            parent: Some(done.clone()),
            ..CreateReq::default()
        },
    )
    .unwrap()
    .0
    .id;
    let rows = engine::epic_status(&b, false, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|r| !r.eligible_for_close),
        "no children closed yet; empty is never eligible"
    );
    engine::close(
        &mut b,
        &ctx(4),
        std::slice::from_ref(&kid),
        Some("done"),
        false,
    )
    .unwrap();
    let eligible = engine::epic_status(&b, true, None).unwrap();
    assert_eq!(eligible.len(), 1);
    assert_eq!(
        (eligible[0].total_children, eligible[0].closed_children),
        (1, 1)
    );
    let (closed, skipped, _) = engine::epic_close_eligible(&mut b, &ctx(5)).unwrap();
    assert_eq!(closed[0].id, done);
    assert_eq!(
        closed[0].close_reason.as_deref(),
        Some("All children completed")
    );
    assert!(skipped.is_empty());
    // A closed epic leaves status; the childless one stays, ineligible.
    let rows = engine::epic_status(&b, false, None).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.epic.id.clone()).collect::<Vec<_>>(),
        [empty]
    );
}

#[test]
fn list_pages_count_dependents_of_every_type_as_br_does() {
    let mut b = backend();
    let target = mk(&mut b, "target", 1);
    let blocks = mk(&mut b, "blocks", 2);
    let related = mk(&mut b, "related", 3);
    engine::dep_add(&mut b, &ctx(4), &blocks, &target, "blocks").unwrap();
    engine::dep_add(&mut b, &ctx(5), &related, &target, "related").unwrap();
    engine::create(
        &mut b,
        &ctx(6),
        &CreateReq {
            title: "child".into(),
            parent: Some(target.clone()),
            ..CreateReq::default()
        },
    )
    .unwrap();
    let page = engine::list(&b, &ListReq::default(), None).unwrap();
    assert_eq!(
        page.dependent_counts[&target], 3,
        "blocks + related + parent-child"
    );
    assert_eq!(page.dependent_counts[&blocks], 0);
}

#[test]
fn delete_tombstones_hides_everywhere_never_blocks_and_is_not_a_close() {
    let mut b = backend();
    let (x, y, z) = (
        mk(&mut b, "blocker", 1),
        mk(&mut b, "blocked", 2),
        mk(&mut b, "other", 3),
    );
    engine::dep_add(&mut b, &ctx(4), &y, &x, "blocks").unwrap();
    // A seed with a live dependent is only previewed without --cascade/--force.
    let r = engine::delete(
        &mut b,
        &ctx(5),
        std::slice::from_ref(&x),
        "dup",
        false,
        false,
        false,
    )
    .unwrap();
    assert!(r.preview);
    assert_eq!(r.blocked_dependents, [y.as_str()]);
    assert!(engine::list(&b, &ListReq::default(), None)
        .unwrap()
        .issues
        .iter()
        .any(|s| s.id == x));
    // --force: deleted; the dependent is orphaned but NOT blocked (constraint 1).
    let r = engine::delete(
        &mut b,
        &ctx(6),
        std::slice::from_ref(&x),
        "dup",
        false,
        true,
        false,
    )
    .unwrap();
    assert!(!r.preview && r.tx > 0);
    assert_eq!(r.orphaned, [y.as_str()]);
    assert!(ready_ids(&b, 7).contains(&y), "a tombstone never blocks");
    // Hidden from every listing and count; show still returns it.
    let listed = engine::list(
        &b,
        &ListReq {
            all: true,
            ..ListReq::default()
        },
        None,
    )
    .unwrap();
    assert!(!listed.issues.iter().any(|s| s.id == x));
    assert_eq!(
        engine::count(&b, &CountReq::default(), None).unwrap().total,
        2
    );
    let st = engine::stats(&b, &ctx(8), engine::StatsReq::default(), None).unwrap();
    assert_eq!((st.total, st.tombstones), (2, 1));
    let shown = engine::show(&b, std::slice::from_ref(&x), None).unwrap();
    let t = &shown[0].seed;
    assert_eq!(t.status, "tombstone");
    // Not a close (constraint 2): no outcome, no closed_at.
    assert_eq!((t.outcome.clone(), t.closed_at.clone()), (None, None));
    assert_eq!(
        engine::comment_list(&b, &x, None)
            .unwrap()
            .last()
            .unwrap()
            .text,
        "Deleted: dup"
    );
    // Cannot gain a dependency in either direction (constraint 1).
    for (from, to) in [(&z, &x), (&x, &z)] {
        let err = engine::dep_add(&mut b, &ctx(9), from, to, "related").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Refused);
    }
    // update cannot set the status; delete is the only way in.
    let err = engine::update(
        &mut b,
        &ctx(10),
        std::slice::from_ref(&z),
        &UpdateReq {
            status: Some("tombstone".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[test]
fn delete_cascade_takes_dependents_with_it() {
    let mut b = backend();
    let (x, y, z) = (mk(&mut b, "x", 1), mk(&mut b, "y", 2), mk(&mut b, "z", 3));
    engine::dep_add(&mut b, &ctx(4), &y, &x, "blocks").unwrap();
    engine::dep_add(&mut b, &ctx(5), &z, &y, "related").unwrap();
    let dry = engine::delete(
        &mut b,
        &ctx(6),
        std::slice::from_ref(&x),
        "",
        true,
        false,
        true,
    )
    .unwrap();
    assert!(dry.preview);
    assert_eq!(
        engine::count(&b, &CountReq::default(), None).unwrap().total,
        3,
        "dry run writes nothing"
    );
    let r = engine::delete(
        &mut b,
        &ctx(7),
        std::slice::from_ref(&x),
        "",
        true,
        false,
        false,
    )
    .unwrap();
    let mut gone = r.deleted.clone();
    gone.sort();
    let mut want = vec![x, y, z];
    want.sort();
    assert_eq!(gone, want, "transitive: z depends on y depends on x");
    assert_eq!(
        engine::count(&b, &CountReq::default(), None).unwrap().total,
        0
    );
}

#[test]
fn deleting_a_closed_seed_drops_its_close_so_a_tombstone_is_never_a_close() {
    // wu, seeds#28 review: delete set the status only, so a seed closed first
    // kept closed_at and close_reason. History keeps them; --at still reads them.
    let mut b = backend();
    let x = mk(&mut b, "x", 1);
    let (_, closed_tx, _) = engine::close(
        &mut b,
        &ctx(2),
        std::slice::from_ref(&x),
        Some("done earlier"),
        false,
    )
    .unwrap();
    engine::delete(
        &mut b,
        &ctx(3),
        std::slice::from_ref(&x),
        "dup",
        false,
        false,
        false,
    )
    .unwrap();
    let t = engine::show(&b, std::slice::from_ref(&x), None).unwrap()[0]
        .seed
        .clone();
    assert_eq!(t.status, "tombstone");
    assert_eq!(
        (t.closed_at, t.close_reason, t.outcome),
        (None, None, None),
        "a tombstone carries no close"
    );
    let before = engine::show(&b, std::slice::from_ref(&x), Some(closed_tx)).unwrap()[0]
        .seed
        .clone();
    assert_eq!(
        before.close_reason.as_deref(),
        Some("done earlier"),
        "history keeps the close"
    );
}

#[test]
fn graph_walks_like_br_dependents_dependencies_and_components() {
    // The shape measured on br: target T, B blocks-on T, C child of T, R related to T.
    let mut b = backend();
    let t = mk(&mut b, "target", 1);
    let bl = mk(&mut b, "blocks", 2);
    let r = mk(&mut b, "related", 3);
    engine::dep_add(&mut b, &ctx(4), &bl, &t, "blocks").unwrap();
    engine::dep_add(&mut b, &ctx(5), &r, &t, "related").unwrap();
    let c = engine::create(
        &mut b,
        &ctx(6),
        &CreateReq {
            title: "child".into(),
            parent: Some(t.clone()),
            ..CreateReq::default()
        },
    )
    .unwrap()
    .0
    .id;
    let ids = |g: &engine::Graph| {
        g.nodes
            .iter()
            .map(|n| (n.seed.id.clone(), n.depth))
            .collect::<Vec<_>>()
    };
    // Dependents: only blocks edges (br shows neither the related nor the child).
    let g = engine::graph(&b, &t, false, None).unwrap();
    assert_eq!(ids(&g), [(t.clone(), 0), (bl.clone(), 1)]);
    assert_eq!(g.edges, [(bl.clone(), t.clone())]);
    // Dependencies: B waits on T, and T (a parent) waits on its child.
    let g = engine::graph(&b, &bl, true, None).unwrap();
    assert_eq!(ids(&g), [(bl.clone(), 0), (t.clone(), 1), (c.clone(), 2)]);
    assert_eq!(g.edges, [(bl.clone(), t.clone()), (t.clone(), c.clone())]);
    // Components: {C, T, B} rooted at C; R alone.
    let all = engine::graph_all(&b, None).unwrap();
    let big = all.iter().find(|g| g.nodes.len() == 3).unwrap();
    assert_eq!(big.roots, [c.as_str()]);
    assert_eq!(ids(big), [(c, 0), (t, 1), (bl, 2)]);
    assert!(all
        .iter()
        .any(|g| g.roots == [r.clone()] && g.nodes.len() == 1));
}

#[test]
fn history_finds_the_exact_tx_of_every_version_amid_other_writes() {
    let mut b = backend();
    let (x, t_create) = {
        let (s, tx) = engine::create(
            &mut b,
            &ctx(1),
            &CreateReq {
                title: "first".into(),
                ..CreateReq::default()
            },
        )
        .unwrap();
        (s.id, tx)
    };
    // Noise: other seeds written between x's versions.
    for n in 2..6 {
        mk(&mut b, &format!("noise {n}"), n);
    }
    let (_, t_update) = engine::update(
        &mut b,
        &ctx(7),
        std::slice::from_ref(&x),
        &UpdateReq {
            title: Some("second".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    mk(&mut b, "more noise", 8);
    let (_, t_close, _) = engine::close(
        &mut b,
        &ctx(9),
        std::slice::from_ref(&x),
        Some("done"),
        false,
    )
    .unwrap();
    mk(&mut b, "after", 10);

    let h = engine::history(&b, &x).unwrap();
    assert_eq!(
        h.iter().map(|e| e.tx).collect::<Vec<_>>(),
        [t_create, t_update, t_close],
        "each version at the tx its write reported"
    );
    assert!(h[0].changes.is_empty());
    assert!(h[1].changes.iter().any(|c| c.starts_with("title:")));
    assert!(h[2].changes.iter().any(|c| c.starts_with("status:")));
    // Every entry is exactly what --at reads at that tx.
    for e in &h {
        let at = engine::show(&b, std::slice::from_ref(&x), Some(e.tx)).unwrap()[0]
            .seed
            .clone();
        assert_eq!(at, e.seed);
    }
    assert_eq!(
        engine::history(&b, "sd-nope").unwrap_err().kind,
        ErrorKind::NotFound
    );
}

#[test]
fn changelog_groups_closed_seeds_by_type_newest_first_since_a_date() {
    let mut b = backend();
    let mk_ty = |b: &mut QuipuBackend, t: &str, ty: &str| {
        engine::create(
            b,
            &at_time("2026-09-01T00:00:00Z"),
            &CreateReq {
                title: t.into(),
                issue_type: Some(ty.into()),
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let (bug_old, bug_new, task, open) = (
        mk_ty(&mut b, "old bug", "bug"),
        mk_ty(&mut b, "new bug", "bug"),
        mk_ty(&mut b, "task", "task"),
        mk_ty(&mut b, "open", "task"),
    );
    let close_at = |b: &mut QuipuBackend, id: &str, when: &str| {
        engine::close(b, &at_time(when), &[id.to_string()], Some("done"), false).unwrap();
    };
    close_at(&mut b, &bug_old, "2026-09-10T00:00:00Z");
    close_at(&mut b, &bug_new, "2026-09-20T00:00:00Z");
    close_at(&mut b, &task, "2026-09-15T00:00:00Z");
    let all = engine::changelog(&b, None, None).unwrap();
    assert_eq!(
        all.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(),
        ["Bugs", "Tasks"]
    );
    assert_eq!(
        all[0]
            .issues
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        [bug_new.clone(), bug_old]
    );
    assert!(!all.iter().flat_map(|g| &g.issues).any(|s| s.id == open));
    let since = engine::parse_since("2026-09-14", "2026-09-30T00:00:00Z").unwrap();
    let recent = engine::changelog(&b, Some(&since), None).unwrap();
    assert_eq!(recent.iter().map(|g| g.issues.len()).sum::<usize>(), 2);
    assert_eq!(
        engine::parse_since("+7d", "2026-09-30T00:00:00Z").unwrap(),
        "2026-09-23T00:00:00Z",
        "br's relative form means the last span"
    );
    assert_eq!(
        engine::parse_since("soon", "2026-09-30T00:00:00Z")
            .unwrap_err()
            .kind,
        ErrorKind::Usage
    );
}

#[test]
fn lint_reports_brs_template_sections_per_type() {
    let mut b = backend();
    let mk_d = |b: &mut QuipuBackend, t: &str, ty: &str, d: Option<&str>| {
        engine::create(
            b,
            &ctx(1),
            &CreateReq {
                title: t.into(),
                issue_type: Some(ty.into()),
                description: d.map(str::to_string),
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let bug = mk_d(&mut b, "bug", "bug", None);
    let task_ok = mk_d(
        &mut b,
        "ok",
        "task",
        Some("intro\n\n### acceptance criteria\n- x"),
    );
    let epic = mk_d(
        &mut b,
        "epic",
        "epic",
        Some("## Acceptance Criteria\n(wrong section)"),
    );
    mk_d(&mut b, "chore", "chore", None);
    let r = engine::lint(&b, &[], None, None, None).unwrap();
    let by: std::collections::BTreeMap<_, _> = r
        .iter()
        .map(|x| {
            (
                x.seed.id.clone(),
                x.missing.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        by[&bug],
        ["## Steps to Reproduce", "## Acceptance Criteria"]
    );
    assert_eq!(by[&epic], ["## Success Criteria"]);
    assert!(
        !by.contains_key(&task_ok),
        "any heading level, case-insensitive"
    );
    assert_eq!(by.len(), 2, "chores have no template");
    // Closed seeds are skipped unless asked for.
    engine::close(
        &mut b,
        &ctx(2),
        std::slice::from_ref(&bug),
        Some("done"),
        false,
    )
    .unwrap();
    assert_eq!(engine::lint(&b, &[], None, None, None).unwrap().len(), 1);
    assert_eq!(
        engine::lint(&b, &[], None, Some("all"), None)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        engine::lint(&b, &[], Some("bug"), Some("all"), None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn orphans_match_whole_ids_newest_commit_first_and_only_open_seeds() {
    let mut b = backend();
    let a = mk(&mut b, "a", 1);
    let child = engine::create(
        &mut b,
        &ctx(2),
        &CreateReq {
            title: "child".into(),
            parent: Some(a.clone()),
            ..CreateReq::default()
        },
    )
    .unwrap()
    .0
    .id;
    let done = mk(&mut b, "done", 3);
    engine::close(
        &mut b,
        &ctx(4),
        std::slice::from_ref(&done),
        Some("ok"),
        false,
    )
    .unwrap();
    let c = |h: &str, s: &str| (h.to_string(), s.to_string(), String::new());
    let commits = vec![
        c("new1", &format!("fix: finish {child}.")),
        c("old1", &format!("wip ({a}); also {done}")),
        c("old0", &format!("start {a}")),
    ];
    let found = engine::orphans(&b, &commits, None).unwrap();
    let got: Vec<(String, String)> = found
        .iter()
        .map(|(s, c)| (s.id.clone(), c.0.clone()))
        .collect();
    // `child` (sd-x.1) does not also count as its parent; the parent's newest
    // mention is old1; the closed seed is not an orphan.
    let mut want = vec![
        (a.clone(), "old1".to_string()),
        (child.clone(), "new1".to_string()),
    ];
    want.sort();
    let mut got_sorted = got.clone();
    got_sorted.sort();
    assert_eq!(got_sorted, want);
}

#[test]
fn list_filters_paginate_reverse_and_hide_deferred_like_br() {
    let mut b = backend();
    let mk_f = |b: &mut QuipuBackend, t: &str, p: &str, d: Option<&str>, l: &[&str], n: u32| {
        engine::create(
            b,
            &ctx(n),
            &CreateReq {
                title: t.into(),
                priority: Some(p.into()),
                description: d.map(str::to_string),
                labels: l.iter().map(|x| x.to_string()).collect(),
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let a = mk_f(
        &mut b,
        "Parser crash",
        "0",
        Some("the LEXER fails"),
        &["x"],
        1,
    );
    let c = mk_f(&mut b, "docs", "3", None, &["y"], 2);
    let d = mk_f(&mut b, "later", "2", None, &[], 3);
    engine::defer(&mut b, &ctx(4), std::slice::from_ref(&d), None).unwrap();
    let ids = |f: Filter, extra: fn(&mut ListReq)| {
        let mut req = ListReq {
            filter: f,
            limit: Some(0),
            ..ListReq::default()
        };
        extra(&mut req);
        engine::list(&b, &req, None)
            .unwrap()
            .issues
            .into_iter()
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    let none = |_: &mut ListReq| {};
    assert_eq!(
        ids(
            Filter {
                title_contains: Some("parser".into()),
                ..Filter::default()
            },
            none
        ),
        [a.as_str()]
    );
    assert_eq!(
        ids(
            Filter {
                desc_contains: Some("lexer".into()),
                ..Filter::default()
            },
            none
        ),
        [a.as_str()]
    );
    assert_eq!(
        ids(
            Filter {
                labels_any: vec!["x".into(), "y".into()],
                ..Filter::default()
            },
            none
        )
        .len(),
        2
    );
    assert_eq!(
        ids(
            Filter {
                priority_min: Some("1".into()),
                ..Filter::default()
            },
            none
        ),
        [c.as_str()]
    );
    assert_eq!(
        ids(
            Filter {
                priority_max: Some("P1".into()),
                ..Filter::default()
            },
            none
        ),
        [a.as_str()]
    );
    assert_eq!(
        ids(
            Filter {
                ids: vec![c.clone()],
                ..Filter::default()
            },
            none
        ),
        [c.as_str()]
    );
    // Deferred is hidden by default (br), shown with --deferred.
    assert!(!ids(Filter::default(), none).contains(&d));
    assert!(ids(Filter::default(), |r| r.deferred = true).contains(&d));
    // Reverse and offset.
    assert_eq!(
        ids(Filter::default(), |r| r.reverse = true),
        [c.as_str(), a.as_str()]
    );
    let p = engine::list(
        &b,
        &ListReq {
            offset: 1,
            limit: Some(1),
            deferred: true,
            ..ListReq::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(
        (p.issues.len(), p.offset, p.total, p.has_more),
        (1, 1, 3, true)
    );
}

#[test]
fn transition_comments_land_in_the_same_tx_and_only_on_changed_seeds() {
    let mut b = backend();
    let (x, y) = (mk(&mut b, "x", 1), mk(&mut b, "y", 2));
    engine::close(
        &mut b,
        &ctx(3),
        std::slice::from_ref(&y),
        Some("done"),
        false,
    )
    .unwrap();
    // close: x changes and gets the comment; y is already closed and gets none.
    let (_, tx, _) = engine::close_as(
        &mut b,
        &ctx(4),
        &[x.clone(), y.clone()],
        Some("shipped"),
        None,
        false,
        Some("closing with the release"),
    )
    .unwrap();
    let last = |b: &QuipuBackend, id: &str, at: Option<u64>| {
        engine::comment_list(b, id, at)
            .unwrap()
            .last()
            .map(|c| c.text.clone())
    };
    assert_eq!(
        last(&b, &x, None).as_deref(),
        Some("closing with the release")
    );
    assert_eq!(
        last(&b, &x, Some(tx - 1)),
        None,
        "same transaction as the close"
    );
    assert_eq!(
        last(&b, &y, None),
        None,
        "an unchanged seed gets no comment"
    );
    // defer / undefer / update carry it too.
    let z = mk(&mut b, "z", 5);
    engine::defer_with(
        &mut b,
        &ctx(6),
        std::slice::from_ref(&z),
        None,
        Some("waiting on vendor"),
    )
    .unwrap();
    assert_eq!(last(&b, &z, None).as_deref(), Some("waiting on vendor"));
    engine::undefer_with(
        &mut b,
        &ctx(7),
        std::slice::from_ref(&z),
        Some("vendor replied"),
    )
    .unwrap();
    assert_eq!(last(&b, &z, None).as_deref(), Some("vendor replied"));
    engine::update(
        &mut b,
        &ctx(8),
        std::slice::from_ref(&z),
        &UpdateReq {
            priority: Some("0".into()),
            transition_comment: Some("escalated".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    assert_eq!(last(&b, &z, None).as_deref(), Some("escalated"));
    assert_eq!(engine::comment_list(&b, &z, None).unwrap().len(), 3);
}

#[test]
fn reparent_onto_an_existing_parent_loop_is_refused_not_an_infinite_walk() {
    use seeds::backend::{Backend as _, SeedWrite, WriteBatch};
    let mut b = backend();
    let (x, y, z) = (mk(&mut b, "x", 1), mk(&mut b, "y", 2), mk(&mut b, "z", 3));
    // Write a parent loop x -> y -> x below the verbs (a raw write or bad merge).
    let snap = b.snapshot(None).unwrap();
    let mut xs = snap.get(&x).unwrap().clone();
    let mut ys = snap.get(&y).unwrap().clone();
    xs.parent = Some(y.clone());
    ys.parent = Some(x.clone());
    let w = |s: seeds::model::Seed| {
        let r = s.revision;
        let mut s = s;
        s.revision += 1;
        SeedWrite {
            seed: s,
            expected_revision: Some(r),
        }
    };
    b.commit(
        &WriteBatch {
            seeds: vec![w(xs), w(ys)],
            source: "test".into(),
            ..WriteBatch::default()
        },
        &ctx(4),
    )
    .unwrap();
    // Reparenting z under x must terminate with a refusal, not hang.
    let err = engine::update(
        &mut b,
        &ctx(5),
        std::slice::from_ref(&z),
        &UpdateReq {
            parent: Some(x.clone()),
            ..UpdateReq::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.message.contains("loop"), "{}", err.message);
}

// br's `ready` semantics, measured on a br scratch store with the same board:
// hybrid is P0/P1 first then age, oldest is age only, --include-deferred adds
// both status deferred and an unreached defer date, --parent -r reaches
// grandchildren and never the root.
#[test]
fn ready_sorts_includes_deferred_and_scopes_to_descendants_like_br() {
    let mut b = backend();
    let mkp = |b: &mut QuipuBackend, t: &str, n: u32, p: &str, parent: Option<&String>| {
        engine::create(
            b,
            &ctx(n),
            &CreateReq {
                title: t.into(),
                priority: Some(p.into()),
                parent: parent.cloned(),
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let a = mkp(&mut b, "a", 1, "3", None);
    let bb = mkp(&mut b, "b", 2, "1", None);
    let c = mkp(&mut b, "c", 3, "2", None);
    let d = mkp(&mut b, "d", 4, "2", None);
    let e = mkp(&mut b, "e", 5, "2", Some(&bb));
    let f = mkp(&mut b, "f", 6, "4", Some(&e));
    for (id, req) in [
        (
            &c,
            UpdateReq {
                status: Some("deferred".into()),
                ..UpdateReq::default()
            },
        ),
        (
            &d,
            UpdateReq {
                defer: Some("2030-01-01".into()),
                ..UpdateReq::default()
            },
        ),
    ] {
        engine::update(&mut b, &ctx(7), std::slice::from_ref(id), &req).unwrap();
    }
    let run = |req: ReadyReq| -> Result<Vec<String>, seeds::error::SdError> {
        engine::ready(&b, &ctx(8), &req, None).map(|p| p.issues.into_iter().map(|s| s.id).collect())
    };
    let sort = |s: &str| ReadyReq {
        sort: Some(s.into()),
        ..ReadyReq::default()
    };
    assert_eq!(
        run(ReadyReq::default()).unwrap(),
        [bb.as_str(), e.as_str(), a.as_str(), f.as_str()]
    );
    assert_eq!(
        run(sort("priority")).unwrap(),
        [bb.as_str(), e.as_str(), a.as_str(), f.as_str()]
    );
    assert_eq!(
        run(sort("hybrid")).unwrap(),
        [bb.as_str(), a.as_str(), e.as_str(), f.as_str()]
    );
    assert_eq!(
        run(sort("oldest")).unwrap(),
        [a.as_str(), bb.as_str(), e.as_str(), f.as_str()]
    );
    assert_eq!(run(sort("bogus")).unwrap_err().kind, ErrorKind::Usage);
    let all = run(ReadyReq {
        include_deferred: true,
        ..sort("oldest")
    })
    .unwrap();
    assert_eq!(
        all,
        [
            a.as_str(),
            bb.as_str(),
            c.as_str(),
            d.as_str(),
            e.as_str(),
            f.as_str()
        ]
    );
    let under = |recursive| ReadyReq {
        filter: Filter {
            parent: Some(bb.clone()),
            ..Filter::default()
        },
        recursive,
        ..ReadyReq::default()
    };
    assert_eq!(run(under(false)).unwrap(), [e.as_str()]);
    assert_eq!(run(under(true)).unwrap(), [e.as_str(), f.as_str()]);
    // br ignores -r without --parent; sd refuses a flag that would do nothing.
    let lone = run(ReadyReq {
        recursive: true,
        ..ReadyReq::default()
    })
    .unwrap_err();
    assert_eq!(lone.kind, ErrorKind::Usage);
}

// wu's conditions on attribution (aegis-w3k75d.13): a claim is self-asserted
// and recorded beside the actor, never as it; it attaches to exactly the
// version its write produced, even when several writes share one second; and
// it never enters the ledger.
#[test]
fn attribution_claims_attach_to_their_own_version_and_never_to_the_ledger() {
    use seeds::backend::Claims;
    let claimed = |m: &str| Ctx {
        claims: Claims {
            agent_name: Some("gennaro".into()),
            model: Some(m.into()),
            ..Claims::default()
        },
        ..ctx(1)
    };
    let mut b = backend();
    let id = engine::create(
        &mut b,
        &claimed("m1"),
        &CreateReq {
            title: "x".into(),
            ..CreateReq::default()
        },
    )
    .unwrap()
    .0
    .id;
    // Same instant as the create: a timestamp cannot tell these apart.
    let bump = |b: &mut QuipuBackend, c: &Ctx, p: &str| {
        engine::update(
            b,
            c,
            std::slice::from_ref(&id),
            &UpdateReq {
                priority: Some(p.into()),
                ..UpdateReq::default()
            },
        )
        .unwrap();
    };
    bump(&mut b, &ctx(1), "1");
    bump(&mut b, &claimed("m3"), "2");
    let h = engine::history(&b, &id).unwrap();
    let got: Vec<Option<String>> = h
        .iter()
        .map(|e| e.claims.as_ref().and_then(|c| c.model.clone()))
        .collect();
    assert_eq!(got, [Some("m1".into()), None, Some("m3".into())]);
    assert_eq!(
        h[0].claims.as_ref().unwrap().agent_name.as_deref(),
        Some("gennaro")
    );
    // The actor is untouched by a claim.
    assert_eq!(h[0].seed.created_by.as_deref(), Some("tester"));
    // The ledger (every exported file) holds no claim.
    let files = pendant::export(&b).unwrap().files;
    assert!(
        files.values().any(|f| f.contains(&id)),
        "control: export has the seed"
    );
    assert!(files
        .values()
        .all(|f| !f.contains("m1") && !f.contains("m3") && !f.contains("gennaro")));
}

// br's stats recent activity, from seed timestamps: created, closed and
// updated in the window are disjoint, touched counts every seed changed in it,
// and a seed untouched since before the window counts nowhere.
#[test]
fn stats_activity_counts_created_closed_updated_in_the_window() {
    let mut b = backend();
    let mk_at = |b: &mut QuipuBackend, t: &str, when: &str| {
        engine::create(
            b,
            &at_time(when),
            &CreateReq {
                title: t.into(),
                ..CreateReq::default()
            },
        )
        .unwrap()
        .0
        .id
    };
    let old = mk_at(&mut b, "old, edited now", "2026-09-28T00:00:00Z");
    let gone = mk_at(&mut b, "old, closed now", "2026-09-28T00:00:01Z");
    mk_at(&mut b, "old, untouched", "2026-09-28T00:00:02Z");
    mk_at(&mut b, "new", "2026-09-30T00:10:00Z");
    engine::update(
        &mut b,
        &at_time("2026-09-30T00:30:00Z"),
        std::slice::from_ref(&old),
        &UpdateReq {
            priority: Some("1".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    engine::close(
        &mut b,
        &at_time("2026-09-30T00:20:00Z"),
        std::slice::from_ref(&gone),
        Some("done"),
        false,
    )
    .unwrap();
    let st = engine::stats(
        &b,
        &at_time("2026-09-30T01:00:00Z"),
        engine::StatsReq {
            activity_hours: Some(24),
            ..engine::StatsReq::default()
        },
        None,
    )
    .unwrap();
    let a = st.activity.unwrap();
    assert_eq!(
        (a.hours, a.created, a.closed, a.updated, a.touched),
        (24, 1, 1, 1, 3)
    );
    // Without activity_hours there is no activity section.
    let none = engine::stats(&b, &ctx(0), engine::StatsReq::default(), None).unwrap();
    assert!(none.activity.is_none());
}

// Forward compatibility (aegis-w3k75d.13, step 1): a fact whose predicate this
// sd does not model (written by a NEWER sd) must survive this sd's write to the
// same seed. Before, the write's diff retracted it, silently, with success.
#[test]
fn an_update_keeps_facts_this_sd_does_not_model() {
    use quipu::store::Datum;
    use quipu::types::{Op, Value};
    let mut b = backend();
    let id = mk(&mut b, "x", 1);
    let future = "https://seeds.local/ontology/fieldFromTheFuture";
    {
        let graph = b.graph_iri().to_string();
        let st = b.store_mut();
        let g = st.graph_create(&graph).unwrap();
        let e = st.intern(&seeds::vocab::item_iri(&id)).unwrap();
        let a = st.intern(future).unwrap();
        st.transact_to_graph(
            &[Datum {
                entity: e,
                attribute: a,
                value: Value::Str("kept".into()),
                valid_from: "2026-09-30T00:00:02Z".into(),
                valid_to: None,
                op: Op::Assert,
            }],
            "2026-09-30T00:00:02Z",
            Some("newer-sd"),
            Some("test"),
            g,
        )
        .unwrap();
    }
    let has_future = |b: &QuipuBackend| -> bool {
        let st = b.store();
        let g = st.graph_create(b.graph_iri());
        let e = st.lookup(&seeds::vocab::item_iri(&id)).unwrap().unwrap();
        let a = st.lookup(future).unwrap().unwrap();
        st.entity_facts_in_graph(e, g.unwrap())
            .unwrap()
            .iter()
            .any(|f| f.attribute == a && f.value == Value::Str("kept".into()))
    };
    assert!(
        has_future(&b),
        "control: the raw fact is there before the write"
    );
    engine::update(
        &mut b,
        &ctx(3),
        std::slice::from_ref(&id),
        &UpdateReq {
            priority: Some("1".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    assert!(
        has_future(&b),
        "this sd's update must not erase what it does not model"
    );
}

// ---------------------------------------------------------------- create --file (aegis-w3k75d.13)

#[test]
fn bulk_markdown_parses_br_sections_and_drops_nothing() {
    let items = engine::parse_bulk_markdown(
        "# ignored\n\npreamble\n\n## One\n\nPara one.\n\nPara two.\n\n```\n## not a heading\n```\n\n\
         ### Priority\n1\n\n### Type\nbug\n\n### Labels\na, b c\n\n### Assignee\nalice\n\n\
         ### Notes\nN.\n\n### Dependencies\nsd-x, related:sd-y\n\n## Two\n### Description\nD.\n",
    )
    .unwrap();
    assert_eq!(items.len(), 2, "a heading inside a fence is text");
    let one = &items[0];
    assert_eq!(one.req.title, "One");
    let d = one.req.description.as_deref().unwrap();
    assert!(
        d.starts_with("Para one.\n\nPara two."),
        "every paragraph kept: {d}"
    );
    assert!(d.contains("## not a heading"), "{d}");
    assert_eq!(one.req.priority.as_deref(), Some("1"));
    assert_eq!(one.req.issue_type.as_deref(), Some("bug"));
    assert_eq!(one.req.labels, ["a", "b", "c"]);
    assert_eq!(one.req.assignee.as_deref(), Some("alice"));
    assert_eq!(one.notes.as_deref(), Some("N."));
    assert_eq!(one.req.deps, ["sd-x", "related:sd-y"]);
    assert_eq!(items[1].req.description.as_deref(), Some("D."));

    // Design and acceptance criteria are fields now, kept verbatim.
    let items =
        engine::parse_bulk_markdown("## A\n### Design\nG.\n### Acceptance Criteria\n- [ ] k\n")
            .unwrap();
    assert_eq!(items[0].design.as_deref(), Some("G."));
    assert_eq!(items[0].req.acceptance_criteria.as_deref(), Some("- [ ] k"));

    // Refused by name, never dropped.
    for (md, want) in [
        ("## A\n### Estimate\n3\n", "unknown section"),
        ("## A\nbody\n### Description\nD\n", "keep one"),
        ("no items at all\n", "no items"),
    ] {
        let e = engine::parse_bulk_markdown(md).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Usage, "{md}");
        assert!(e.message.contains(want), "{md}: {}", e.message);
    }
}

#[test]
fn create_many_writes_every_item_in_one_transaction_or_none() {
    let mut b = backend();
    let dep = mk(&mut b, "existing", 1);
    let md = format!("## Same\n### Dependencies\n{dep}\n\n## Same\n### Notes\nn\n");
    let items = engine::parse_bulk_markdown(&md).unwrap();
    let (planned, tx) = engine::create_many(&mut b, &ctx(2), &items, true).unwrap();
    assert_eq!((planned.len(), tx), (2, 0));
    assert_eq!(
        b.snapshot(None).unwrap().seeds.len(),
        1,
        "a dry run writes nothing"
    );

    let (seeds, _) = engine::create_many(&mut b, &ctx(2), &items, false).unwrap();
    assert_ne!(
        seeds[0].id, seeds[1].id,
        "same title, same instant: distinct ids"
    );
    let snap = b.snapshot(None).unwrap();
    assert_eq!(snap.seeds.len(), 3);
    assert!(snap.seeds[&seeds[0].id].blocked_on.contains(&dep));
    assert_eq!(snap.seeds[&seeds[1].id].notes.as_deref(), Some("n"));

    // One bad item (an unknown dependency) and nothing is written.
    let bad =
        engine::parse_bulk_markdown("## Fine\n\n## Broken\n### Dependencies\nsd-nope\n").unwrap();
    assert!(engine::create_many(&mut b, &ctx(3), &bad, false).is_err());
    assert_eq!(b.snapshot(None).unwrap().seeds.len(), 3, "all or none");
}

#[test]
fn acceptance_checklist_edits_in_place_like_br() {
    // aegis-w3k75d.13: br's --check/--uncheck/--add-acceptance, from its CLI spec.
    use seeds::engine::edit_checklist;
    let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let body = "Intro line\n- [ ] parse input\n- [x] Write docs\n  * [ ] parse output\nfooter";

    // By number: only the box character changes; every other byte is kept.
    let out = edit_checklist(body, &v(&["1"]), &[], &[]).unwrap();
    assert_eq!(
        out,
        body.replacen("- [ ] parse input", "- [x] parse input", 1)
    );
    // Comma list, and untick; an item already in the asked state is untouched.
    let out = edit_checklist(body, &v(&["1,3"]), &v(&["2"]), &[]).unwrap();
    assert_eq!(
        out,
        "Intro line\n- [x] parse input\n- [ ] Write docs\n  * [x] parse output\nfooter"
    );
    assert_eq!(edit_checklist(body, &v(&["2"]), &[], &[]).unwrap(), body);

    // Text: case-insensitive; an exact match wins over substrings; a unique
    // substring works; an ambiguous one is refused, naming the candidates.
    let out = edit_checklist(body, &v(&["WRITE DOCS"]), &[], &[]).unwrap();
    assert_eq!(out, body);
    let out = edit_checklist(body, &[], &v(&["docs"]), &[]).unwrap();
    assert!(out.contains("- [ ] Write docs"), "{out}");
    let e = edit_checklist(body, &v(&["parse"]), &[], &[]).unwrap_err();
    assert!(e.contains("matches 2 items") && e.contains("1, 3"), "{e}");
    let exact = "- [ ] parse\n- [ ] parse output\n";
    assert_eq!(
        edit_checklist(exact, &v(&["parse"]), &[], &[]).unwrap(),
        "- [x] parse\n- [ ] parse output\n"
    );

    // Refusals, before anything changes.
    for (c, u, want) in [
        (v(&["4"]), vec![], "no item 4"),
        (v(&["0"]), vec![], "no item 0"),
        (v(&["nothing like it"]), vec![], "matches no"),
        (v(&["1"]), v(&["1"]), "both checked and unchecked"),
    ] {
        let e = edit_checklist(body, &c, &u, &[]).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    let e = edit_checklist("just prose", &v(&["1"]), &[], &[]).unwrap_err();
    assert!(e.contains("no acceptance checklist"), "{e}");

    // Add appends unchecked items, to an empty field or after a missing newline.
    assert_eq!(
        edit_checklist("", &[], &[], &v(&["a", "b"])).unwrap(),
        "- [ ] a\n- [ ] b\n"
    );
    assert_eq!(
        edit_checklist("x", &[], &[], &v(&["- y"])).unwrap(),
        "x\n- [ ] - y\n"
    );
    assert!(edit_checklist("", &[], &[], &v(&["  "])).is_err());
    // CRLF lines keep their endings.
    assert_eq!(
        edit_checklist("- [ ] a\r\n- [ ] b\r\n", &v(&["b"]), &[], &[]).unwrap(),
        "- [ ] a\r\n- [x] b\r\n"
    );
}

#[test]
fn ephemeral_seeds_stay_local_through_pendant_and_sync_and_are_never_ready() {
    // aegis-w3k75d.13, lead ruling (a): ephemerals live in a sibling graph
    // that every read sees and no ledger carries.
    let mut local = backend();
    let shared = mk(&mut local, "shared", 1);
    let eph = engine::create(
        &mut local,
        &ctx(2),
        &CreateReq {
            title: "ephemeral".into(),
            ephemeral: true,
            ..CreateReq::default()
        },
    )
    .unwrap()
    .0
    .id;
    let snap = local.snapshot(None).unwrap();
    assert!(snap.get(&eph).unwrap().ephemeral);
    assert!(!snap.get(&shared).unwrap().ephemeral);

    // Both ready definitions exclude it, and agree.
    let mut by_model = engine::ready_by_model(&snap, &ctx(3).now);
    by_model.sort();
    assert_eq!(by_model, vec![shared.clone()]);
    assert_eq!(ready_ids(&local, 3), vec![shared.clone()]);

    // The pendant carries the shared seed (control) and not the ephemeral one.
    let base = seeds::pendant::read(&seeds::pendant::export(&local).unwrap())
        .unwrap()
        .snapshot;
    assert!(base.seeds.contains_key(&shared));
    assert!(!base.seeds.contains_key(&eph));

    // Sync with a second store: the ephemeral seed, even changed since the
    // base, is never pushed, and the local copy is never removed.
    let mut remote = QuipuBackend::in_memory("https://seeds.local/project/remote").unwrap();
    seeds::sync::import(&mut remote, &ctx(4), &base, None, true).unwrap();
    engine::update(
        &mut local,
        &ctx(5),
        std::slice::from_ref(&eph),
        &UpdateReq {
            title: Some("ephemeral, edited".into()),
            ..UpdateReq::default()
        },
    )
    .unwrap();
    let (_, lr, rr) = seeds::sync::sync(&base, &mut local, &mut remote, &ctx(6), false).unwrap();
    assert!(
        !lr.wrote && !rr.wrote,
        "nothing shared changed: {lr:?} {rr:?}"
    );
    assert!(!remote.snapshot(None).unwrap().seeds.contains_key(&eph));
    assert_eq!(
        local.snapshot(None).unwrap().get(&eph).unwrap().title,
        "ephemeral, edited"
    );

    // A replace-import (how a repo store follows its pendant after a pull)
    // makes the SHARED part equal the ledger and leaves ephemerals alone.
    seeds::sync::import(&mut local, &ctx(7), &base, None, true).unwrap();
    let after = local.snapshot(None).unwrap();
    assert!(after.get(&eph).unwrap().ephemeral);
    assert!(after.seeds.contains_key(&shared));
}

/// A positive control that graph-aware edits use indexed context reads, while
/// still refusing if successive reads observe different transaction heads.
struct IndexedContext {
    inner: QuipuBackend,
    reads: std::cell::Cell<usize>,
    skew: bool,
}
impl Backend for IndexedContext {
    fn snapshot(&self, _: Option<u64>) -> seeds::error::Result<seeds::model::Snapshot> {
        panic!("unexpected full-board read")
    }
    fn snapshot_items(&self, ids: &[String]) -> seeds::error::Result<seeds::model::Snapshot> {
        let n = self.reads.get() + 1;
        self.reads.set(n);
        let mut snap = self.inner.snapshot_items(ids)?;
        if self.skew && n > 1 {
            snap.tx += 1;
        }
        Ok(snap)
    }
    fn ready_ids(&self, at: Option<u64>) -> seeds::error::Result<Vec<String>> {
        self.inner.ready_ids(at)
    }
    fn claims_of(&self, id: &str) -> seeds::error::Result<Vec<(u64, seeds::backend::Claims)>> {
        self.inner.claims_of(id)
    }
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> seeds::error::Result<u64> {
        self.inner.commit(batch, ctx)
    }
}

#[test]
fn indexed_claim_checks_real_blockers_and_allows_closed_ones() {
    let mut inner = backend();
    let a = mk(&mut inner, "claim candidate", 1);
    let blocker = mk(&mut inner, "blocker", 2);
    engine::dep_add(&mut inner, &ctx(3), &a, &blocker, "blocks").unwrap();
    let mut b = IndexedContext {
        inner,
        reads: Default::default(),
        skew: false,
    };
    let req = UpdateReq {
        claim: true,
        ..Default::default()
    };
    let before = b.inner.snapshot(None).unwrap();
    let err = engine::update(&mut b, &ctx(4), std::slice::from_ref(&a), &req).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(err.message.contains(&blocker));
    assert_eq!(b.inner.snapshot(None).unwrap().seeds, before.seeds);
    assert_eq!(b.reads.get(), 2);
    engine::close(&mut b.inner, &ctx(5), &[blocker], Some("done"), false).unwrap();
    let (items, _) = engine::update(&mut b, &ctx(6), &[a], &req).unwrap();
    assert_eq!(items[0].status, "in_progress");
}

#[test]
fn indexed_dependency_walk_keeps_long_cycle_and_revision_guards() {
    let mut inner = backend();
    let ids: Vec<_> = (0..12)
        .map(|i| mk(&mut inner, &format!("chain {i}"), i))
        .collect();
    for pair in ids.windows(2) {
        engine::dep_add(&mut inner, &ctx(20), &pair[0], &pair[1], "blocks").unwrap();
    }
    let mut b = IndexedContext {
        inner,
        reads: Default::default(),
        skew: false,
    };
    let before = b.inner.snapshot(None).unwrap();
    let err = engine::dep_add(&mut b, &ctx(21), &ids[11], &ids[0], "blocks").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.message.contains("cycle"));
    assert_eq!(b.reads.get(), 11);
    assert_eq!(b.inner.snapshot(None).unwrap().seeds, before.seeds);
    b.skew = true;
    b.reads.set(0);
    let err = engine::dep_add(&mut b, &ctx(22), &ids[11], &ids[0], "blocks").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Conflict);
    assert!(err.message.contains("context changed"));
    assert_eq!(b.inner.snapshot(None).unwrap().seeds, before.seeds);
}

// ---- a sync too large for one request (aegis-w3k75d.15) ----

/// A remote with a request limit: it advertises `advertised` and really
/// refuses (TOO_LARGE, unsent) any batch whose estimate is over `limit`.
/// `fail_at` makes the n-th commit attempt (1-based) fail unwritten, as a
/// dropped connection would before the server read the request.
struct Capped<'a> {
    inner: &'a mut QuipuBackend,
    advertised: usize,
    limit: usize,
    fail_at: Option<usize>,
    attempts: usize,
    landed: Vec<usize>,
}

impl Backend for Capped<'_> {
    fn snapshot(&self, at: Option<u64>) -> seeds::error::Result<seeds::model::Snapshot> {
        self.inner.snapshot(at)
    }
    fn ready_ids(&self, at: Option<u64>) -> seeds::error::Result<Vec<String>> {
        self.inner.ready_ids(at)
    }
    fn claims_of(&self, id: &str) -> seeds::error::Result<Vec<(u64, seeds::backend::Claims)>> {
        self.inner.claims_of(id)
    }
    fn max_write_bytes(&self) -> Option<usize> {
        Some(self.advertised)
    }
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> seeds::error::Result<u64> {
        self.attempts += 1;
        if self.fail_at == Some(self.attempts) {
            return Err(seeds::error::SdError::new(
                ErrorKind::Unreachable,
                "connection refused",
            ));
        }
        if seeds::sync_batch::estimate_bytes(batch) > self.limit {
            return Err(seeds::error::SdError::refused(format!(
                "{}: test limit",
                seeds::backend::TOO_LARGE
            )));
        }
        let tx = self.inner.commit(batch, ctx)?;
        self.landed.push(
            batch.seeds.len()
                + batch.comments.len()
                + batch.delete_seeds.len()
                + batch.delete_comments.len(),
        );
        Ok(tx)
    }
}

/// A chain of `n` seeds, each blocked on the one before, each with a comment.
fn chain(b: &mut QuipuBackend, n: usize) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for i in 0..n {
        let id = mk(b, &format!("step {i}"), 1);
        if let Some(prev) = ids.last() {
            engine::dep_add(b, &ctx(2), &id, prev, "blocks").unwrap();
        }
        engine::comment_add(b, &ctx(3), &id, &format!("note on step {i}"), None).unwrap();
        ids.push(id);
    }
    ids
}

fn assert_same(local: &QuipuBackend, remote: &QuipuBackend) {
    let (l, r) = (
        local.snapshot(None).unwrap(),
        remote.snapshot(None).unwrap(),
    );
    assert_eq!(l.seeds, r.seeds, "remote holds exactly the local seeds");
    assert_eq!(
        l.comments, r.comments,
        "and exactly its comments, once each"
    );
}

#[test]
fn a_sync_over_the_request_limit_lands_in_ordered_batches() {
    let mut local = backend();
    chain(&mut local, 24);
    let whole = sync::plan_sync(&Default::default(), &local, &backend())
        .unwrap()
        .remote
        .0;
    let cap = seeds::sync_batch::estimate_bytes(&whole) / 5;
    let mut store = backend();
    let mut remote = Capped {
        inner: &mut store,
        advertised: cap,
        limit: cap,
        fail_at: None,
        attempts: 0,
        landed: Vec::new(),
    };
    let mut seen = Vec::new();
    sync::sync_with_progress(
        &Default::default(),
        &mut local,
        &mut remote,
        &ctx(9),
        false,
        &mut |b| seen.push(b),
    )
    .unwrap();
    assert!(remote.landed.len() >= 5, "split: {:?}", remote.landed);
    assert_eq!(remote.attempts, remote.landed.len(), "no batch was refused");
    assert_eq!(
        seen.len(),
        remote.landed.len(),
        "one progress line per batch"
    );
    assert_eq!(seen.last().unwrap().done, seen.last().unwrap().planned);
    assert_eq!(
        remote.landed.iter().sum::<usize>(),
        48,
        "24 seeds + 24 comments"
    );
    assert_same(&local, &store);
    // Nothing left to push.
    let base = local.snapshot(None).unwrap();
    let (_, _, rr) = sync::sync(
        &base,
        &mut local,
        &mut backend_ref(&mut store),
        &ctx(10),
        false,
    )
    .unwrap();
    assert!(!rr.wrote);
}

/// `&mut QuipuBackend` as an owned-looking backend, for a plain sync.
fn backend_ref(b: &mut QuipuBackend) -> Capped<'_> {
    Capped {
        inner: b,
        advertised: usize::MAX,
        limit: usize::MAX,
        fail_at: None,
        attempts: 0,
        landed: Vec::new(),
    }
}

#[test]
fn a_split_push_that_stops_part_way_resumes_without_duplicates() {
    let mut local = backend();
    chain(&mut local, 24);
    let local_before = local.snapshot(None).unwrap();
    let whole = sync::plan_sync(&Default::default(), &local, &backend())
        .unwrap()
        .remote
        .0;
    let cap = seeds::sync_batch::estimate_bytes(&whole) / 5;
    let mut store = backend();
    let mut remote = Capped {
        inner: &mut store,
        advertised: cap,
        limit: cap,
        fail_at: Some(3),
        attempts: 0,
        landed: Vec::new(),
    };
    let e = sync::sync(&Default::default(), &mut local, &mut remote, &ctx(9), false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unreachable, "the failure keeps its kind");
    assert!(e.message.contains("2 landed"), "{}", e.message);
    assert!(e.message.contains("re-run"), "{}", e.message);
    assert_eq!(remote.landed.len(), 2);
    let landed_first = remote.landed.iter().sum::<usize>();
    let after = local.snapshot(None).unwrap();
    assert_eq!(after.seeds, local_before.seeds, "local untouched");
    assert_eq!(after.comments, local_before.comments);

    // The same sync again: only what did not land is written.
    remote.fail_at = None;
    remote.landed.clear();
    sync::sync(
        &Default::default(),
        &mut local,
        &mut remote,
        &ctx(10),
        false,
    )
    .unwrap();
    assert_eq!(
        remote.landed.iter().sum::<usize>() + landed_first,
        48,
        "every item written exactly once across both runs"
    );
    assert_same(&local, &store);
}

#[test]
fn a_batch_refused_as_too_large_is_halved_and_still_lands() {
    let mut local = backend();
    chain(&mut local, 16);
    let whole = sync::plan_sync(&Default::default(), &local, &backend())
        .unwrap()
        .remote
        .0;
    let est = seeds::sync_batch::estimate_bytes(&whole);
    let mut store = backend();
    // The server's real limit is far below what it advertises.
    let mut remote = Capped {
        inner: &mut store,
        advertised: est / 2,
        limit: est / 7,
        fail_at: None,
        attempts: 0,
        landed: Vec::new(),
    };
    sync::sync(&Default::default(), &mut local, &mut remote, &ctx(9), false).unwrap();
    assert!(
        remote.attempts > remote.landed.len(),
        "some batch was halved"
    );
    assert_same(&local, &store);
}

#[test]
fn a_write_too_large_to_split_is_refused_and_writes_nothing() {
    let mut local = backend();
    chain(&mut local, 1);
    let mut store = backend();
    let mut remote = Capped {
        inner: &mut store,
        advertised: 64,
        limit: 64,
        fail_at: None,
        attempts: 0,
        landed: Vec::new(),
    };
    let e = sync::sync(&Default::default(), &mut local, &mut remote, &ctx(9), false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(seeds::backend::is_too_large(&e), "{}", e.message);
    assert!(store.snapshot(None).unwrap().seeds.is_empty());
}

#[test]
fn split_for_cap_orders_blockers_first_and_keeps_cycles_and_comments_together() {
    let mut local = backend();
    let ids = chain(&mut local, 12);
    let mut batch = sync::plan_sync(&Default::default(), &local, &backend())
        .unwrap()
        .remote
        .0;
    // Reverse the input so the order is the splitter's work, not the plan's.
    batch.seeds.reverse();
    // Close a cycle between the last two seeds (the splitter does not
    // validate; it must still keep a cycle in one batch).
    let (x, y) = (ids[10].clone(), ids[11].clone());
    for w in &mut batch.seeds {
        if w.seed.id == x {
            w.seed.blocked_on.insert(y.clone());
        }
    }
    let cap = seeds::sync_batch::estimate_bytes(&batch) / 6;
    let parts = seeds::sync_batch::split_for_cap(&batch, cap, usize::MAX);
    assert!(parts.len() >= 4, "{}", parts.len());
    let pos = |id: &str| {
        parts
            .iter()
            .position(|p| p.seeds.iter().any(|w| w.seed.id == id))
            .unwrap()
    };
    for w in &batch.seeds {
        for b in &w.seed.blocked_on {
            assert!(
                pos(b) <= pos(&w.seed.id),
                "{b} lands no later than {}",
                w.seed.id
            );
        }
    }
    assert_eq!(pos(&x), pos(&y), "the cycle travels together");
    for p in &parts {
        assert_eq!(p.source, batch.source);
        for c in &p.comments {
            assert!(
                p.seeds.iter().any(|w| w.seed.id == c.seed),
                "comment with its seed"
            );
        }
    }
    let mut all: Vec<String> = parts
        .iter()
        .flat_map(|p| p.seeds.iter().map(|w| w.seed.id.clone()))
        .collect();
    all.sort();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(all, want, "every seed exactly once");
    assert_eq!(
        parts.iter().map(|p| p.comments.len()).sum::<usize>(),
        batch.comments.len()
    );
    // A batch that fits is not split.
    assert_eq!(
        seeds::sync_batch::split_for_cap(&batch, usize::MAX, usize::MAX).len(),
        1
    );
}

/// A remote that nests at most `clauses` guard clauses per write, the
/// quipu-server abort this limit exists for (aegis-rq1afp).
struct ClauseCapped<'a> {
    inner: &'a mut QuipuBackend,
    clauses: usize,
    seen: Vec<usize>,
}

impl Backend for ClauseCapped<'_> {
    fn snapshot(&self, at: Option<u64>) -> seeds::error::Result<seeds::model::Snapshot> {
        self.inner.snapshot(at)
    }
    fn ready_ids(&self, at: Option<u64>) -> seeds::error::Result<Vec<String>> {
        self.inner.ready_ids(at)
    }
    fn claims_of(&self, id: &str) -> seeds::error::Result<Vec<(u64, seeds::backend::Claims)>> {
        self.inner.claims_of(id)
    }
    fn max_write_clauses(&self) -> Option<usize> {
        Some(self.clauses)
    }
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> seeds::error::Result<u64> {
        let n = seeds::sync_batch::clause_count(batch);
        assert!(
            n <= self.clauses,
            "a batch nesting {n} clauses reached the server"
        );
        self.seen.push(n);
        self.inner.commit(batch, ctx)
    }
}

#[test]
fn a_sync_with_too_many_guard_clauses_is_split_under_the_clause_limit() {
    let mut local = backend();
    chain(&mut local, 20); // 20 new seeds (2 clauses each) + 20 comments = 60
    let mut store = backend();
    let mut remote = ClauseCapped {
        inner: &mut store,
        clauses: 9,
        seen: Vec::new(),
    };
    sync::sync(&Default::default(), &mut local, &mut remote, &ctx(9), false).unwrap();
    assert!(remote.seen.len() >= 7, "{:?}", remote.seen);
    assert_eq!(remote.seen.iter().sum::<usize>(), 60);
    assert_same(&local, &store);
}

#[test]
fn a_seed_with_more_comments_than_the_clause_limit_still_syncs() {
    // One seed carrying more comments than one write may guard: aegis-h7xuql
    // has 558 comments against the 500-clause default.
    let mut local = backend();
    let id = mk(&mut local, "long thread", 1);
    for i in 0..30 {
        engine::comment_add(&mut local, &ctx(3), &id, &format!("reply {i}"), None).unwrap();
    }
    let batch = sync::plan_sync(&Default::default(), &local, &backend())
        .unwrap()
        .remote
        .0;
    let parts = seeds::sync_batch::split_for_cap(&batch, usize::MAX, 9);
    assert!(
        parts
            .iter()
            .all(|p| seeds::sync_batch::clause_count(p) <= 9),
        "{:?}",
        parts
            .iter()
            .map(seeds::sync_batch::clause_count)
            .collect::<Vec<_>>()
    );
    assert!(
        parts[0].seeds.iter().any(|w| w.seed.id == id) && !parts[0].comments.is_empty(),
        "the seed lands first, with as many of its comments as fit"
    );
    let mut store = backend();
    let mut remote = ClauseCapped {
        inner: &mut store,
        clauses: 9,
        seen: Vec::new(),
    };
    sync::sync(&Default::default(), &mut local, &mut remote, &ctx(9), false).unwrap();
    assert_eq!(remote.seen.iter().sum::<usize>(), 32, "{:?}", remote.seen);
    assert_same(&local, &store);
}

// A push must never obtain the complete remote snapshot. It must also filter
// a backend's permitted superset of named items before planning deletions.
struct PushRemote {
    inner: QuipuBackend,
    reads: std::cell::RefCell<Vec<Vec<String>>>,
}
impl Backend for PushRemote {
    fn snapshot(&self, _: Option<u64>) -> seeds::error::Result<seeds::model::Snapshot> {
        panic!("push read the entire remote")
    }
    fn snapshot_items(&self, ids: &[String]) -> seeds::error::Result<seeds::model::Snapshot> {
        self.reads.borrow_mut().push(ids.to_vec());
        self.inner.snapshot(None) // deliberately a superset
    }
    fn ready_ids(&self, at: Option<u64>) -> seeds::error::Result<Vec<String>> {
        self.inner.ready_ids(at)
    }
    fn claims_of(&self, id: &str) -> seeds::error::Result<Vec<(u64, seeds::backend::Claims)>> {
        self.inner.claims_of(id)
    }
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> seeds::error::Result<u64> {
        self.inner.commit(batch, ctx)
    }
}

#[test]
fn push_only_reads_changed_items_and_leaves_unseen_remote_changes_for_two_way_sync() {
    use seeds::sync;
    let mut local = backend();
    let a = mk(&mut local, "touched", 1);
    let b = mk(&mut local, "untouched", 2);
    let mut remote = PushRemote {
        inner: backend(),
        reads: Default::default(),
    };
    sync::import(
        &mut remote.inner,
        &ctx(3),
        &local.snapshot(None).unwrap(),
        None,
        true,
    )
    .unwrap();
    let base = local.snapshot(None).unwrap();
    engine::close(
        &mut remote.inner,
        &ctx(4),
        std::slice::from_ref(&b),
        Some("remote"),
        false,
    )
    .unwrap();
    let remote_only = mk(&mut remote.inner, "remote only", 5);
    engine::update(
        &mut local,
        &ctx(6),
        std::slice::from_ref(&a),
        &UpdateReq {
            priority: Some(0.to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    let p = sync::plan_push(&base, &local, &remote).unwrap();
    assert_eq!(*remote.reads.borrow(), vec![vec![a.clone()]]);
    assert!(p.remote.1.removed.is_empty());
    let (new_base, _, rr) =
        sync::apply_sync_plan(p, &mut local, &mut remote, &ctx(7), false, &mut |_| {}).unwrap();
    assert!(rr.wrote);
    assert_eq!(
        local.snapshot(None).unwrap().get(&b).unwrap().status,
        "open"
    );
    assert!(!new_base.seeds.contains_key(&remote_only));
    // No local delta: zero remote reads, even though the remote is ahead.
    remote.reads.borrow_mut().clear();
    let p = sync::plan_push(&new_base, &local, &remote).unwrap();
    assert!(p.remote.0.is_empty() && p.local.0.is_empty());
    assert!(remote.reads.borrow().is_empty());
    sync::sync(&new_base, &mut local, &mut remote.inner, &ctx(8), false).unwrap();
    assert_same(&local, &remote.inner);
    assert_eq!(
        local.snapshot(None).unwrap().get(&b).unwrap().status,
        "closed"
    );
}

#[test]
fn push_only_preserves_touched_field_conflicts_and_revision_races() {
    use seeds::sync;
    let mut local = backend();
    let a = mk(&mut local, "touched", 1);
    let mut remote = PushRemote {
        inner: backend(),
        reads: Default::default(),
    };
    sync::import(
        &mut remote.inner,
        &ctx(2),
        &local.snapshot(None).unwrap(),
        None,
        true,
    )
    .unwrap();
    let base = local.snapshot(None).unwrap();
    for (b, priority) in [(&mut local, 0), (&mut remote.inner, 3)] {
        engine::update(
            b,
            &ctx(3),
            std::slice::from_ref(&a),
            &UpdateReq {
                priority: Some(priority.to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    }
    let p = sync::plan_push(&base, &local, &remote).unwrap();
    assert_eq!(p.refusal(false).unwrap().kind, ErrorKind::Conflict);
    assert_eq!(
        sync::apply_sync_plan(p, &mut local, &mut remote, &ctx(4), false, &mut |_| {})
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
    engine::update(
        &mut remote.inner,
        &ctx(5),
        std::slice::from_ref(&a),
        &UpdateReq {
            priority: Some(2.to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    let p = sync::plan_push(&base, &local, &remote).unwrap();
    assert!(p.conflicts.is_empty());
    engine::update(
        &mut remote.inner,
        &ctx(6),
        std::slice::from_ref(&a),
        &UpdateReq {
            priority: Some(1.to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    let before = local.snapshot(None).unwrap();
    assert_eq!(
        sync::apply_sync_plan(p, &mut local, &mut remote, &ctx(7), false, &mut |_| {})
            .unwrap_err()
            .kind,
        ErrorKind::Conflict
    );
    assert_eq!(local.snapshot(None).unwrap().seeds, before.seeds);
}

#[test]
fn push_only_merges_comment_only_changes_and_retains_removal_guard() {
    use seeds::sync;
    let mut local = backend();
    let a = mk(&mut local, "comments", 1);
    let mut remote = PushRemote {
        inner: backend(),
        reads: Default::default(),
    };
    sync::import(
        &mut remote.inner,
        &ctx(2),
        &local.snapshot(None).unwrap(),
        None,
        true,
    )
    .unwrap();
    let base = local.snapshot(None).unwrap();
    engine::comment_add(&mut local, &ctx(3), &a, "local", None).unwrap();
    engine::comment_add(&mut remote.inner, &ctx(4), &a, "remote", None).unwrap();
    let p = sync::plan_push(&base, &local, &remote).unwrap();
    assert!(p.conflicts.is_empty());
    let (base, _, _) =
        sync::apply_sync_plan(p, &mut local, &mut remote, &ctx(5), false, &mut |_| {}).unwrap();
    assert_eq!(local.snapshot(None).unwrap().comments.len(), 2);
    assert_same(&local, &remote.inner);
    local
        .commit(
            &WriteBatch {
                delete_seeds: vec![(a.clone(), base.get(&a).unwrap().revision)],
                ..Default::default()
            },
            &ctx(6),
        )
        .unwrap();
    let p = sync::plan_push(&base, &local, &remote).unwrap();
    assert_eq!(p.refusal(false).unwrap().kind, ErrorKind::Refused);
    assert_eq!(p.remote.1.removed, vec![a]);
    sync::apply_sync_plan(p, &mut local, &mut remote, &ctx(7), true, &mut |_| {}).unwrap();
    assert_same(&local, &remote.inner);
}

#[test]
fn push_only_resumes_a_partial_split_without_rewriting_landed_items() {
    use seeds::sync;
    let mut local = backend();
    chain(&mut local, 12);
    let base = seeds::model::Snapshot::default();
    let mut target = backend();
    let batch = sync::plan_push(&base, &local, &target).unwrap().remote.0;
    let limit = seeds::sync_batch::estimate_bytes(&batch) / 4;
    let mut remote = Capped {
        inner: &mut target,
        advertised: limit,
        limit,
        fail_at: Some(2),
        attempts: 0,
        landed: vec![],
    };
    let p = sync::plan_push(&base, &local, &remote).unwrap();
    let err =
        sync::apply_sync_plan(p, &mut local, &mut remote, &ctx(9), false, &mut |_| {}).unwrap_err();
    assert!(err.message.contains("1 landed"), "{}", err.message);
    let landed = remote.inner.snapshot(None).unwrap().seeds.len();
    assert!(landed > 0 && landed < 12);
    remote.fail_at = None;
    let p = sync::plan_push(&base, &local, &remote).unwrap();
    assert_eq!(p.remote.1.created.len(), 12 - landed);
    assert!(p.remote.1.updated.is_empty());
    sync::apply_sync_plan(p, &mut local, &mut remote, &ctx(10), false, &mut |_| {}).unwrap();
    assert_same(&local, remote.inner);
}

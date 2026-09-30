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

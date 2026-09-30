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

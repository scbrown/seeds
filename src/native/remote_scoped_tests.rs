//! aegis-aane52 S2: every command routed to scoped reads prints exactly what
//! it printed from a whole snapshot, on a real quipu server.
//!
//! The same graph is read two ways: through [`RemoteBackend`] (scoped reads)
//! and through [`Whole`], the same backend with every scoped read turned back
//! into a whole snapshot (the S1 behaviour). Reads compare on one graph;
//! writes run on two identical graphs, one each way, and compare their
//! results and the graphs they leave.

use std::collections::BTreeSet;

use super::tests::quipu_server;
use super::RemoteBackend;
use crate::backend::{Backend, Ctx, WriteBatch};
use crate::engine::{self, CountReq, CreateReq, Filter, ListReq, ReadyReq, UpdateReq};
use crate::error::{Result, SdError};
use crate::model::Snapshot;
use crate::output;
use crate::quipu_backend::QuipuBackend;
use crate::vocab::{self, term};

/// A backend with no scoped reads: the whole-snapshot reference.
struct Whole<'a>(&'a mut RemoteBackend);

impl Backend for Whole<'_> {
    fn snapshot(&self, at: Option<u64>) -> Result<Snapshot> {
        self.0.snapshot(at)
    }
    fn ready_ids(&self, at: Option<u64>) -> Result<Vec<String>> {
        self.0.ready_ids(at)
    }
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> Result<u64> {
        self.0.commit(batch, ctx)
    }
    fn claims_of(&self, id: &str) -> Result<Vec<(u64, crate::backend::Claims)>> {
        self.0.claims_of(id)
    }
    fn max_write_bytes(&self) -> Option<usize> {
        self.0.max_write_bytes()
    }
    fn max_write_clauses(&self) -> Option<usize> {
        self.0.max_write_clauses()
    }
}

fn ctx() -> Ctx {
    Ctx {
        now: "2026-10-07T12:00:00Z".into(),
        actor: "tester".into(),
        prefix: "eq".into(),
        claims: Default::default(),
    }
}

fn err(e: &SdError) -> String {
    format!("ERR {:?}: {}", e.kind, e.message)
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// A board with every shape the routed commands read: statuses, types,
/// priorities, assignees, labels, an epic with children, `blocks` chains
/// (some blockers closed), related and discovered-from edges, comments,
/// deferred seeds (one still deferred, one past its date), a tombstone, a
/// dangling edge, and ephemeral seeds; built locally, then copied.
fn corpus() -> (Snapshot, Vec<String>) {
    let mut b = QuipuBackend::in_memory("https://seeds.local/project/corpus").unwrap();
    let c = ctx();
    let mk = |b: &mut QuipuBackend, id: &str, req: CreateReq| {
        let mut c = c.clone();
        c.prefix = id.into();
        let (s, _) = engine::create(b, &c, &req).unwrap();
        s.id
    };
    let types = ["task", "bug", "feature", "chore", "docs"];
    let people = ["ian", "ellie", "alan"];
    let mut all = Vec::new();
    for n in 0..60 {
        let req = CreateReq {
            title: format!("seed {n}"),
            issue_type: Some(types[n % types.len()].into()),
            priority: Some((n % 5).to_string()),
            assignee: (n % 4 != 0).then(|| people[n % 3].to_string()),
            labels: if n == 1 {
                vec!["infra".into(), "cutover".into(), "{ ?s".into(), "?s".into()]
            } else {
                match n % 3 {
                    0 => vec!["infra".into()],
                    1 => vec!["infra".into(), "cutover".into()],
                    _ => vec![],
                }
            },
            ..CreateReq::default()
        };
        all.push(mk(&mut b, &format!("s{n:02}"), req));
    }
    let epic = mk(
        &mut b,
        "epic",
        CreateReq {
            title: "the epic".into(),
            issue_type: Some("epic".into()),
            ..CreateReq::default()
        },
    );
    for n in 0..5 {
        let child = mk(
            &mut b,
            "child",
            CreateReq {
                title: format!("child {n}"),
                parent: Some(epic.clone()),
                ..CreateReq::default()
            },
        );
        all.push(child);
    }
    // blocks chains, related, discovered-from
    for n in (1..40).step_by(3) {
        engine::dep_add(&mut b, &c, &all[n], &all[n - 1], "blocks").unwrap();
    }
    for n in (2..40).step_by(7) {
        engine::dep_add(&mut b, &c, &all[n], &all[n + 10], "related").unwrap();
        engine::dep_add(&mut b, &c, &all[n + 1], &all[n + 20], "discovered-from").unwrap();
    }
    // many dependents on one seed
    for n in 41..55 {
        engine::dep_add(&mut b, &c, &all[n], &all[3], "related").unwrap();
    }
    // comments
    for n in (0..60).step_by(4) {
        for k in 0..(n % 3 + 1) {
            engine::comment_add(&mut b, &c, &all[n], &format!("note {k} on {n}"), None).unwrap();
        }
    }
    // statuses: close some (some of them blockers), in progress, blocked, defer
    for n in (0..40).step_by(6) {
        engine::close(&mut b, &c, &[all[n].clone()], Some("done"), true).unwrap();
    }
    let set = |b: &mut QuipuBackend, id: &str, req: UpdateReq| {
        engine::update(b, &c, &[id.to_string()], &req).unwrap();
    };
    set(
        &mut b,
        &all[5],
        UpdateReq {
            status: Some("in_progress".into()),
            ..Default::default()
        },
    );
    set(
        &mut b,
        &all[8],
        UpdateReq {
            status: Some("blocked".into()),
            ..Default::default()
        },
    );
    set(
        &mut b,
        &all[9],
        UpdateReq {
            status: Some("hooked".into()),
            ..Default::default()
        },
    );
    set(
        &mut b,
        &all[11],
        UpdateReq {
            defer: Some("2099-01-01".into()),
            status: Some("deferred".into()),
            ..Default::default()
        },
    );
    set(
        &mut b,
        &all[14],
        UpdateReq {
            defer: Some("2020-01-01".into()),
            status: Some("deferred".into()),
            ..Default::default()
        },
    );
    set(
        &mut b,
        &all[17],
        UpdateReq {
            defer: Some("2099-01-01".into()),
            ..Default::default()
        },
    );
    // a deferred seed blocked by an open one
    engine::dep_add(&mut b, &c, &all[14], &all[16], "blocks").unwrap();
    engine::delete(&mut b, &c, &[all[20].clone()], "gone", false, true, false).unwrap();
    // ephemeral seeds, one depending on a shared seed, one with a comment
    let e1 = mk(
        &mut b,
        "eph",
        CreateReq {
            title: "ephemeral one".into(),
            ephemeral: true,
            labels: vec!["infra".into()],
            assignee: Some("ian".into()),
            deps: vec![all[2].clone()],
            ..CreateReq::default()
        },
    );
    engine::comment_add(&mut b, &c, &e1, "on an ephemeral seed", None).unwrap();
    all.push(epic);
    all.push(e1);
    let mut snap = b.snapshot(None).unwrap();
    // Nonzero lead times cross a century leap day and month boundary. This
    // checks the remote mean against the core's independent calendar model.
    for (index, created, closed) in [
        (0, "1999-12-31T23:00:00Z", "2000-03-01T00:00:01.999Z"),
        (6, "1900-02-28T23:00:00Z", "1900-03-01T01:00:00Z"),
        (12, "1999-12-31T23:00:00-03:00", "2000-01-01T00:00:00+03:00"),
        (24, "2026-10-07T01:00:00", "2026-10-07T02:10:00"),
    ] {
        let seed = snap.seeds.get_mut(&all[index]).unwrap();
        seed.created_at = created.into();
        seed.closed_at = Some(closed.into());
    }
    // a dangling edge: a dependency on a seed that is not in the ledger
    if let Some(s) = snap.seeds.get_mut(&all[26]) {
        s.blocked_on.insert("ghost-1".into());
    }
    (snap, all)
}

/// Where [`corpus`] puts the epic and the ephemeral seed among its ids.
const EPIC: usize = 65;
const EPH: usize = 66;

/// Copy `snap` into `graph` on the server, then add seeds some other writer
/// stored irregularly (each reads with a default in `Seed::from_facts`).
fn load(base: &str, graph: &str, snap: &Snapshot) -> RemoteBackend {
    let mut r = RemoteBackend::connect(base, graph, None, None, &[]).unwrap();
    r.subject_page = 7;
    let batch = WriteBatch {
        seeds: snap
            .seeds
            .values()
            .map(|s| crate::backend::SeedWrite {
                seed: s.clone(),
                expected_revision: None,
            })
            .collect(),
        comments: snap.comments.clone(),
        source: "seeds:test".into(),
        ..WriteBatch::default()
    };
    let mut c = ctx();
    c.now = "2026-10-07T11:00:00Z".into();
    let dangling = snap
        .seeds
        .values()
        .any(|s| s.blocked_on.contains("ghost-1"));
    // The dangling edge cannot be written through a checked commit; write
    // the rest, then add it raw below.
    let mut batch = batch;
    for w in &mut batch.seeds {
        w.seed.blocked_on.remove("ghost-1");
    }
    crate::sync_batch::commit_capped(&mut r, &batch, &c, &mut |_| {}).unwrap();
    let a = term::work_item();
    let id = term::identifier();
    let name = term::name();
    let st = term::status();
    let pr = term::priority();
    let ag = term::assigned_to();
    let ty = term::issue_type();
    let it = |s: &str| vocab::item_iri(s);
    let mut nt = String::new();
    // no status, no priority, no type: open, P2, task
    nt += &format!(
        "<{}> a <{a}> ; <{id}> \"irr-none\" ; <{name}> \"irregular none\" .\n",
        it("irr-none")
    );
    // a status no build writes: shown by a default listing
    nt += &format!("<{}> a <{a}> ; <{id}> \"irr-weird\" ; <{name}> \"weird\" ; <{st}> \"weird\" ; <{ag}> <{}> .\n", it("irr-weird"), vocab::principal_iri("ian"));
    // a language-tagged status: not read, so open
    nt += &format!("<{}> a <{a}> ; <{id}> \"irr-lang\" ; <{name}> \"lang\" ; <{st}> \"closed\"@en ; <{pr}> 1 .\n", it("irr-lang"));
    // an assignee stored as a plain string
    nt += &format!("<{}> a <{a}> ; <{id}> \"irr-str\" ; <{name}> \"string assignee\" ; <{st}> \"open\" ; <{ag}> \"ian\" ; <{ty}> \"bug\" .\n", it("irr-str"));
    // a priority out of range: the default
    nt += &format!("<{}> a <{a}> ; <{id}> \"irr-prio\" ; <{name}> \"big priority\" ; <{st}> \"open\" ; <{pr}> 300 .\n", it("irr-prio"));
    if dangling {
        let owner = snap
            .seeds
            .values()
            .find(|s| s.blocked_on.contains("ghost-1"))
            .unwrap();
        nt += &format!(
            "<{}> <{}> <{}> .\n",
            it(&owner.id),
            term::blocked_on(),
            it("ghost-1")
        );
    }
    // Raw, as another writer would: no checks, no report needed.
    let q = format!("INSERT DATA {{ GRAPH <{graph}> {{ {nt} }} }}");
    r.post(
        "/update",
        "application/x-www-form-urlencoded",
        &format!("update={}", super::form_encode(&q)),
        true,
    )
    .unwrap();
    r
}

/// Everything a read command prints, both ways.
fn reads(b: &dyn Backend, c: &Ctx, n: &[String], all_ids: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("ledger counts {:?}", b.ledger_counts()));
    for req in [
        engine::StatsReq::default(),
        engine::StatsReq {
            by_type: true,
            by_priority: true,
            by_assignee: true,
            by_label: true,
            activity_hours: Some(48),
        },
    ] {
        let stats = engine::stats(b, c, req, None).map(|mut stats| {
            // Decimal AVG and f64 summation may differ at the last bit.
            // Compare the mean to one nanohour; all other fields stay exact.
            stats.average_lead_time_hours = (stats.average_lead_time_hours * 1e9).round() / 1e9;
            stats
        });
        out.push(format!("stats {req:?} {stats:?}"));
    }
    for term in [
        "seed",
        "note",
        "ephemeral",
        "irregular",
        "no such matching text",
        "SEED 1",
    ] {
        for (full, all, offset, reverse, sort) in [
            (false, false, 0, false, "priority"),
            (false, true, 3, true, "title"),
            (true, false, 0, false, "priority"),
            (true, true, 3, true, "title"),
        ] {
            let req = engine::SearchReq {
                query: term.into(),
                full,
                all,
                offset,
                reverse,
                sort: Some(sort.into()),
                limit: Some(5),
                ..Default::default()
            };
            out.push(match engine::search(b, &req, None) {
                Ok(result) => format!(
                    "search {req:?} hidden={} {}",
                    result.hidden_closed,
                    output::search_json(&result)
                ),
                Err(e) => format!("search {req:?} {}", err(&e)),
            });
        }
    }
    let mut shows: Vec<Vec<String>> = all_ids.iter().map(|id| vec![id.clone()]).collect();
    shows.push(ids(&[n[3].as_str(), n[1].as_str(), n[EPH].as_str()]));
    shows.push(ids(&[n[0].as_str(), "nope"]));
    for v in shows {
        out.push(match engine::show(b, &v, None) {
            Ok(views) => format!(
                "show {v:?}\n{}\n{}",
                output::show_json(&views),
                views
                    .iter()
                    .map(output::view_text)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            ),
            Err(e) => format!("show {v:?} {}", err(&e)),
        });
    }
    let f = |g: &dyn Fn(&mut Filter)| {
        let mut f = Filter::default();
        g(&mut f);
        f
    };
    let filters: Vec<Filter> = vec![
        Filter::default(),
        f(&|f: &mut Filter| f.status = Some("open".into())),
        f(&|f: &mut Filter| f.status = Some("closed".into())),
        f(&|f: &mut Filter| f.status = Some("deferred".into())),
        f(&|f: &mut Filter| f.status = Some("in_progress".into())),
        f(&|f: &mut Filter| f.status = Some("tombstone".into())),
        f(&|f: &mut Filter| f.assignee = Some("ian".into())),
        f(&|f: &mut Filter| {
            f.assignee = Some("ellie".into());
            f.labels = vec!["{ ?s".into(), "?s".into()];
        }),
        f(&|f: &mut Filter| f.unassigned = true),
        f(&|f: &mut Filter| f.labels = vec!["infra".into()]),
        f(&|f: &mut Filter| f.labels = vec!["infra".into(), "cutover".into()]),
        f(&|f: &mut Filter| f.labels_any = vec!["cutover".into()]),
        f(&|f: &mut Filter| f.issue_type = Some("task".into())),
        f(&|f: &mut Filter| f.issue_type = Some("bug".into())),
        f(&|f: &mut Filter| f.priority = Some("2".into())),
        f(&|f: &mut Filter| f.priority = Some("1".into())),
        f(&|f: &mut Filter| f.parent = Some(n[EPIC].as_str().into())),
        f(&|f: &mut Filter| f.ids = ids(&[n[1].as_str(), n[2].as_str(), n[20].as_str(), "nope"])),
        f(&|f: &mut Filter| f.title_contains = Some("seed 1".into())),
        f(&|f: &mut Filter| {
            f.status = Some("open".into());
            f.assignee = Some("ian".into());
            f.labels = vec!["infra".into()];
        }),
    ];
    for filter in &filters {
        for (all, deferred, limit, offset, sort, reverse, defer_until_present) in [
            (false, false, None, 0, None, false, false),
            (true, false, Some(0), 0, Some("created"), false, false),
            (false, true, Some(5), 3, Some("id"), true, false),
            (true, true, Some(0), 0, Some("id"), false, true),
        ] {
            let req = ListReq {
                filter: filter.clone(),
                all,
                limit,
                sort: sort.map(str::to_string),
                offset,
                reverse,
                deferred,
                defer_until_present,
            };
            out.push(match engine::list(b, &req, None) {
                Ok(p) => format!(
                    "list {req:?}\n{}\n{}",
                    output::list_json(&p),
                    output::page_text(&p, "matching")
                ),
                Err(e) => format!("list {req:?} {}", err(&e)),
            });
        }
        for (include_closed, by) in [
            (false, None),
            (true, Some("status")),
            (false, Some("label")),
        ] {
            let req = CountReq {
                filter: filter.clone(),
                by: by.map(str::to_string),
                include_closed,
            };
            out.push(match engine::count(b, &req, None) {
                Ok(n) => format!("count {req:?} {n:?}"),
                Err(e) => format!("count {req:?} {}", err(&e)),
            });
        }
        if filter.status.is_none() && filter.ids.is_empty() {
            for (include_deferred, limit, sort, recursive) in [
                (false, None, None, false),
                (true, Some(4), Some("hybrid"), false),
                (false, None, Some("oldest"), filter.parent.is_some()),
            ] {
                let req = ReadyReq {
                    filter: filter.clone(),
                    limit,
                    sort: sort.map(str::to_string),
                    include_deferred,
                    recursive,
                };
                out.push(match engine::ready(b, c, &req, None) {
                    Ok(p) => format!(
                        "ready {req:?}\n{}\n{}",
                        output::seeds_json(&p.issues),
                        output::page_text(&p, "ready")
                    ),
                    Err(e) => format!("ready {req:?} {}", err(&e)),
                });
            }
        }
    }
    out
}

/// One write, run both ways; its printed result with the transaction left
/// out (the two graphs commit at different transactions).
fn write(op: usize, b: &mut dyn Backend, c: &Ctx, n: &[String]) -> String {
    let claim = UpdateReq {
        claim: true,
        ..UpdateReq::default()
    };
    let r = |x: Result<String>| x.unwrap_or_else(|e| err(&e));
    match op {
        0 => r(engine::update(b, c, &ids(&[n[32].as_str()]), &claim).map(|(s, _)| format!("{s:?}"))),
        // blocked by an open seed
        1 => r(engine::update(b, c, &ids(&[n[4].as_str()]), &claim).map(|(s, _)| format!("{s:?}"))),
        // claimed by someone else
        2 => {
            let mut other = c.clone();
            other.actor = "rival".into();
            r(engine::update(b, &other, &ids(&[n[32].as_str()]), &claim)
                .map(|(s, _)| format!("{s:?}")))
        }
        // the claimer again: unchanged
        3 => r(engine::update(b, c, &ids(&[n[32].as_str()]), &claim).map(|(s, _)| format!("{s:?}"))),
        // blocked by a closed seed only, assigned to the claimer: claimable
        4 => {
            let mut ellie = c.clone();
            ellie.actor = "ellie".into();
            r(engine::update(b, &ellie, &ids(&[n[7].as_str()]), &claim)
                .map(|(s, _)| format!("{s:?}")))
        }
        5 => r(engine::update(
            b,
            c,
            &ids(&[n[10].as_str(), n[EPH].as_str()]),
            &UpdateReq {
                title: Some("retitled".into()),
                add_labels: vec!["new".into()],
                transition_comment: Some("moved".into()),
                ..UpdateReq::default()
            },
        )
        .map(|(s, _)| format!("{s:?}"))),
        6 => r(engine::update(b, c, &ids(&["nope"]), &claim).map(|(s, _)| format!("{s:?}"))),
        // close: an open blocker refuses, force passes, already closed warns
        7 => r(engine::close(
            b,
            c,
            &ids(&[n[13].as_str(), n[16].as_str()]),
            Some("x"),
            false,
        )
        .map(|(s, _, w)| format!("{s:?} {w:?}"))),
        8 => r(engine::close_as(
            b,
            c,
            &ids(&[n[13].as_str(), n[12].as_str(), n[4].as_str()]),
            Some("x"),
            None,
            true,
            Some("closing"),
        )
        .map(|(s, _, w)| format!("{s:?} {w:?}"))),
        9 => r(engine::close(b, c, &ids(&[n[12].as_str()]), None, false)
            .map(|(s, _, w)| format!("{s:?} {w:?}"))),
        10 => r(
            engine::close(b, c, &ids(&[n[26].as_str()]), Some("dangling"), false)
                .map(|(s, _, w)| format!("{s:?} {w:?}")),
        ),
        // comments
        11 => {
            r(engine::comment_add(b, c, n[8].as_str(), "first", None)
                .map(|(x, _)| format!("{x:?}")))
        }
        12 => r(engine::comment_add(b, c, n[4].as_str(), "another", None)
            .map(|(x, _)| format!("{x:?}"))),
        13 => r(
            engine::comment_add(b, c, n[EPH].as_str(), "eph again", None)
                .map(|(x, _)| format!("{x:?}")),
        ),
        14 => r(engine::comment_add(b, c, "nope", "x", None).map(|(x, _)| format!("{x:?}"))),
        // dependencies: a cycle refuses; new edges; a missing or deleted end
        15 => r(
            engine::dep_add(b, c, n[0].as_str(), n[1].as_str(), "blocks")
                .map(|d| format!("{:?}", (d.action, d.dep_type))),
        ),
        16 => r(
            engine::dep_add(b, c, n[33].as_str(), n[35].as_str(), "blocks")
                .map(|d| format!("{:?}", (d.action, d.dep_type))),
        ),
        17 => r(
            engine::dep_add(b, c, n[33].as_str(), n[35].as_str(), "blocks")
                .map(|d| format!("{:?}", (d.action, d.dep_type))),
        ),
        18 => r(
            engine::dep_add(b, c, n[32].as_str(), n[EPH].as_str(), "related")
                .map(|d| format!("{:?}", (d.action, d.dep_type))),
        ),
        19 => r(engine::dep_add(b, c, n[33].as_str(), "nope", "blocks")
            .map(|d| format!("{:?}", (d.action, d.dep_type)))),
        20 => r(
            engine::dep_add(b, c, n[33].as_str(), n[20].as_str(), "related")
                .map(|d| format!("{:?}", (d.action, d.dep_type))),
        ),
        _ => unreachable!(),
    }
}

const WRITES: usize = 21;

#[test]
fn scoped_reads_print_exactly_what_a_whole_snapshot_prints() {
    let Some(server) = quipu_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let (snap, n) = corpus();
    let mut a = load(&server.base, "https://seeds.local/project/scoped-a", &snap);
    let mut b = load(&server.base, "https://seeds.local/project/scoped-b", &snap);
    let c = ctx();
    let mut all_ids: Vec<String> = a.snapshot(None).unwrap().seeds.keys().cloned().collect();
    all_ids.push("nope".into());
    let ids_seen: BTreeSet<&str> = all_ids.iter().map(String::as_str).collect();
    for want in [
        "irr-none",
        "irr-weird",
        "irr-lang",
        "irr-str",
        "irr-prio",
        &n[EPH],
        &n[20],
    ] {
        assert!(ids_seen.contains(want), "the corpus lacks {want}");
    }

    // Reads, one graph, both ways.
    let scoped = reads(&a, &c, &n, &all_ids);
    let whole = reads(&Whole(&mut a), &c, &n, &all_ids);
    assert_eq!(scoped.len(), whole.len());
    for (s, w) in scoped.iter().zip(&whole) {
        assert_eq!(s, w, "a scoped read printed something else");
    }
    // The comparison saw real answers, not only errors and empty pages.
    let shown = scoped
        .iter()
        .filter(|s| s.contains("\"comments\":[{"))
        .count();
    assert!(shown > 10, "only {shown} shows carried comments");
    assert!(scoped
        .iter()
        .any(|s| s.starts_with("list") && s.contains("irr-none")));
    assert!(scoped
        .iter()
        .any(|s| s.starts_with("ready") && s.contains(n[EPIC].as_str())));
    assert!(scoped
        .iter()
        .any(|s| s.starts_with("list") && s.contains("irr-weird")));

    // Writes, two identical graphs, one each way.
    for op in 0..WRITES {
        let sw = write(op, &mut a, &c, &n);
        let ww = write(op, &mut Whole(&mut b), &c, &n);
        assert_eq!(sw, ww, "write {op} returned something else");
        // The claims under test: one lands, one names its holder, one is blocked.
        match op {
            0 | 4 => assert!(sw.contains("in_progress"), "write {op}: {sw}"),
            1 => assert!(sw.contains("is blocked by"), "write {op}: {sw}"),
            2 => assert!(sw.contains("already claimed by tester"), "write {op}: {sw}"),
            7 => assert!(sw.contains("is blocked by"), "write {op}: {sw}"),
            16 => assert!(sw.contains("added"), "write {op}: {sw}"),
            _ => {}
        }
        let (sa, sb) = (a.snapshot(None).unwrap(), b.snapshot(None).unwrap());
        assert_eq!(
            format!("{:?}", sa.seeds),
            format!("{:?}", sb.seeds),
            "after write {op}"
        );
        assert_eq!(
            format!("{:?}", sa.comments),
            format!("{:?}", sb.comments),
            "after write {op}"
        );
    }
    // And the reads still agree on the graph the writes left.
    let scoped = reads(&a, &c, &n, &all_ids);
    let whole = reads(&Whole(&mut b), &c, &n, &all_ids);
    for (s, w) in scoped.iter().zip(&whole) {
        assert_eq!(
            s, w,
            "a scoped read printed something else after the writes"
        );
    }
}

// sattler's review of #100: a seed stored under an IRI other than its
// canonical one is found by the ready query (by id) but not by the scoped read
// (by IRI). ready must refuse, naming the id and both IRIs, never leave it out.
#[test]
fn ready_refuses_a_seed_stored_under_a_non_canonical_iri() {
    let Some(server) = quipu_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let graph = "https://seeds.local/project/odd-iri";
    let mut r = RemoteBackend::connect(&server.base, graph, None, None, &[]).unwrap();
    let c = ctx();
    let (fine, _) = engine::create(
        &mut r,
        &c,
        &CreateReq {
            title: "stored where it should be".into(),
            ..CreateReq::default()
        },
    )
    .unwrap();
    let odd = "https://seeds.local/elsewhere/odd-1";
    let q = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{ <{odd}> a <{}> ; <{}> \"odd-1\" ; <{}> \"odd\" ; \
         <{}> \"open\" . }} }}",
        term::work_item(),
        term::identifier(),
        term::name(),
        term::status()
    );
    r.post(
        "/update",
        "application/x-www-form-urlencoded",
        &format!("update={}", super::form_encode(&q)),
        true,
    )
    .unwrap();
    // The whole snapshot reads it by id: it is ready.
    let whole = engine::ready(&Whole(&mut r), &c, &ReadyReq::default(), None).unwrap();
    let ids: Vec<&str> = whole.issues.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"odd-1") && ids.contains(&fine.id.as_str()),
        "{ids:?}"
    );
    // The scoped read cannot find it at its canonical IRI: a refusal, never
    // an answer without it.
    let e = engine::ready(&r, &c, &ReadyReq::default(), None).unwrap_err();
    assert_eq!(e.kind, crate::error::ErrorKind::Failed, "{}", e.message);
    for want in ["odd-1", odd, &vocab::item_iri("odd-1")] {
        assert!(
            e.message.contains(want),
            "{want} missing from: {}",
            e.message
        );
    }
}

//! The verbs, as pure functions over a [`Backend`].
//!
//! Nothing here parses arguments, reads a clock or touches the filesystem: the
//! caller hands in typed requests and a [`Ctx`]. That is what lets the same
//! verbs run in the native CLI and on wasm32.
//!
//! Every write reads a snapshot, computes the post-state of the seeds it
//! touches, and commits them with the revision it read ([`SeedWrite`]). The
//! backend refuses the batch if any seed moved in between, so a stale writer
//! gets a clean [`crate::error::ErrorKind::Conflict`], never a silently lost
//! update. That check is atomic within one store handle; across processes the
//! native CLI serializes writers with a file lock (`src/native/store.rs`).

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::{Backend, Ctx, SeedWrite, WriteBatch};
use crate::error::{ErrorKind, Result, SdError};
use crate::ids;
use crate::model::{self, Comment, Seed, Snapshot};
use crate::vocab;

/// The default `list` page size, as in br. `--limit 0` lists everything.
pub const DEFAULT_LIST_LIMIT: usize = 50;

/// What a verb produced beyond its result: things the caller should print on
/// stderr (never mixed into `--json` output).
pub type Warnings = Vec<String>;

// ---------------------------------------------------------------- create

/// `sd create`.
#[derive(Debug, Clone, Default)]
pub struct CreateReq {
    /// Title (required, non-empty).
    pub title: String,
    /// Description.
    pub description: Option<String>,
    /// Type; `task` when absent.
    pub issue_type: Option<String>,
    /// Priority; 2 when absent.
    pub priority: Option<String>,
    /// Assignee.
    pub assignee: Option<String>,
    /// Labels.
    pub labels: Vec<String>,
    /// Parent seed: the new seed is minted as `<parent>.<n>`.
    pub parent: Option<String>,
    /// Dependencies, br's form: `id` (blocks) or `type:id`.
    pub deps: Vec<String>,
    /// The shuttle run that creates or drives this seed (an IRI or a bare run id).
    pub workflow_run: Option<String>,
    /// The workflow step creating this seed. With `workflow_run`, the seed's
    /// id is derived from (run, step, visit) and the create is idempotent: if
    /// that seed exists it is returned unchanged (transaction 0).
    pub step: Option<String>,
    /// Which entry into `step` this is (1 when absent).
    pub visit: Option<u32>,
    /// Compute and return the seed without writing it.
    pub dry_run: bool,
}

/// Create a seed. Returns it and the transaction that wrote it (0 on a dry run).
pub fn create(b: &mut dyn Backend, ctx: &Ctx, req: &CreateReq) -> Result<(Seed, u64)> {
    let title = req.title.trim();
    if title.is_empty() {
        return Err(SdError::usage("a seed needs a non-empty title"));
    }
    let snap = b.snapshot(None)?;
    let run = non_empty(req.workflow_run.as_deref()).map(|r| vocab::run_iri(&r));
    let keyed = match (non_empty(req.step.as_deref()), &run) {
        (Some(step), Some(run)) => {
            if req.parent.is_some() {
                return Err(SdError::usage(
                    "--step mints the id from the run and step, so it cannot take --parent",
                ));
            }
            let visit = req.visit.unwrap_or(1);
            if visit == 0 {
                return Err(SdError::usage("--visit counts from 1"));
            }
            Some(ids::keyed(&ctx.prefix, run, &step, visit))
        }
        (Some(_), None) => {
            return Err(SdError::usage("--step needs --workflow-run"));
        }
        (None, _) if req.visit.is_some() => {
            return Err(SdError::usage("--visit needs --step"));
        }
        (None, _) => None,
    };
    if let Some(id) = &keyed {
        if let Some(existing) = existing_keyed(&snap, id, run.as_deref())? {
            return Ok((existing, 0));
        }
    }
    let id = match (&keyed, &req.parent) {
        (Some(id), _) => id.clone(),
        (None, Some(p)) => {
            snap.get(p)?;
            ids::child(p, |c| snap.seeds.contains_key(c))
        }
        (None, None) => ids::mint(&ctx.prefix, title, &ctx.now, |c| snap.seeds.contains_key(c)),
    };
    let mut seed = Seed {
        id,
        title: title.to_string(),
        description: non_empty(req.description.as_deref()),
        status: "open".into(),
        priority: match &req.priority {
            Some(p) => model::parse_priority(p)?,
            None => model::DEFAULT_PRIORITY,
        },
        issue_type: match &req.issue_type {
            Some(t) => model::parse_type(t)?,
            None => "task".into(),
        },
        assignee: non_empty(req.assignee.as_deref().map(str::trim)),
        labels: clean_labels(&req.labels),
        created_at: ctx.now.clone(),
        created_by: non_empty(Some(&ctx.actor)),
        updated_at: ctx.now.clone(),
        parent: req.parent.clone(),
        workflow_run: run.clone(),
        revision: 1,
        ..Seed::default()
    };
    for spec in &req.deps {
        let (dep_type, target) = parse_dep_spec(spec)?;
        snap.get(&target)?;
        seed.add_dep(&target, &dep_type);
    }
    if req.dry_run {
        return Ok((seed, 0));
    }
    let written = b.commit(
        &WriteBatch {
            seeds: vec![SeedWrite {
                seed: seed.clone(),
                expected_revision: None,
            }],
            comments: vec![],
            source: "seeds:create".into(),
            ..WriteBatch::default()
        },
        ctx,
    );
    match (written, &keyed) {
        (Ok(tx), _) => Ok((seed, tx)),
        // A keyed create that lost a race: the other writer created the same
        // seed between our read and our write, and the compare-and-set refused
        // ours. That is the idempotent outcome, not a failure.
        (Err(e), Some(id)) if e.kind == ErrorKind::Conflict => {
            let snap = b.snapshot(None)?;
            match existing_keyed(&snap, id, run.as_deref())? {
                Some(existing) => Ok((existing, 0)),
                None => Err(e),
            }
        }
        (Err(e), _) => Err(e),
    }
}

/// The seed a keyed create names, if it already exists. A seed with that id
/// but a different run is refused rather than returned: it is not the seed
/// this step asked for.
fn existing_keyed(snap: &Snapshot, id: &str, run: Option<&str>) -> Result<Option<Seed>> {
    let Some(existing) = snap.seeds.get(id) else {
        return Ok(None);
    };
    if existing.workflow_run.as_deref() != run {
        return Err(SdError::refused(format!(
            "{id} exists but belongs to a different workflow run ({}), so it is not the seed \
             this step names",
            existing.workflow_run.as_deref().unwrap_or("none")
        )));
    }
    Ok(Some(existing.clone()))
}

/// br's `--deps` element: `id` means `blocks`, `type:id` names the type.
fn parse_dep_spec(spec: &str) -> Result<(String, String)> {
    match spec.split_once(':') {
        Some((t, id)) => Ok((model::parse_dep_type(t)?, id.trim().to_string())),
        None => Ok(("blocks".into(), spec.trim().to_string())),
    }
}

// ---------------------------------------------------------------- show

/// A seed with everything `show` prints about it.
#[derive(Debug, Clone)]
pub struct SeedView {
    /// The seed.
    pub seed: Seed,
    /// What it depends on: (target, type, target seed if present).
    pub dependencies: Vec<(String, &'static str, Option<Seed>)>,
    /// What depends on it: (dependent, type, dependent seed).
    pub dependents: Vec<(String, &'static str, Option<Seed>)>,
    /// Its comments, in order.
    pub comments: Vec<Comment>,
}

/// `sd show`: the seeds named, as of `at`.
pub fn show(b: &dyn Backend, ids: &[String], at: Option<u64>) -> Result<Vec<SeedView>> {
    let snap = b.snapshot(at)?;
    ids.iter().map(|id| view(&snap, id)).collect()
}

fn view(snap: &Snapshot, id: &str) -> Result<SeedView> {
    let seed = snap.get(id)?.clone();
    let dependencies = seed
        .dependencies()
        .into_iter()
        .map(|(t, ty)| {
            let s = snap.seeds.get(&t).cloned();
            (t, ty, s)
        })
        .collect();
    let dependents = snap
        .dependents(id)
        .into_iter()
        .map(|(d, ty)| {
            let s = snap.seeds.get(&d).cloned();
            (d, ty, s)
        })
        .collect();
    let comments = snap.comments_on(id).into_iter().cloned().collect();
    Ok(SeedView {
        seed,
        dependencies,
        dependents,
        comments,
    })
}

// ---------------------------------------------------------------- list / ready / count

/// Filters shared by `list`, `ready` and `count`.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only this status.
    pub status: Option<String>,
    /// Only this type.
    pub issue_type: Option<String>,
    /// Only this assignee.
    pub assignee: Option<String>,
    /// Only seeds with no assignee.
    pub unassigned: bool,
    /// Only seeds carrying every one of these labels.
    pub labels: Vec<String>,
    /// Only this priority.
    pub priority: Option<String>,
    /// Only children of this seed.
    pub parent: Option<String>,
}

impl Filter {
    fn compile(&self) -> Result<CompiledFilter> {
        Ok(CompiledFilter {
            status: self
                .status
                .as_deref()
                .map(model::parse_status)
                .transpose()?,
            issue_type: self
                .issue_type
                .as_deref()
                .map(model::parse_type)
                .transpose()?,
            assignee: self.assignee.clone(),
            unassigned: self.unassigned,
            labels: clean_labels(&self.labels),
            priority: self
                .priority
                .as_deref()
                .map(model::parse_priority)
                .transpose()?,
            parent: self.parent.clone(),
        })
    }
}

struct CompiledFilter {
    status: Option<String>,
    issue_type: Option<String>,
    assignee: Option<String>,
    unassigned: bool,
    labels: BTreeSet<String>,
    priority: Option<u8>,
    parent: Option<String>,
}

impl CompiledFilter {
    fn matches(&self, s: &Seed) -> bool {
        self.status.as_ref().is_none_or(|v| &s.status == v)
            && self.issue_type.as_ref().is_none_or(|v| &s.issue_type == v)
            && self
                .assignee
                .as_ref()
                .is_none_or(|v| s.assignee.as_ref() == Some(v))
            && (!self.unassigned || s.assignee.is_none())
            && self.labels.is_subset(&s.labels)
            && self.priority.is_none_or(|p| s.priority == p)
            && self
                .parent
                .as_ref()
                .is_none_or(|p| s.parent.as_ref() == Some(p))
    }
}

/// Sort orders `list --sort` accepts.
pub const SORTS: &[&str] = &["priority", "created", "updated", "id", "title"];

fn sort_seeds(seeds: &mut [Seed], sort: Option<&str>) -> Result<()> {
    match sort.unwrap_or("priority") {
        "priority" => seeds.sort_by(|a, b| {
            (a.priority, &a.created_at, &a.id).cmp(&(b.priority, &b.created_at, &b.id))
        }),
        "created" => seeds.sort_by(|a, b| (&a.created_at, &a.id).cmp(&(&b.created_at, &b.id))),
        "updated" => seeds.sort_by(|a, b| (&b.updated_at, &a.id).cmp(&(&a.updated_at, &b.id))),
        "id" => seeds.sort_by(|a, b| a.id.cmp(&b.id)),
        "title" => seeds.sort_by(|a, b| (&a.title, &a.id).cmp(&(&b.title, &b.id))),
        other => {
            return Err(SdError::usage(format!(
                "unknown sort {other:?}; expected one of {}",
                SORTS.join(", ")
            )))
        }
    }
    Ok(())
}

/// `sd list`.
#[derive(Debug, Clone, Default)]
pub struct ListReq {
    /// Filters.
    pub filter: Filter,
    /// Include closed seeds (they are hidden unless `--status` or `--all`).
    pub all: bool,
    /// Page size; `None` means [`DEFAULT_LIST_LIMIT`], `Some(0)` means all.
    pub limit: Option<usize>,
    /// Sort order (one of [`SORTS`]).
    pub sort: Option<String>,
}

/// A page of seeds and whether it was cut short.
#[derive(Debug, Clone)]
pub struct Page {
    /// The seeds on this page.
    pub issues: Vec<Seed>,
    /// How many matched before the limit.
    pub total: usize,
    /// The limit applied (0 = none).
    pub limit: usize,
    /// True when `total > issues.len()`.
    pub has_more: bool,
    /// For each seed on the page, how many seeds declare any dependency on it
    /// (br's `dependent_count`: blocks, parent-child, related, discovered-from).
    pub dependent_counts: BTreeMap<String, usize>,
}

/// `sd list`, as of `at`.
pub fn list(b: &dyn Backend, req: &ListReq, at: Option<u64>) -> Result<Page> {
    let f = req.filter.compile()?;
    let snap = b.snapshot(at)?;
    let mut seeds: Vec<Seed> = snap
        .seeds
        .values()
        .filter(|s| req.all || f.status.is_some() || s.status != "closed")
        .filter(|s| f.matches(s))
        .cloned()
        .collect();
    sort_seeds(&mut seeds, req.sort.as_deref())?;
    Ok(page(seeds, req.limit.unwrap_or(DEFAULT_LIST_LIMIT), &snap))
}

fn page(mut seeds: Vec<Seed>, limit: usize, snap: &Snapshot) -> Page {
    let total = seeds.len();
    if limit > 0 && seeds.len() > limit {
        seeds.truncate(limit);
    }
    let dependent_counts = seeds
        .iter()
        .map(|s| (s.id.clone(), snap.dependents(&s.id).len()))
        .collect();
    Page {
        has_more: seeds.len() < total,
        issues: seeds,
        total,
        limit,
        dependent_counts,
    }
}

/// `sd search`.
#[derive(Debug, Clone, Default)]
pub struct SearchReq {
    /// Text to find (case-insensitive substring).
    pub query: String,
    /// Filters.
    pub filter: Filter,
    /// Include closed seeds (hidden, and counted, unless `--status` or `--all`).
    pub all: bool,
    /// Page size; `None` means [`DEFAULT_LIST_LIMIT`] (br's 50), `Some(0)` means all.
    pub limit: Option<usize>,
    /// Sort order (one of [`SORTS`]).
    pub sort: Option<String>,
}

/// A page of search hits and how many closed hits were hidden.
#[derive(Debug, Clone)]
pub struct SearchPage {
    /// The page.
    pub page: Page,
    /// Closed seeds that matched but were hidden (pass `--all` to see them).
    pub hidden_closed: usize,
}

/// `sd search`: seeds whose id, title, description or any comment contains
/// the query, case-insensitively (br's fields; notes are not searched, as in br).
pub fn search(b: &dyn Backend, req: &SearchReq, at: Option<u64>) -> Result<SearchPage> {
    let query = req.query.trim().to_lowercase();
    if query.is_empty() {
        return Err(SdError::usage("search needs a non-empty query"));
    }
    let f = req.filter.compile()?;
    let snap = b.snapshot(at)?;
    let hit = |s: &Seed| {
        let has = |t: &str| t.to_lowercase().contains(&query);
        has(&s.id)
            || has(&s.title)
            || s.description.as_deref().is_some_and(has)
            || snap.comments_on(&s.id).iter().any(|c| has(&c.text))
    };
    let show_closed = req.all || f.status.is_some();
    let (mut seeds, mut hidden_closed) = (Vec::new(), 0);
    for s in snap.seeds.values().filter(|s| f.matches(s) && hit(s)) {
        if s.status == "closed" && !show_closed {
            hidden_closed += 1;
        } else {
            seeds.push(s.clone());
        }
    }
    sort_seeds(&mut seeds, req.sort.as_deref())?;
    Ok(SearchPage {
        page: page(seeds, req.limit.unwrap_or(DEFAULT_LIST_LIMIT), &snap),
        hidden_closed,
    })
}

/// `sd stale`: seeds not updated for at least `days` days, oldest first.
/// Closed seeds are left out unless `statuses` names `closed` (br). `statuses`
/// entries may be comma-separated.
pub fn stale(
    b: &dyn Backend,
    ctx: &Ctx,
    days: u32,
    statuses: &[String],
    at: Option<u64>,
) -> Result<Vec<Seed>> {
    let statuses = statuses
        .iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(model::parse_status)
        .collect::<Result<BTreeSet<_>>>()?;
    let now = parse_instant(&ctx.now)
        .ok_or_else(|| SdError::failed(format!("unreadable clock {:?}", ctx.now)))?;
    let cutoff = format_instant(now - i64::from(days) * 86_400);
    let snap = b.snapshot(at)?;
    let mut seeds: Vec<Seed> = snap
        .seeds
        .values()
        .filter(|s| {
            if statuses.is_empty() {
                s.status != "closed"
            } else {
                statuses.contains(&s.status)
            }
        })
        .filter(|s| s.updated_at.as_str() <= cutoff.as_str())
        .cloned()
        .collect();
    seeds.sort_by(|a, b| (&a.updated_at, &a.id).cmp(&(&b.updated_at, &b.id)));
    Ok(seeds)
}

/// Which breakdowns `sd stats` adds to its summary.
#[derive(Debug, Clone, Copy, Default)]
pub struct StatsReq {
    /// By issue type.
    pub by_type: bool,
    /// By priority (`P0`..`P4`).
    pub by_priority: bool,
    /// By assignee (`(unassigned)` for none).
    pub by_assignee: bool,
    /// By label (`(no labels)` for none).
    pub by_label: bool,
}

/// `sd stats`: br's summary counts and optional breakdowns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// Every seed.
    pub total: usize,
    /// Seeds by status.
    pub open: usize,
    /// In progress.
    pub in_progress: usize,
    /// Closed.
    pub closed: usize,
    /// Status `blocked` (br counts the status, not the dependency graph).
    pub blocked: usize,
    /// Deferred.
    pub deferred: usize,
    /// What `sd ready` would list now.
    pub ready: usize,
    /// Open epics whose children are all closed.
    pub epics_eligible_for_closure: usize,
    /// Mean hours from creation to close, over closed seeds (0 when none).
    pub average_lead_time_hours: f64,
    /// (dimension, [(key, count)]) in br's order: type, priority, assignee, label.
    pub breakdowns: Vec<(&'static str, Vec<(String, usize)>)>,
}

/// `sd stats`, as of `at`. Breakdowns count every seed, closed included (br).
pub fn stats(b: &dyn Backend, ctx: &Ctx, req: StatsReq, at: Option<u64>) -> Result<Stats> {
    let snap = b.snapshot(at)?;
    let seeds: Vec<&Seed> = snap.seeds.values().collect();
    let count = |st: &str| seeds.iter().filter(|s| s.status == st).count();
    let lead: Vec<f64> = seeds
        .iter()
        .filter(|s| s.status == "closed")
        .filter_map(|s| {
            let closed = parse_instant(s.closed_at.as_deref()?)?;
            Some((closed - parse_instant(&s.created_at)?) as f64 / 3600.0)
        })
        .collect();
    let eligible = seeds
        .iter()
        .filter(|e| e.issue_type == "epic" && e.status != "closed")
        .filter(|e| {
            let children: Vec<&&Seed> = seeds
                .iter()
                .filter(|c| c.parent.as_deref() == Some(e.id.as_str()))
                .collect();
            !children.is_empty() && children.iter().all(|c| c.status == "closed")
        })
        .count();
    let tally = |keys: &dyn Fn(&Seed) -> Vec<String>| {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for s in &seeds {
            for k in keys(s) {
                *m.entry(k).or_default() += 1;
            }
        }
        m.into_iter().collect::<Vec<_>>()
    };
    let mut breakdowns = Vec::new();
    if req.by_type {
        breakdowns.push(("type", tally(&|s| vec![s.issue_type.clone()])));
    }
    if req.by_priority {
        breakdowns.push(("priority", tally(&|s| vec![format!("P{}", s.priority)])));
    }
    if req.by_assignee {
        breakdowns.push((
            "assignee",
            tally(&|s| vec![s.assignee.clone().unwrap_or_else(|| "(unassigned)".into())]),
        ));
    }
    if req.by_label {
        breakdowns.push((
            "label",
            tally(&|s| {
                if s.labels.is_empty() {
                    vec!["(no labels)".into()]
                } else {
                    s.labels.iter().cloned().collect()
                }
            }),
        ));
    }
    Ok(Stats {
        total: seeds.len(),
        open: count("open"),
        in_progress: count("in_progress"),
        closed: count("closed"),
        blocked: count("blocked"),
        deferred: count("deferred"),
        ready: ready(b, ctx, &ReadyReq::default(), at)?.total,
        epics_eligible_for_closure: eligible,
        average_lead_time_hours: if lead.is_empty() {
            0.0
        } else {
            lead.iter().sum::<f64>() / lead.len() as f64
        },
        breakdowns,
    })
}

/// One epic's progress, as `sd epic status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicStatus {
    /// The epic.
    pub epic: Seed,
    /// Its children (seeds whose parent it is).
    pub total_children: usize,
    /// How many of them are closed.
    pub closed_children: usize,
    /// Every child closed, and at least one child (br).
    pub eligible_for_close: bool,
}

/// `sd epic status`: every epic that is not closed, with child progress.
pub fn epic_status(
    b: &dyn Backend,
    eligible_only: bool,
    at: Option<u64>,
) -> Result<Vec<EpicStatus>> {
    let snap = b.snapshot(at)?;
    let mut out = Vec::new();
    for e in snap
        .seeds
        .values()
        .filter(|e| e.issue_type == "epic" && e.status != "closed")
    {
        let kids: Vec<&Seed> = snap
            .seeds
            .values()
            .filter(|c| c.parent.as_deref() == Some(e.id.as_str()))
            .collect();
        let closed = kids.iter().filter(|c| c.status == "closed").count();
        let eligible = !kids.is_empty() && closed == kids.len();
        if eligible || !eligible_only {
            out.push(EpicStatus {
                epic: e.clone(),
                total_children: kids.len(),
                closed_children: closed,
                eligible_for_close: eligible,
            });
        }
    }
    Ok(out)
}

/// `sd epic close-eligible`: close every eligible epic in one transaction with
/// br's reason. An eligible epic that still has an open blocker of its own is
/// left open and reported, never force-closed.
pub fn epic_close_eligible(
    b: &mut dyn Backend,
    ctx: &Ctx,
) -> Result<(Vec<Seed>, Vec<Skipped>, u64)> {
    let snap = b.snapshot(None)?;
    let (mut ids, mut skipped) = (Vec::new(), Vec::new());
    for st in epic_status(b, true, None)? {
        let blockers = snap.open_blockers(&st.epic);
        if blockers.is_empty() {
            ids.push(st.epic.id);
        } else {
            skipped.push(Skipped {
                id: st.epic.id,
                reason: format!("blocked by {}", blockers.join(", ")),
            });
        }
    }
    if ids.is_empty() {
        return Ok((vec![], skipped, snap.tx));
    }
    let (closed, tx, _) = close(b, ctx, &ids, Some("All children completed"), false)?;
    Ok((closed, skipped, tx))
}

/// `sd blocked`. Repeated types and priorities are alternatives; labels must
/// all match (br's semantics).
#[derive(Debug, Clone, Default)]
pub struct BlockedReq {
    /// Only these types (any of them).
    pub types: Vec<String>,
    /// Only these priorities (any of them).
    pub priorities: Vec<String>,
    /// Only seeds carrying every one of these labels.
    pub labels: Vec<String>,
    /// Page size; `None` means [`DEFAULT_LIST_LIMIT`] (br's 50), `Some(0)` means all.
    pub limit: Option<usize>,
}

/// A page of blocked seeds and, for each, its open blockers.
#[derive(Debug, Clone)]
pub struct BlockedPage {
    /// The page.
    pub page: Page,
    /// Open `blocks` targets by seed id, for every seed on the page.
    pub blocked_by: BTreeMap<String, Vec<String>>,
}

/// `sd blocked`: seeds that are not closed and have at least one open `blocks`
/// dependency, as of `at`. A `blocked` status alone does not qualify (br).
pub fn blocked(b: &dyn Backend, req: &BlockedReq, at: Option<u64>) -> Result<BlockedPage> {
    let types = req
        .types
        .iter()
        .map(|t| model::parse_type(t))
        .collect::<Result<BTreeSet<_>>>()?;
    let priorities = req
        .priorities
        .iter()
        .map(|p| model::parse_priority(p))
        .collect::<Result<BTreeSet<_>>>()?;
    let labels = clean_labels(&req.labels);
    let snap = b.snapshot(at)?;
    let mut blocked_by = BTreeMap::new();
    let mut seeds: Vec<Seed> = Vec::new();
    for s in snap.seeds.values().filter(|s| s.status != "closed") {
        let blockers = snap.open_blockers(s);
        if blockers.is_empty()
            || !(types.is_empty() || types.contains(&s.issue_type))
            || !(priorities.is_empty() || priorities.contains(&s.priority))
            || !labels.is_subset(&s.labels)
        {
            continue;
        }
        blocked_by.insert(s.id.clone(), blockers);
        seeds.push(s.clone());
    }
    sort_seeds(&mut seeds, None)?;
    let page = page(seeds, req.limit.unwrap_or(DEFAULT_LIST_LIMIT), &snap);
    blocked_by.retain(|id, _| page.issues.iter().any(|s| &s.id == id));
    Ok(BlockedPage { page, blocked_by })
}

/// `sd ready`.
#[derive(Debug, Clone, Default)]
pub struct ReadyReq {
    /// Filters (its `status` is ignored: ready means open).
    pub filter: Filter,
    /// `None` = every ready seed (the default: never silently short).
    pub limit: Option<usize>,
}

/// `sd ready`: open seeds with no open `blocks` dependency and no future defer
/// date, as of `at`. Unlimited unless `limit` is given; the page says when it
/// was cut.
pub fn ready(b: &dyn Backend, ctx: &Ctx, req: &ReadyReq, at: Option<u64>) -> Result<Page> {
    let mut filter = req.filter.clone();
    filter.status = None;
    let f = filter.compile()?;
    let snap = b.snapshot(at)?;
    let ids = b.ready_ids(at)?;
    let mut seeds: Vec<Seed> = ids
        .iter()
        .filter_map(|id| snap.seeds.get(id))
        .filter(|s| !is_deferred(s, &ctx.now))
        .filter(|s| f.matches(s))
        .cloned()
        .collect();
    sort_seeds(&mut seeds, Some("priority"))?;
    Ok(page(seeds, req.limit.unwrap_or(0), &snap))
}

/// The ready definition computed directly over a snapshot, independent of the
/// backend's query. The backend's SPARQL is authoritative for `sd ready`; this
/// exists so the two can be checked against each other.
pub fn ready_by_model(snap: &Snapshot, now: &str) -> Vec<String> {
    snap.seeds
        .values()
        .filter(|s| s.status == "open" && snap.open_blockers(s).is_empty())
        .filter(|s| !is_deferred(s, now))
        .map(|s| s.id.clone())
        .collect()
}

fn is_deferred(s: &Seed, now: &str) -> bool {
    // ISO-8601 dates and UTC instants order correctly as strings; a date-only
    // defer ("2026-10-01") sorts before that day's instants, so it releases at
    // the start of the day.
    s.defer_until.as_deref().is_some_and(|d| d > now)
}

/// `sd count`.
#[derive(Debug, Clone, Default)]
pub struct CountReq {
    /// Filters.
    pub filter: Filter,
    /// Group by `status`, `priority`, `type`, `assignee` or `label`.
    pub by: Option<String>,
    /// Count closed seeds too (hidden unless `--status` or this).
    pub include_closed: bool,
}

/// A count, optionally grouped.
#[derive(Debug, Clone)]
pub struct Count {
    /// Seeds matched.
    pub total: usize,
    /// Per-group counts when `--by` was given, sorted by group.
    pub groups: Option<Vec<(String, usize)>>,
}

/// `sd count`, as of `at`.
pub fn count(b: &dyn Backend, req: &CountReq, at: Option<u64>) -> Result<Count> {
    let f = req.filter.compile()?;
    let snap = b.snapshot(at)?;
    let seeds: Vec<&Seed> = snap
        .seeds
        .values()
        .filter(|s| req.include_closed || f.status.is_some() || s.status != "closed")
        .filter(|s| f.matches(s))
        .collect();
    let groups = match req.by.as_deref() {
        None => None,
        Some(by) => {
            let mut g: BTreeMap<String, usize> = BTreeMap::new();
            for s in &seeds {
                let keys: Vec<String> = match by {
                    "status" => vec![s.status.clone()],
                    "priority" => vec![format!("P{}", s.priority)],
                    "type" => vec![s.issue_type.clone()],
                    "assignee" => vec![s.assignee.clone().unwrap_or_else(|| "(unassigned)".into())],
                    "label" => {
                        if s.labels.is_empty() {
                            vec!["(no labels)".into()]
                        } else {
                            s.labels.iter().cloned().collect()
                        }
                    }
                    other => {
                        return Err(SdError::usage(format!(
                            "unknown --by {other:?}; expected status, priority, type, assignee or label"
                        )))
                    }
                };
                for k in keys {
                    *g.entry(k).or_default() += 1;
                }
            }
            Some(g.into_iter().collect())
        }
    };
    Ok(Count {
        total: seeds.len(),
        groups,
    })
}

// ---------------------------------------------------------------- update / close

/// `sd update`. `Some("")` clears an optional field (assignee, defer).
#[derive(Debug, Clone, Default)]
pub struct UpdateReq {
    /// New title.
    pub title: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New notes.
    pub notes: Option<String>,
    /// New status.
    pub status: Option<String>,
    /// New priority.
    pub priority: Option<String>,
    /// New assignee; empty clears it.
    pub assignee: Option<String>,
    /// Atomically claim: assignee = actor and `in_progress`, only if unclaimed.
    pub claim: bool,
    /// Labels to add.
    pub add_labels: Vec<String>,
    /// Labels to remove.
    pub remove_labels: Vec<String>,
    /// Defer until this date or instant; empty clears it.
    pub defer: Option<String>,
    /// The shuttle run driving the seed (an IRI or a bare run id); empty clears it.
    pub workflow_run: Option<String>,
}

/// `sd update`: all named seeds change in one transaction, or none do.
pub fn update(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    req: &UpdateReq,
) -> Result<(Vec<Seed>, u64)> {
    let status = req.status.as_deref().map(model::parse_status).transpose()?;
    let priority = req
        .priority
        .as_deref()
        .map(model::parse_priority)
        .transpose()?;
    if req.claim && (req.assignee.is_some() || status.is_some()) {
        return Err(SdError::usage(
            "--claim sets the assignee and status itself; do not combine it with --assignee or --status",
        ));
    }
    if let Some(t) = &req.title {
        if t.trim().is_empty() {
            return Err(SdError::usage("a seed needs a non-empty title"));
        }
    }
    let snap = b.snapshot(None)?;
    let mut writes = Vec::new();
    for id in ids {
        let before = snap.get(id)?;
        let mut s = before.clone();
        if req.claim {
            claim(&snap, &mut s, &ctx.actor)?;
        }
        if let Some(t) = &req.title {
            s.title = t.trim().to_string();
        }
        if let Some(d) = &req.description {
            s.description = non_empty(Some(d));
        }
        if let Some(n) = &req.notes {
            s.notes = non_empty(Some(n));
        }
        if let Some(p) = priority {
            s.priority = p;
        }
        if let Some(a) = &req.assignee {
            s.assignee = non_empty(Some(a.trim()));
        }
        if let Some(d) = &req.defer {
            s.defer_until = non_empty(Some(d.trim()));
        }
        if let Some(r) = &req.workflow_run {
            s.workflow_run = non_empty(Some(r.trim())).map(|r| vocab::run_iri(&r));
        }
        for l in clean_labels(&req.add_labels) {
            s.labels.insert(l);
        }
        for l in clean_labels(&req.remove_labels) {
            s.labels.remove(&l);
        }
        if let Some(st) = &status {
            set_status(&mut s, st, &ctx.now, None);
        }
        if s == *before {
            continue;
        }
        s.updated_at = ctx.now.clone();
        s.revision = before.revision + 1;
        writes.push(SeedWrite {
            seed: s,
            expected_revision: Some(before.revision),
        });
    }
    finish(b, ctx, &snap, ids, writes, "seeds:update")
}

// ---------------------------------------------------------------- labels

/// One seed's outcome from `label add` / `label remove`, in br's vocabulary:
/// `added` or `exists`, `removed` or `not_found`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelChange {
    /// The seed.
    pub issue_id: String,
    /// The label.
    pub label: String,
    /// What happened to this seed.
    pub status: &'static str,
}

fn one_label(label: &str) -> Result<String> {
    let label = label.trim();
    if label.is_empty() {
        return Err(SdError::usage("a label must be non-empty"));
    }
    if label.contains(',') {
        return Err(SdError::usage(format!(
            "a label cannot contain a comma: {label:?} (labels are comma-separated elsewhere)"
        )));
    }
    Ok(label.to_string())
}

fn unique(ids: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|i| seen.insert(i.as_str()))
        .cloned()
        .collect()
}

/// `sd label add|remove`: every named seed changes in one transaction, or none
/// do. A seed that already has (or lacks) the label is reported, not written.
pub fn label_change(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    label: &str,
    add: bool,
) -> Result<(Vec<LabelChange>, u64)> {
    let label = one_label(label)?;
    let ids = unique(ids);
    if ids.is_empty() {
        return Err(SdError::usage("at least one seed id is required"));
    }
    let snap = b.snapshot(None)?;
    let (mut writes, mut changes) = (Vec::new(), Vec::new());
    for id in &ids {
        let before = snap.get(id)?;
        let mut s = before.clone();
        let changed = if add {
            s.labels.insert(label.clone())
        } else {
            s.labels.remove(&label)
        };
        let status = match (add, changed) {
            (true, true) => "added",
            (true, false) => "exists",
            (false, true) => "removed",
            (false, false) => "not_found",
        };
        changes.push(LabelChange {
            issue_id: id.clone(),
            label: label.clone(),
            status,
        });
        if changed {
            s.updated_at = ctx.now.clone();
            s.revision = before.revision + 1;
            writes.push(SeedWrite {
                seed: s,
                expected_revision: Some(before.revision),
            });
        }
    }
    let source = if add {
        "seeds:label-add"
    } else {
        "seeds:label-remove"
    };
    let (_, tx) = finish(b, ctx, &snap, &ids, writes, source)?;
    Ok((changes, tx))
}

/// `sd label rename <old> <new>`: every seed carrying `old` carries `new`
/// instead, in one transaction. Returns the number of seeds changed.
pub fn label_rename(b: &mut dyn Backend, ctx: &Ctx, old: &str, new: &str) -> Result<(usize, u64)> {
    let (old, new) = (one_label(old)?, one_label(new)?);
    let snap = b.snapshot(None)?;
    if old == new {
        return Ok((0, snap.tx));
    }
    let (mut writes, mut ids) = (Vec::new(), Vec::new());
    for before in snap.seeds.values().filter(|s| s.labels.contains(&old)) {
        let mut s = before.clone();
        s.labels.remove(&old);
        s.labels.insert(new.clone());
        s.updated_at = ctx.now.clone();
        s.revision = before.revision + 1;
        ids.push(s.id.clone());
        writes.push(SeedWrite {
            seed: s,
            expected_revision: Some(before.revision),
        });
    }
    let (_, tx) = finish(b, ctx, &snap, &ids, writes, "seeds:label-rename")?;
    Ok((ids.len(), tx))
}

/// `sd label list [id]`: one seed's labels, or every label in use (closed
/// seeds included, as br does), sorted.
pub fn labels(b: &dyn Backend, id: Option<&str>, at: Option<u64>) -> Result<Vec<String>> {
    let snap = b.snapshot(at)?;
    Ok(match id {
        Some(id) => snap.get(id)?.labels.iter().cloned().collect(),
        None => snap
            .seeds
            .values()
            .flat_map(|s| s.labels.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    })
}

/// `sd label list-all`: every label in use with the number of seeds carrying it.
pub fn label_counts(b: &dyn Backend, at: Option<u64>) -> Result<Vec<(String, usize)>> {
    let snap = b.snapshot(at)?;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for l in snap.seeds.values().flat_map(|s| s.labels.iter()) {
        *counts.entry(l.clone()).or_default() += 1;
    }
    Ok(counts.into_iter().collect())
}

// ---------------------------------------------------------------- transitions

/// A seed a status transition changed, with the status it left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The seed after the change.
    pub seed: Seed,
    /// Its status before.
    pub previous_status: String,
}

/// A seed a status transition left alone, with br's reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The seed.
    pub id: String,
    /// Why it was not changed.
    pub reason: String,
}

/// What `reopen`, `defer` and `undefer` return.
pub type Transitions = (Vec<Transition>, Vec<Skipped>, u64);

/// Apply one transition to every named seed in one transaction. `skip` says
/// why a seed is left alone; `apply` changes the rest. Unknown ids write nothing.
fn transition(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    source: &str,
    skip: impl Fn(&Seed) -> Option<String>,
    apply: impl Fn(&mut Seed),
    comment: Option<&str>,
) -> Result<Transitions> {
    let ids = unique(ids);
    if ids.is_empty() {
        return Err(SdError::usage("at least one seed id is required"));
    }
    let snap = b.snapshot(None)?;
    let (mut writes, mut done, mut skipped, mut comments) = (vec![], vec![], vec![], vec![]);
    for id in &ids {
        let before = snap.get(id)?;
        if let Some(reason) = skip(before) {
            skipped.push(Skipped {
                id: id.clone(),
                reason,
            });
            continue;
        }
        let mut s = before.clone();
        apply(&mut s);
        s.updated_at = ctx.now.clone();
        s.revision = before.revision + 1;
        if let Some(text) = comment {
            let index = snap
                .comments_on(id)
                .iter()
                .map(|c| c.index)
                .max()
                .unwrap_or(0)
                + 1;
            comments.push(Comment {
                seed: id.clone(),
                index,
                author: ctx.actor.clone(),
                text: text.to_string(),
                created_at: ctx.now.clone(),
            });
        }
        done.push(Transition {
            seed: s.clone(),
            previous_status: before.status.clone(),
        });
        writes.push(SeedWrite {
            seed: s,
            expected_revision: Some(before.revision),
        });
    }
    let tx = if writes.is_empty() {
        snap.tx
    } else {
        b.commit(
            &WriteBatch {
                seeds: writes,
                comments,
                source: source.into(),
                ..WriteBatch::default()
            },
            ctx,
        )?
    };
    Ok((done, skipped, tx))
}

/// `sd reopen`: closed seeds become open; a reason is stored as the comment
/// "Reopened: <reason>" in the same transaction (br).
pub fn reopen(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    reason: Option<&str>,
) -> Result<Transitions> {
    let comment = reason
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| format!("Reopened: {r}"));
    transition(
        b,
        ctx,
        ids,
        "seeds:reopen",
        |s| match s.status.as_str() {
            "closed" => None,
            "open" => Some("already open".into()),
            other => Some(format!("not closed (status: {other})")),
        },
        |s| set_status(s, "open", &ctx.now, None),
        comment.as_deref(),
    )
}

/// `sd defer`: status `deferred`, hidden from ready until `until` (br's forms:
/// `+30m`, `+2h`, `+1d`, `+1w`, `tomorrow`, a date or an RFC 3339 instant).
/// Without `until` the seed is deferred with no date. Closed seeds are skipped.
pub fn defer(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    until: Option<&str>,
) -> Result<Transitions> {
    let until = until.map(|u| parse_until(u, &ctx.now)).transpose()?;
    transition(
        b,
        ctx,
        ids,
        "seeds:defer",
        |s| (s.status == "closed").then(|| "cannot defer closed issue".to_string()),
        |s| {
            s.status = "deferred".into();
            s.defer_until = until.clone();
        },
        None,
    )
}

/// `sd undefer`: deferred seeds become open and lose their defer date.
pub fn undefer(b: &mut dyn Backend, ctx: &Ctx, ids: &[String]) -> Result<Transitions> {
    transition(
        b,
        ctx,
        ids,
        "seeds:undefer",
        |s| (s.status != "deferred").then(|| format!("not deferred (status: {})", s.status)),
        |s| {
            s.status = "open".into();
            s.defer_until = None;
        },
        None,
    )
}

/// Resolve a `--until` value against `now` (an RFC 3339 UTC instant). Dates and
/// instants are kept as given; relative forms become a UTC instant or date.
pub fn parse_until(value: &str, now: &str) -> Result<String> {
    let v = value.trim();
    let bad = || {
        SdError::usage(format!(
            "cannot read --until {value:?}; use +30m, +2h, +1d, +1w, tomorrow, \
             YYYY-MM-DD or an RFC 3339 instant"
        ))
    };
    if let Some(rest) = v.strip_prefix('+') {
        let (n, unit) = rest.split_at(rest.len().saturating_sub(1));
        let n: i64 = n.parse().map_err(|_| bad())?;
        let secs = match unit {
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            "w" => 604_800,
            _ => return Err(bad()),
        };
        return Ok(format_instant(
            parse_instant(now).ok_or_else(bad)? + n * secs,
        ));
    }
    if v.eq_ignore_ascii_case("tomorrow") {
        let t = parse_instant(now).ok_or_else(bad)? + 86_400;
        return Ok(format_instant(t)[..10].to_string());
    }
    if v.len() == 10 && parse_date(v).is_some() {
        return Ok(v.to_string());
    }
    if v.len() >= 20 && parse_instant(v).is_some() {
        return Ok(v.to_string());
    }
    Err(bad())
}

fn parse_date(d: &str) -> Option<i64> {
    let b = d.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, day): (i64, i64, i64) = (
        d[..4].parse().ok()?,
        d[5..7].parse().ok()?,
        d[8..10].parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from the civil calendar (Hinnant), proleptic Gregorian, UTC.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// Seconds since the epoch of `YYYY-MM-DDTHH:MM:SS` (anything after is ignored:
/// seeds' instants are UTC).
fn parse_instant(t: &str) -> Option<i64> {
    let b = t.as_bytes();
    if b.len() < 19 || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (h, mi, s): (i64, i64, i64) = (
        t[11..13].parse().ok()?,
        t[14..16].parse().ok()?,
        t[17..19].parse().ok()?,
    );
    if h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(parse_date(&t[..10])? * 86_400 + h * 3600 + mi * 60 + s)
}

fn format_instant(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn claim(snap: &Snapshot, s: &mut Seed, actor: &str) -> Result<()> {
    if actor.trim().is_empty() {
        return Err(SdError::usage(
            "--claim needs an actor: pass --actor or set SEEDS_ACTOR",
        ));
    }
    match (&s.assignee, s.status.as_str()) {
        (Some(a), "in_progress") if a == actor => return Ok(()),
        (Some(a), _) if a != actor => {
            return Err(SdError::conflict(format!(
                "{} is already claimed by {a}; nothing was written",
                s.id
            )))
        }
        (_, "open") => {}
        (_, other) => {
            return Err(SdError::conflict(format!(
                "{} is {other}, not open; only an open seed can be claimed",
                s.id
            )))
        }
    }
    let blockers = snap.open_blockers(s);
    if !blockers.is_empty() {
        return Err(SdError::conflict(format!(
            "{} is blocked by {}; nothing was written",
            s.id,
            blockers.join(", ")
        )));
    }
    s.assignee = Some(actor.to_string());
    s.status = "in_progress".into();
    Ok(())
}

fn set_status(s: &mut Seed, status: &str, now: &str, reason: Option<&str>) {
    set_status_as(s, status, now, reason, None);
}

fn set_status_as(
    s: &mut Seed,
    status: &str,
    now: &str,
    reason: Option<&str>,
    outcome: Option<&str>,
) {
    if status == "closed" {
        if s.status != "closed" {
            s.closed_at = Some(now.to_string());
        }
        if let Some(r) = reason {
            s.close_reason = non_empty(Some(r));
        }
        s.outcome = Some(
            outcome
                .map(str::to_string)
                .or_else(|| s.outcome.clone())
                .unwrap_or_else(|| "done".into()),
        );
    } else {
        s.closed_at = None;
        s.close_reason = None;
        s.outcome = None;
    }
    s.status = status.to_string();
}

/// `sd close`: all named seeds close in one transaction, or none do. A seed
/// with open `blocks` dependencies is refused unless `force`.
pub fn close(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    reason: Option<&str>,
    force: bool,
) -> Result<(Vec<Seed>, u64, Warnings)> {
    close_as(b, ctx, ids, reason, None, force)
}

/// [`close`] with an explicit outcome (one of [`model::OUTCOMES`]; `done`
/// when `None`).
pub fn close_as(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    reason: Option<&str>,
    outcome: Option<&str>,
    force: bool,
) -> Result<(Vec<Seed>, u64, Warnings)> {
    let outcome = outcome.map(model::parse_outcome).transpose()?;
    let mut warnings = Vec::new();
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    if reason.is_none() {
        warnings.push(
            "closing without --reason: the close reason is what later readers search; \
             say what landed and how you know"
                .to_string(),
        );
    }
    let snap = b.snapshot(None)?;
    let mut writes = Vec::new();
    for id in ids {
        let before = snap.get(id)?;
        if before.status == "closed" {
            warnings.push(format!("{id} was already closed; left unchanged"));
            continue;
        }
        let blockers = snap.open_blockers(before);
        if !blockers.is_empty() && !force {
            return Err(SdError::refused(format!(
                "{id} is blocked by {} (still open); close those first or pass --force. \
                 Nothing was written.",
                blockers.join(", ")
            )));
        }
        let mut s = before.clone();
        set_status_as(&mut s, "closed", &ctx.now, reason, outcome.as_deref());
        s.updated_at = ctx.now.clone();
        s.revision = before.revision + 1;
        writes.push(SeedWrite {
            seed: s,
            expected_revision: Some(before.revision),
        });
    }
    let (seeds, tx) = finish(b, ctx, &snap, ids, writes, "seeds:close")?;
    Ok((seeds, tx, warnings))
}

fn finish(
    b: &mut dyn Backend,
    ctx: &Ctx,
    snap: &Snapshot,
    ids: &[String],
    writes: Vec<SeedWrite>,
    source: &str,
) -> Result<(Vec<Seed>, u64)> {
    let mut out: BTreeMap<String, Seed> = BTreeMap::new();
    for w in &writes {
        out.insert(w.seed.id.clone(), w.seed.clone());
    }
    let tx = if writes.is_empty() {
        snap.tx
    } else {
        b.commit(
            &WriteBatch {
                seeds: writes,
                comments: vec![],
                source: source.into(),
                ..WriteBatch::default()
            },
            ctx,
        )?
    };
    let mut seeds = Vec::new();
    for id in ids {
        match out.get(id) {
            Some(s) => seeds.push(s.clone()),
            None => seeds.push(snap.get(id)?.clone()),
        }
    }
    Ok((seeds, tx))
}

// ---------------------------------------------------------------- dependencies

/// The result of `dep add` / `dep remove`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepChange {
    /// The dependent.
    pub issue_id: String,
    /// What it depends on.
    pub depends_on_id: String,
    /// The dependency type.
    pub dep_type: String,
    /// `added`, `removed`, or `unchanged` (it was already there).
    pub action: &'static str,
    /// The transaction.
    pub tx: u64,
}

/// `sd dep add <issue> <depends-on>`.
pub fn dep_add(
    b: &mut dyn Backend,
    ctx: &Ctx,
    issue: &str,
    depends_on: &str,
    dep_type: &str,
) -> Result<DepChange> {
    let dep_type = model::parse_dep_type(dep_type)?;
    if issue == depends_on {
        return Err(SdError::refused(format!("{issue} cannot depend on itself")));
    }
    let snap = b.snapshot(None)?;
    let before = snap.get(issue)?;
    snap.get(depends_on)?;
    if before.has_dep(depends_on, &dep_type) {
        return Ok(DepChange {
            issue_id: issue.into(),
            depends_on_id: depends_on.into(),
            dep_type,
            action: "unchanged",
            tx: snap.tx,
        });
    }
    if dep_type == "blocks" {
        if let Some(path) = blocks_path(&snap, depends_on, issue) {
            return Err(SdError::refused(format!(
                "{issue} -> {depends_on} would make a cycle ({}); nothing was written",
                path.join(" -> ")
            )));
        }
    }
    if dep_type == "parent-child" {
        if let Some(p) = &before.parent {
            return Err(SdError::refused(format!(
                "{issue} already has parent {p}; remove that dependency first"
            )));
        }
    }
    let mut s = before.clone();
    s.add_dep(depends_on, &dep_type);
    s.updated_at = ctx.now.clone();
    s.revision = before.revision + 1;
    let tx = b.commit(
        &WriteBatch {
            seeds: vec![SeedWrite {
                seed: s,
                expected_revision: Some(before.revision),
            }],
            comments: vec![],
            source: "seeds:dep-add".into(),
            ..WriteBatch::default()
        },
        ctx,
    )?;
    Ok(DepChange {
        issue_id: issue.into(),
        depends_on_id: depends_on.into(),
        dep_type,
        action: "added",
        tx,
    })
}

/// A `blocks` path from `from` to `to`, if one exists.
fn blocks_path(snap: &Snapshot, from: &str, to: &str) -> Option<Vec<String>> {
    let mut stack = vec![vec![from.to_string()]];
    let mut seen = BTreeSet::new();
    while let Some(path) = stack.pop() {
        let last = path.last().expect("non-empty").clone();
        if last == to {
            return Some(path);
        }
        if !seen.insert(last.clone()) {
            continue;
        }
        if let Some(s) = snap.seeds.get(&last) {
            for next in &s.blocked_on {
                let mut p = path.clone();
                p.push(next.clone());
                stack.push(p);
            }
        }
    }
    None
}

/// `sd dep remove <issue> <depends-on>`.
pub fn dep_remove(
    b: &mut dyn Backend,
    ctx: &Ctx,
    issue: &str,
    depends_on: &str,
    dep_type: &str,
) -> Result<DepChange> {
    let dep_type = model::parse_dep_type(dep_type)?;
    let snap = b.snapshot(None)?;
    let before = snap.get(issue)?;
    if !before.has_dep(depends_on, &dep_type) {
        return Err(SdError::new(
            crate::error::ErrorKind::NotFound,
            format!("{issue} has no {dep_type} dependency on {depends_on}"),
        ));
    }
    let mut s = before.clone();
    s.remove_dep(depends_on, &dep_type);
    s.updated_at = ctx.now.clone();
    s.revision = before.revision + 1;
    let tx = b.commit(
        &WriteBatch {
            seeds: vec![SeedWrite {
                seed: s,
                expected_revision: Some(before.revision),
            }],
            comments: vec![],
            source: "seeds:dep-remove".into(),
            ..WriteBatch::default()
        },
        ctx,
    )?;
    Ok(DepChange {
        issue_id: issue.into(),
        depends_on_id: depends_on.into(),
        dep_type,
        action: "removed",
        tx,
    })
}

/// One row of `dep list`.
#[derive(Debug, Clone)]
pub struct DepRow {
    /// The dependent.
    pub issue_id: String,
    /// What it depends on.
    pub depends_on_id: String,
    /// The type.
    pub dep_type: &'static str,
    /// The other end of the edge (the target for `down`, the dependent for `up`).
    pub other: Option<Seed>,
}

/// `sd dep list <id>`: what `id` depends on (`up == false`) or what depends
/// on it (`up == true`), as of `at`.
pub fn dep_list(b: &dyn Backend, id: &str, up: bool, at: Option<u64>) -> Result<Vec<DepRow>> {
    let snap = b.snapshot(at)?;
    let seed = snap.get(id)?;
    let rows = if up {
        snap.dependents(id)
            .into_iter()
            .map(|(d, t)| DepRow {
                issue_id: d.clone(),
                depends_on_id: id.into(),
                dep_type: t,
                other: snap.seeds.get(&d).cloned(),
            })
            .collect()
    } else {
        seed.dependencies()
            .into_iter()
            .map(|(t, ty)| DepRow {
                issue_id: id.into(),
                depends_on_id: t.clone(),
                dep_type: ty,
                other: snap.seeds.get(&t).cloned(),
            })
            .collect()
    };
    Ok(rows)
}

// ---------------------------------------------------------------- comments

/// `sd comments add`.
pub fn comment_add(
    b: &mut dyn Backend,
    ctx: &Ctx,
    id: &str,
    text: &str,
    author: Option<&str>,
) -> Result<(Comment, u64)> {
    if text.trim().is_empty() {
        return Err(SdError::usage("a comment needs non-empty text"));
    }
    let snap = b.snapshot(None)?;
    snap.get(id)?;
    let index = snap
        .comments_on(id)
        .iter()
        .map(|c| c.index)
        .max()
        .unwrap_or(0)
        + 1;
    let comment = Comment {
        seed: id.into(),
        index,
        author: author
            .map(str::to_string)
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| ctx.actor.clone()),
        text: text.to_string(),
        created_at: ctx.now.clone(),
    };
    let tx = b.commit(
        &WriteBatch {
            seeds: vec![],
            comments: vec![comment.clone()],
            source: "seeds:comment".into(),
            ..WriteBatch::default()
        },
        ctx,
    )?;
    Ok((comment, tx))
}

/// `sd comments list <id>`, as of `at`.
pub fn comment_list(b: &dyn Backend, id: &str, at: Option<u64>) -> Result<Vec<Comment>> {
    let snap = b.snapshot(at)?;
    snap.get(id)?;
    Ok(snap.comments_on(id).into_iter().cloned().collect())
}

// ---------------------------------------------------------------- helpers

/// `None` for an absent or blank value; otherwise the value as given.
fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty()).map(str::to_string)
}

fn clean_labels(labels: &[String]) -> BTreeSet<String> {
    labels
        .iter()
        .flat_map(|l| l.split(','))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

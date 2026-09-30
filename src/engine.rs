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
use crate::error::{Result, SdError};
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
    let id = match &req.parent {
        Some(p) => {
            snap.get(p)?;
            ids::child(p, |c| snap.seeds.contains_key(c))
        }
        None => ids::mint(&ctx.prefix, title, &ctx.now, |c| snap.seeds.contains_key(c)),
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
        workflow_run: non_empty(req.workflow_run.as_deref()).map(|r| vocab::run_iri(&r)),
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
    let tx = b.commit(
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
    )?;
    Ok((seed, tx))
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
    Ok(page(seeds, req.limit.unwrap_or(DEFAULT_LIST_LIMIT)))
}

fn page(mut seeds: Vec<Seed>, limit: usize) -> Page {
    let total = seeds.len();
    if limit > 0 && seeds.len() > limit {
        seeds.truncate(limit);
    }
    Page {
        has_more: seeds.len() < total,
        issues: seeds,
        total,
        limit,
    }
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
    let page = page(seeds, req.limit.unwrap_or(DEFAULT_LIST_LIMIT));
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
    Ok(page(seeds, req.limit.unwrap_or(0)))
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
    if status == "closed" {
        if s.status != "closed" {
            s.closed_at = Some(now.to_string());
        }
        if let Some(r) = reason {
            s.close_reason = non_empty(Some(r));
        }
    } else {
        s.closed_at = None;
        s.close_reason = None;
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
        set_status(&mut s, "closed", &ctx.now, reason);
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

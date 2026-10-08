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

use crate::backend::{merge_snapshots, Backend, Ctx, SeedQuery, SeedWrite, WriteBatch};
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
    /// Owner (br's `owner`, usually an email).
    pub owner: Option<String>,
    pub assignee: Option<String>,
    /// Acceptance criteria (br's `acceptance_criteria`).
    pub acceptance_criteria: Option<String>,
    /// A reference to the same work elsewhere (br's `external_ref`).
    pub external_ref: Option<String>,
    /// Due date, br's forms.
    pub due: Option<String>,
    /// Time estimate in minutes.
    pub estimate: Option<String>,
    /// Labels.
    pub labels: Vec<String>,
    /// Parent seed: the new seed is minted as `<parent>.<n>`.
    pub parent: Option<String>,
    /// A human-readable slug embedded in the id (br's `--slug`); ignored under
    /// `parent`, as br does.
    pub slug: Option<String>,
    /// Governing instructions for an agent (br's `agent_context`): JSON text.
    pub agent_context: Option<String>,
    /// Create the seed in the project's ephemeral graph (br's `--ephemeral`):
    /// visible to every read, never shared or exported, never ready.
    pub ephemeral: bool,
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
    /// Initial status (open, in_progress, blocked, deferred); default open.
    pub status: Option<String>,
    /// Defer until this (br's forms); implies status deferred.
    pub defer: Option<String>,
    pub dry_run: bool,
}

/// Create a seed. Returns it and the transaction that wrote it (0 on a dry run).
/// What `create` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    /// The seed (the existing one, for a keyed create that found it).
    pub seed: Seed,
    /// The transaction, or 0 when there is none to report: a dry run, an
    /// existing keyed seed, or a remote store (quipu's /update returns no tx).
    pub tx: u64,
    /// A keyed create found the seed already there and wrote nothing.
    pub existed: bool,
}

pub fn create(b: &mut dyn Backend, ctx: &Ctx, req: &CreateReq) -> Result<(Seed, u64)> {
    create_outcome(b, ctx, req).map(|c| (c.seed, c.tx))
}

/// A new seed from a create request, checked against `snap` (its deps must
/// exist there). Shared by `sd create` and `sd create --file`, so one seed
/// and a bulk import are built the same way.
fn new_seed(
    snap: &Snapshot,
    ctx: &Ctx,
    req: &CreateReq,
    id: String,
    title: &str,
    run: Option<String>,
) -> Result<Seed> {
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
        owner: non_empty(req.owner.as_deref().map(str::trim)),
        acceptance_criteria: non_empty(req.acceptance_criteria.as_deref()),
        external_ref: non_empty(req.external_ref.as_deref().map(str::trim)),
        agent_context: match non_empty(req.agent_context.as_deref()) {
            Some(c) => Some(agent_context(&c)?),
            None => None,
        },
        due_at: match non_empty(req.due.as_deref()) {
            Some(d) => Some(parse_when(&d, &ctx.now, "--due")?),
            None => None,
        },
        estimated_minutes: req.estimate.as_deref().map(parse_estimate).transpose()?,
        labels: clean_labels(&req.labels),
        created_at: ctx.now.clone(),
        created_by: non_empty(Some(&ctx.actor)),
        updated_at: ctx.now.clone(),
        parent: req.parent.clone(),
        workflow_run: run,
        revision: 1,
        ephemeral: req.ephemeral,
        ..Seed::default()
    };
    for spec in &req.deps {
        let (dep_type, target) = parse_dep_spec(spec)?;
        snap.get(&target)?;
        seed.add_dep(&target, &dep_type);
    }
    for (target, kind) in seed.dependencies() {
        seed.set_dependency_origin(&target, kind, &ctx.now, &ctx.actor)?;
    }
    if let Some(st) = &req.status {
        let st = model::parse_status(st)?;
        if st == model::TOMBSTONE || st == "closed" {
            return Err(SdError::usage(format!(
                "create cannot start a seed as {st}; create it, then sd close or sd delete it"
            )));
        }
        seed.status = st;
    }
    if let Some(d) = &req.defer {
        if req.status.as_deref().is_some_and(|s| s != "deferred") {
            return Err(SdError::usage(
                "--defer makes the seed deferred; do not combine it with another --status",
            ));
        }
        seed.status = "deferred".into();
        seed.defer_until = Some(parse_until(d, &ctx.now)?);
    }
    Ok(seed)
}

/// One seed of a bulk create: the request, plus notes and design (which
/// `sd create` has no flag for, but a markdown item can carry).
#[derive(Debug, Clone, Default)]
pub struct BulkItem {
    /// The fields `sd create` takes.
    pub req: CreateReq,
    /// Notes.
    pub notes: Option<String>,
    /// Design notes.
    pub design: Option<String>,
}

/// `sd create --file`: every item becomes a seed in ONE transaction, or none
/// does. Ids are minted against the store and the batch, so two items with
/// the same title never collide.
pub fn create_many(
    b: &mut dyn Backend,
    ctx: &Ctx,
    items: &[BulkItem],
    dry_run: bool,
) -> Result<(Vec<Seed>, u64)> {
    let snap = b.snapshot(None)?;
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut seeds = Vec::new();
    for item in items {
        let title = item.req.title.trim();
        if title.is_empty() {
            return Err(SdError::usage("a seed needs a non-empty title"));
        }
        let id = ids::mint(&ctx.prefix, title, &ctx.now, |c| {
            snap.seeds.contains_key(c) || minted.contains(c)
        });
        minted.insert(id.clone());
        let mut seed = new_seed(&snap, ctx, &item.req, id, title, None)?;
        seed.notes = non_empty(item.notes.as_deref());
        seed.design = non_empty(item.design.as_deref());
        seeds.push(seed);
    }
    if dry_run || seeds.is_empty() {
        return Ok((seeds, 0));
    }
    let tx = b.commit(
        &WriteBatch {
            seeds: seeds
                .iter()
                .map(|s| SeedWrite {
                    seed: s.clone(),
                    expected_revision: None,
                })
                .collect(),
            source: "seeds:create".into(),
            ..WriteBatch::default()
        },
        ctx,
    )?;
    Ok((seeds, tx))
}

/// Parse br's bulk-create markdown (`br create --file`): each `## Title`
/// starts a seed; `### Priority`, `### Type`, `### Labels`, `### Assignee`,
/// `### Dependencies`, `### Description` and `### Notes` set its fields; the
/// text between the title and its first `###` is the description. Headings
/// inside fenced code blocks are text. Anything before the first `##` is
/// ignored, as br does.
///
/// Unlike br, nothing is dropped silently: a section seeds cannot store
/// (`Design`, `Acceptance Criteria`) or does not know is refused by name, as
/// is an item with both a body and a `### Description`. br keeps only the
/// first paragraph of a body; seeds keeps all of it.
pub fn parse_bulk_markdown(text: &str) -> Result<Vec<BulkItem>> {
    struct Raw {
        title: String,
        body: Vec<String>,
        sections: Vec<(String, Vec<String>)>,
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        let heading = |p: &str| (!fence).then(|| t.strip_prefix(p)).flatten();
        if let Some(title) = heading("## ") {
            raws.push(Raw {
                title: title.trim().to_string(),
                body: vec![],
                sections: vec![],
            });
            continue;
        }
        let Some(cur) = raws.last_mut() else {
            continue;
        };
        if let Some(name) = heading("### ") {
            cur.sections.push((name.trim().to_string(), vec![]));
            continue;
        }
        match cur.sections.last_mut() {
            Some((_, lines)) => lines.push(line.to_string()),
            None => cur.body.push(line.to_string()),
        }
    }
    if raws.is_empty() {
        return Err(SdError::usage(
            "no items: each seed starts with a `## Title` heading",
        ));
    }
    let list = |v: &str| -> Vec<String> {
        v.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    };
    let mut items = Vec::new();
    for raw in raws {
        let mut item = BulkItem::default();
        item.req.title = raw.title.clone();
        let body = raw.body.join("\n").trim().to_string();
        let mut explicit = None;
        for (name, lines) in raw.sections {
            let value = lines.join("\n").trim().to_string();
            let one = value.clone();
            match name.to_ascii_lowercase().as_str() {
                "description" => explicit = Some(value),
                "notes" => item.notes = Some(value),
                "design" => item.design = Some(value),
                "acceptance criteria" => item.req.acceptance_criteria = Some(value),
                "priority" => item.req.priority = Some(one),
                "type" => item.req.issue_type = Some(one),
                "assignee" => item.req.assignee = Some(one),
                "labels" => item.req.labels = list(&value),
                "dependencies" | "deps" => item.req.deps = list(&value),
                _ => {
                    return Err(SdError::usage(format!(
                        "item {:?}: unknown section `### {name}`; nothing was written. Known: \
                         Description, Notes, Design, Acceptance Criteria, Priority, Type, \
                         Assignee, Labels, Dependencies",
                        raw.title
                    )))
                }
            }
        }
        item.req.description = match (body.is_empty(), explicit) {
            (true, d) => d,
            (false, None) => Some(body),
            (false, Some(_)) => {
                return Err(SdError::usage(format!(
                    "item {:?} has both text under its title and a `### Description`; \
                     keep one, so neither is dropped",
                    raw.title
                )))
            }
        };
        items.push(item);
    }
    Ok(items)
}

/// [`create`], saying explicitly whether a keyed create found an existing
/// seed. Callers must branch on `existed`, never on `tx == 0`: a remote store
/// reports tx 0 for every write.
pub fn create_outcome(b: &mut dyn Backend, ctx: &Ctx, req: &CreateReq) -> Result<Created> {
    let title = req.title.trim();
    if title.is_empty() {
        return Err(SdError::usage("a seed needs a non-empty title"));
    }
    let run = non_empty(req.workflow_run.as_deref()).map(|r| vocab::run_iri(&r));
    let keyed = match (non_empty(req.step.as_deref()), &run) {
        (Some(step), Some(run)) => {
            if req.parent.is_some() {
                return Err(SdError::usage(
                    "--step mints the id from the run and step, so it cannot take --parent",
                ));
            }
            if req.slug.is_some() {
                return Err(SdError::usage(
                    "--step mints the id from the run and step, so it cannot take --slug",
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
    let mut referenced: Vec<String> = req.parent.iter().cloned().collect();
    referenced.extend(keyed.iter().cloned());
    for dependency in &req.deps {
        referenced.push(parse_dep_spec(dependency)?.1);
    }
    let mut snap = b.snapshot_items(&referenced)?;
    if let Some(id) = &keyed {
        if let Some(existing) = existing_keyed(&snap, id, run.as_deref())? {
            return Ok(Created {
                seed: existing,
                tx: 0,
                existed: true,
            });
        }
    }
    let id = loop {
        let candidate = match (&keyed, &req.parent) {
            (Some(id), _) => id.clone(),
            (None, Some(p)) => {
                snap.get(p)?;
                ids::child(p, |c| snap.seeds.contains_key(c))
            }
            (None, None) => match req.slug.as_deref().and_then(ids::slug) {
                Some(slug) => ids::mint_slugged(&ctx.prefix, &slug, title, &ctx.now, |c| {
                    snap.seeds.contains_key(c)
                }),
                None => ids::mint(&ctx.prefix, title, &ctx.now, |c| snap.seeds.contains_key(c)),
            },
        };
        if keyed.is_some() {
            break candidate;
        }
        let occupied = b.snapshot_items(std::slice::from_ref(&candidate))?;
        if !occupied.seeds.contains_key(&candidate) {
            break candidate;
        }
        // Feed only observed collisions back into the unchanged ID generator.
        // Native commit still requires absence atomically under the write lock.
        snap.seeds.extend(occupied.seeds);
    };
    let seed = new_seed(&snap, ctx, req, id, title, run.clone())?;
    if req.dry_run {
        return Ok(Created {
            seed,
            tx: 0,
            existed: false,
        });
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
        (Ok(tx), _) => Ok(Created {
            seed,
            tx,
            existed: false,
        }),
        // A keyed create that lost a race: the other writer created the same
        // seed between our read and our write, and the compare-and-set refused
        // ours. That is the idempotent outcome, not a failure.
        (Err(e), Some(id)) if e.kind == ErrorKind::Conflict => {
            let snap = b.snapshot(None)?;
            match existing_keyed(&snap, id, run.as_deref())? {
                Some(existing) => Ok(Created {
                    seed: existing,
                    tx: 0,
                    existed: true,
                }),
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
    let snap = if scoped(b, at) {
        show_scope(b, ids)?
    } else {
        b.snapshot(at)?
    };
    ids.iter().map(|id| view(&snap, id)).collect()
}

/// Whether a command should read only the items it answers from
/// ([`Backend::scoped_reads`]): current state on a backend that indexes it.
/// A pinned read (`--at`) keeps the whole snapshot as of that transaction.
fn scoped(b: &dyn Backend, at: Option<u64>) -> bool {
    at.is_none() && b.scoped_reads()
}

/// Everything [`view`] reads for `ids`: the seeds and their comments, the
/// seeds they depend on, and the seeds that depend on them.
fn show_scope(b: &dyn Backend, ids: &[String]) -> Result<Snapshot> {
    let mut snap = b.snapshot_items(ids)?;
    let targets: Vec<String> = ids
        .iter()
        .filter_map(|id| snap.seeds.get(id))
        .flat_map(|s| s.dependencies().into_iter().map(|(t, _)| t))
        .filter(|t| !snap.seeds.contains_key(t))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !targets.is_empty() {
        merge_snapshots(&mut snap, b.snapshot_seeds(&targets)?);
    }
    merge_snapshots(&mut snap, b.snapshot_dependents(ids)?);
    Ok(snap)
}

/// Add to `snap` the seeds that depend on any of `ids`, so
/// [`Snapshot::dependents`] (a page's dependent counts) answers for them.
fn with_dependents(b: &dyn Backend, snap: &mut Snapshot, ids: &[String]) -> Result<()> {
    if !ids.is_empty() {
        merge_snapshots(snap, b.snapshot_dependents(ids)?);
    }
    Ok(())
}

/// The ids on the page [`page_at`] will cut from `seeds`.
fn page_ids(seeds: &[Seed], offset: usize, limit: usize) -> Vec<String> {
    seeds
        .iter()
        .skip(offset)
        .take(if limit == 0 { usize::MAX } else { limit })
        .map(|s| s.id.clone())
        .collect()
}

/// The [`SeedQuery`] for `f` over the seeds a listing shows when no status
/// is given: every status except those `hidden` (seeds of a status no
/// writer of this build uses are shown, as a whole-snapshot listing shows
/// them).
fn listing_query(f: &CompiledFilter, hidden: &[&str]) -> SeedQuery {
    let (statuses, other_statuses) = match &f.status {
        Some(s) => (Some([s.clone()].into()), false),
        None if hidden.is_empty() => (None, false),
        None => (
            Some(
                model::STATUSES
                    .iter()
                    .filter(|s| !hidden.contains(s))
                    .map(|s| s.to_string())
                    .collect(),
            ),
            true,
        ),
    };
    SeedQuery {
        statuses,
        other_statuses,
        issue_type: f.issue_type.clone(),
        assignee: f.assignee.clone(),
        labels: f.labels.clone(),
        priority: f.priority,
        parent: f.parent.clone(),
    }
}

/// A superset of the seeds `f` (with `q` describing its pushable part) can
/// match, read as narrowly as the backend allows: the named ids when the
/// filter names some, else the seeds `q` selects.
fn matching_snapshot(b: &dyn Backend, f: &CompiledFilter, q: &SeedQuery) -> Result<Snapshot> {
    if f.ids.is_empty() {
        b.snapshot_where(q)
    } else {
        b.snapshot_seeds(&f.ids.iter().cloned().collect::<Vec<_>>())
    }
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    /// Title contains this (case-insensitive).
    pub title_contains: Option<String>,
    /// Description contains this (case-insensitive).
    pub desc_contains: Option<String>,
    /// Notes contain this (case-insensitive).
    pub notes_contains: Option<String>,
    /// Only seeds carrying ANY of these labels.
    pub labels_any: Vec<String>,
    /// Only priority >= this (0 = critical).
    pub priority_min: Option<String>,
    /// Only priority <= this.
    pub priority_max: Option<String>,
    /// Only these ids.
    pub ids: Vec<String>,
    /// Only seeds overdue at this instant: due before it and not closed
    /// (br's `--overdue`). The CLI passes the current time.
    pub overdue_at: Option<String>,
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
            title_contains: self.title_contains.as_deref().map(str::to_lowercase),
            desc_contains: self.desc_contains.as_deref().map(str::to_lowercase),
            notes_contains: self.notes_contains.as_deref().map(str::to_lowercase),
            labels_any: clean_labels(&self.labels_any),
            priority_min: self
                .priority_min
                .as_deref()
                .map(model::parse_priority)
                .transpose()?,
            priority_max: self
                .priority_max
                .as_deref()
                .map(model::parse_priority)
                .transpose()?,
            ids: self.ids.iter().cloned().collect(),
            overdue_at: match &self.overdue_at {
                Some(now) => Some(epoch_of(now).ok_or_else(|| {
                    SdError::usage(format!("--overdue: cannot read the time {now:?}"))
                })?),
                None => None,
            },
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
    title_contains: Option<String>,
    desc_contains: Option<String>,
    notes_contains: Option<String>,
    labels_any: BTreeSet<String>,
    priority_min: Option<u8>,
    priority_max: Option<u8>,
    ids: BTreeSet<String>,
    overdue_at: Option<i64>,
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
            && contains(&self.title_contains, Some(&s.title))
            && contains(&self.desc_contains, s.description.as_ref())
            && contains(&self.notes_contains, s.notes.as_ref())
            && (self.labels_any.is_empty() || !self.labels_any.is_disjoint(&s.labels))
            && self.priority_min.is_none_or(|p| s.priority >= p)
            && self.priority_max.is_none_or(|p| s.priority <= p)
            && (self.ids.is_empty() || self.ids.contains(&s.id))
            && self.overdue_at.is_none_or(|now| {
                s.status != "closed"
                    && !s.is_tombstone()
                    && s.due_at
                        .as_deref()
                        .and_then(epoch_of)
                        .is_some_and(|d| d < now)
            })
    }
}

fn contains(needle: &Option<String>, hay: Option<&String>) -> bool {
    needle
        .as_ref()
        .is_none_or(|n| hay.is_some_and(|h| h.to_lowercase().contains(n)))
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
    /// Skip this many results first (pagination).
    pub offset: usize,
    /// Reverse the sort order.
    pub reverse: bool,
    /// Include deferred seeds (hidden by default, as br does).
    pub deferred: bool,
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
    /// How many results were skipped before this page.
    pub offset: usize,
    /// True when `total > issues.len()`.
    pub has_more: bool,
    /// For each seed on the page, how many seeds declare any dependency on it
    /// (br's `dependent_count`: blocks, parent-child, related, discovered-from).
    pub dependent_counts: BTreeMap<String, usize>,
}

/// `sd list`, as of `at`.
pub fn list(b: &dyn Backend, req: &ListReq, at: Option<u64>) -> Result<Page> {
    let f = req.filter.compile()?;
    let scoped = scoped(b, at);
    if scoped {
        if let Some(page) = b.list_page(req)? {
            return Ok(page);
        }
    }
    let mut snap = if scoped {
        let mut hidden = vec![model::TOMBSTONE];
        if !req.all {
            hidden.push("closed");
            if !req.deferred {
                hidden.push("deferred");
            }
        }
        matching_snapshot(b, &f, &listing_query(&f, &hidden))?
    } else {
        b.snapshot(at)?
    };
    let mut seeds: Vec<Seed> = snap
        .seeds
        .values()
        .filter(|s| {
            f.status.is_some()
                || (!s.is_tombstone()
                    && (req.all || s.status != "closed")
                    && (req.all || req.deferred || s.status != "deferred"))
        })
        .filter(|s| f.matches(s))
        .cloned()
        .collect();
    sort_seeds(&mut seeds, req.sort.as_deref())?;
    if req.reverse {
        seeds.reverse();
    }
    let limit = req.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    let counts = if scoped {
        b.dependent_counts(&page_ids(&seeds, req.offset, limit))?
    } else {
        None
    };
    if scoped && counts.is_none() {
        with_dependents(b, &mut snap, &page_ids(&seeds, req.offset, limit))?;
    }
    let mut page = page_at(seeds, req.offset, limit, &snap);
    if let Some(counts) = counts {
        page.dependent_counts = counts;
    }
    Ok(page)
}

pub(crate) fn listing_seed_query(req: &ListReq) -> Result<SeedQuery> {
    let f = req.filter.compile()?;
    let mut hidden = vec![model::TOMBSTONE];
    if !req.all {
        hidden.push("closed");
        if !req.deferred {
            hidden.push("deferred");
        }
    }
    Ok(listing_query(&f, &hidden))
}

pub(crate) fn list_matches_page(req: &ListReq, snap: &Snapshot) -> Result<Page> {
    let f = req.filter.compile()?;
    let mut seeds = snap
        .seeds
        .values()
        .filter(|s| {
            f.status.is_some()
                || (!s.is_tombstone()
                    && (req.all || s.status != "closed")
                    && (req.all || req.deferred || s.status != "deferred"))
        })
        .filter(|s| f.matches(s))
        .cloned()
        .collect::<Vec<_>>();
    sort_seeds(&mut seeds, req.sort.as_deref())?;
    if req.reverse {
        seeds.reverse();
    }
    Ok(page_at(
        seeds,
        req.offset,
        req.limit.unwrap_or(DEFAULT_LIST_LIMIT),
        snap,
    ))
}

fn page(seeds: Vec<Seed>, limit: usize, snap: &Snapshot) -> Page {
    page_at(seeds, 0, limit, snap)
}

fn page_at(mut seeds: Vec<Seed>, offset: usize, limit: usize, snap: &Snapshot) -> Page {
    let total = seeds.len();
    seeds.drain(..offset.min(seeds.len()));
    if limit > 0 && seeds.len() > limit {
        seeds.truncate(limit);
    }
    let dependent_counts = seeds
        .iter()
        .map(|s| (s.id.clone(), snap.dependents(&s.id).len()))
        .collect();
    Page {
        has_more: offset + seeds.len() < total,
        issues: seeds,
        total,
        limit,
        offset,
        dependent_counts,
    }
}

/// `sd search`.
#[derive(Debug, Clone, Default)]
pub struct SearchReq {
    /// Text to find (case-insensitive substring).
    pub query: String,
    /// Include ids, descriptions and comments. The default searches titles.
    pub full: bool,
    /// Filters.
    pub filter: Filter,
    /// Include closed seeds (hidden, and counted, unless `--status` or `--all`).
    pub all: bool,
    /// Page size; `None` means [`DEFAULT_LIST_LIMIT`] (br's 50), `Some(0)` means all.
    pub limit: Option<usize>,
    /// Sort order (one of [`SORTS`]).
    pub sort: Option<String>,
    /// Skip this many results first (pagination).
    pub offset: usize,
    /// Reverse the sort order.
    pub reverse: bool,
    /// Include deferred seeds (hidden by default, as br does).
    pub deferred: bool,
}

/// A page of search hits and how many closed hits were hidden.
#[derive(Debug, Clone)]
pub struct SearchPage {
    /// The page.
    pub page: Page,
    /// Closed seeds that matched but were hidden (pass `--all` to see them).
    pub hidden_closed: usize,
    /// Whether the query searched all supported text fields.
    pub full: bool,
}

/// `sd search`: seeds whose id, title, description or any comment contains
/// the query, case-insensitively (br's fields; notes are not searched, as in br).
pub fn search(b: &dyn Backend, req: &SearchReq, at: Option<u64>) -> Result<SearchPage> {
    let query = req.query.trim().to_lowercase();
    if query.is_empty() {
        return Err(SdError::usage("search needs a non-empty query"));
    }
    let f = req.filter.compile()?;
    if scoped(b, at) {
        if let Some(page) = b.search_page(req)? {
            return Ok(page);
        }
    }
    let snap = b.snapshot(at)?;
    let hit = |s: &Seed| {
        let has = |t: &str| t.to_lowercase().contains(&query);
        has(&s.title)
            || (req.full
                && (has(&s.id)
                    || s.description.as_deref().is_some_and(has)
                    || snap.comments_on(&s.id).iter().any(|c| has(&c.text))))
    };
    let show_closed = req.all || f.status.is_some();
    let (mut seeds, mut hidden_closed) = (Vec::new(), 0);
    for s in snap
        .seeds
        .values()
        .filter(|s| f.matches(s) && hit(s) && (f.status.is_some() || !s.is_tombstone()))
        .filter(|s| f.status.is_some() || req.all || req.deferred || s.status != "deferred")
    {
        if s.status == "closed" && !show_closed {
            hidden_closed += 1;
        } else {
            seeds.push(s.clone());
        }
    }
    sort_seeds(&mut seeds, req.sort.as_deref())?;
    if req.reverse {
        seeds.reverse();
    }
    Ok(SearchPage {
        page: page_at(
            seeds,
            req.offset,
            req.limit.unwrap_or(DEFAULT_LIST_LIMIT),
            &snap,
        ),
        hidden_closed,
        full: req.full,
    })
}

/// Page seeds already selected by an indexed text search. The backend must
/// include every matching seed; filters and ordering remain the core's.
pub(crate) fn search_matches_page(req: &SearchReq, snap: &Snapshot) -> Result<SearchPage> {
    let f = req.filter.compile()?;
    let show_closed = req.all || f.status.is_some();
    let mut hidden_closed = 0;
    let mut seeds = Vec::new();
    for seed in snap
        .seeds
        .values()
        .filter(|s| f.matches(s) && (f.status.is_some() || !s.is_tombstone()))
        .filter(|s| f.status.is_some() || req.all || req.deferred || s.status != "deferred")
    {
        if seed.status == "closed" && !show_closed {
            hidden_closed += 1;
        } else {
            seeds.push(seed.clone());
        }
    }
    sort_seeds(&mut seeds, req.sort.as_deref())?;
    if req.reverse {
        seeds.reverse();
    }
    Ok(SearchPage {
        page: page_at(
            seeds,
            req.offset,
            req.limit.unwrap_or(DEFAULT_LIST_LIMIT),
            snap,
        ),
        hidden_closed,
        full: req.full,
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
                s.status != "closed" && !s.is_tombstone()
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
    /// Recent activity over this many hours back from now; `None` skips it.
    pub activity_hours: Option<u64>,
}

/// `sd stats` recent activity: what seed timestamps say happened in the
/// window. sd has no git commits and keeps no reopen record on the seed, so
/// br's `commit_count` and `issues_reopened` are not tracked (null in JSON).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// The window, in hours.
    pub hours: u64,
    /// Seeds created in the window.
    pub created: usize,
    /// Seeds closed in the window.
    pub closed: usize,
    /// Seeds created before the window, changed in it, and not closed in it.
    pub updated: usize,
    /// Distinct seeds touched in the window.
    pub touched: usize,
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
    /// Deleted (tombstoned); not counted in `total` or any other count.
    pub tombstones: usize,
    /// What `sd ready` would list now.
    pub ready: usize,
    /// Open epics whose children are all closed.
    pub epics_eligible_for_closure: usize,
    /// Mean hours from creation to close, over closed seeds (0 when none).
    pub average_lead_time_hours: f64,
    /// (dimension, [(key, count)]) in br's order: type, priority, assignee, label.
    pub breakdowns: Vec<(&'static str, Vec<(String, usize)>)>,
    /// Recent activity, when asked for.
    pub activity: Option<Activity>,
}

/// `sd stats`, as of `at`. Breakdowns count every seed, closed included (br).
pub fn stats(b: &dyn Backend, ctx: &Ctx, req: StatsReq, at: Option<u64>) -> Result<Stats> {
    if scoped(b, at) {
        if let Some(stats) = b.aggregate_stats(ctx, req)? {
            return Ok(stats);
        }
    }
    let snap = b.snapshot(at)?;
    let tombstones = snap.seeds.values().filter(|s| s.is_tombstone()).count();
    let seeds: Vec<&Seed> = snap.seeds.values().filter(|s| !s.is_tombstone()).collect();
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
        tombstones,
        ready: ready(b, ctx, &ReadyReq::default(), at)?.total,
        epics_eligible_for_closure: eligible,
        average_lead_time_hours: if lead.is_empty() {
            0.0
        } else {
            lead.iter().sum::<f64>() / lead.len() as f64
        },
        breakdowns,
        activity: match req.activity_hours {
            None => None,
            Some(h) => {
                let since = parse_since(&format!("{h}h"), &ctx.now)?;
                let at_or_after = |t: Option<&str>| t.is_some_and(|t| t >= since.as_str());
                let created = |s: &&&Seed| at_or_after(Some(&s.created_at));
                let closed = |s: &&&Seed| at_or_after(s.closed_at.as_deref());
                Some(Activity {
                    hours: h,
                    created: seeds.iter().filter(created).count(),
                    closed: seeds.iter().filter(closed).count(),
                    updated: seeds
                        .iter()
                        .filter(|s| at_or_after(Some(&s.updated_at)) && !created(s) && !closed(s))
                        .count(),
                    touched: seeds
                        .iter()
                        .filter(|s| at_or_after(Some(&s.updated_at)))
                        .count(),
                })
            }
        },
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
        .filter(|e| e.issue_type == "epic" && e.status != "closed" && !e.is_tombstone())
    {
        let kids: Vec<&Seed> = snap
            .seeds
            .values()
            .filter(|c| c.parent.as_deref() == Some(e.id.as_str()) && !c.is_tombstone())
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
    epic_close_eligible_with(b, ctx, None)
}

/// [`epic_close_eligible`] with a comment on each closed epic, in the same tx.
pub fn epic_close_eligible_with(
    b: &mut dyn Backend,
    ctx: &Ctx,
    comment: Option<&str>,
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
    let (closed, tx, _) = close_as(
        b,
        ctx,
        &ids,
        Some("All children completed"),
        None,
        false,
        comment,
    )?;
    Ok((closed, skipped, tx))
}

/// One node of `sd graph`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    /// The seed.
    pub seed: Seed,
    /// Breadth-first distance from the root (or from the component's roots).
    pub depth: usize,
}

/// A connected piece of `sd graph`: nodes, `[waiter, waited_on]` edges, roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graph {
    /// Nodes in breadth-first order.
    pub nodes: Vec<GraphNode>,
    /// `(from, to)`: `from` waits on `to`.
    pub edges: Vec<(String, String)>,
    /// Where the walk started.
    pub roots: Vec<String>,
}

/// What a live seed waits on, br's way: its `blocks` targets, and (for a
/// parent) its children, since a parent waits on its children. Tombstones
/// take no part.
fn waits_on(snap: &Snapshot, s: &Seed) -> Vec<String> {
    let live = |id: &String| snap.seeds.get(id).is_some_and(|t| !t.is_tombstone());
    let mut out: Vec<String> = s.blocked_on.iter().filter(|t| live(t)).cloned().collect();
    out.extend(
        snap.seeds
            .values()
            .filter(|c| c.parent.as_deref() == Some(s.id.as_str()) && !c.is_tombstone())
            .map(|c| c.id.clone()),
    );
    out
}

/// One BFS step: the next node, and the `(from, to)` edge that reached it.
type Step = (String, (String, String));

fn bfs(snap: &Snapshot, roots: &[String], next: &dyn Fn(&Seed) -> Vec<Step>) -> Graph {
    let mut seen: BTreeSet<String> = roots.iter().cloned().collect();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut queue: std::collections::VecDeque<(String, usize)> =
        roots.iter().map(|r| (r.clone(), 0)).collect();
    while let Some((id, depth)) = queue.pop_front() {
        let Some(s) = snap.seeds.get(&id) else {
            continue;
        };
        nodes.push(GraphNode {
            seed: s.clone(),
            depth,
        });
        for (n, edge) in next(s) {
            if !edges.contains(&edge) {
                edges.push(edge);
            }
            if seen.insert(n.clone()) {
                queue.push_back((n, depth + 1));
            }
        }
    }
    Graph {
        nodes,
        edges,
        roots: roots.to_vec(),
    }
}

/// `sd graph <id>`: what the seed unblocks (its `blocks` dependents,
/// transitively), or with `dependencies` what it waits on (blockers and, for
/// a parent, children). Shapes as br.
pub fn graph(b: &dyn Backend, id: &str, dependencies: bool, at: Option<u64>) -> Result<Graph> {
    let snap = b.snapshot(at)?;
    snap.get(id)?;
    let roots = [id.to_string()];
    Ok(if dependencies {
        bfs(&snap, &roots, &|s| {
            waits_on(&snap, s)
                .into_iter()
                .map(|t| (t.clone(), (s.id.clone(), t)))
                .collect()
        })
    } else {
        bfs(&snap, &roots, &|s| {
            snap.seeds
                .values()
                .filter(|d| !d.is_tombstone() && d.blocked_on.contains(&s.id))
                .map(|d| (d.id.clone(), (d.id.clone(), s.id.clone())))
                .collect()
        })
    })
}

/// `sd graph --all`: the connected components of every open, in-progress or
/// blocked seed. Each component is rooted at the seeds that wait on nothing
/// and walked toward what waits on them.
pub fn graph_all(b: &dyn Backend, at: Option<u64>) -> Result<Vec<Graph>> {
    let snap = b.snapshot(at)?;
    let active = |s: &Seed| matches!(s.status.as_str(), "open" | "in_progress" | "blocked");
    let members: Vec<&Seed> = snap.seeds.values().filter(|s| active(s)).collect();
    let ids: BTreeSet<String> = members.iter().map(|s| s.id.clone()).collect();
    let mut out_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut in_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in &members {
        for t in waits_on(&snap, s).into_iter().filter(|t| ids.contains(t)) {
            out_edges.entry(s.id.clone()).or_default().push(t.clone());
            in_edges.entry(t).or_default().push(s.id.clone());
        }
    }
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut graphs = Vec::new();
    for s in &members {
        if done.contains(&s.id) {
            continue;
        }
        // The component, ignoring direction.
        let mut comp: BTreeSet<String> = BTreeSet::new();
        let mut stack = vec![s.id.clone()];
        while let Some(id) = stack.pop() {
            if comp.insert(id.clone()) {
                for n in out_edges
                    .get(&id)
                    .into_iter()
                    .chain(in_edges.get(&id))
                    .flatten()
                {
                    stack.push(n.clone());
                }
            }
        }
        done.extend(comp.iter().cloned());
        let roots: Vec<String> = comp
            .iter()
            .filter(|id| out_edges.get(*id).is_none_or(Vec::is_empty))
            .cloned()
            .collect();
        let roots = if roots.is_empty() {
            vec![s.id.clone()]
        } else {
            roots
        };
        graphs.push(bfs(&snap, &roots, &|x| {
            in_edges
                .get(&x.id)
                .into_iter()
                .flatten()
                .map(|w| (w.clone(), (w.clone(), x.id.clone())))
                .collect()
        }));
    }
    Ok(graphs)
}

/// One version of a seed in `sd history`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The transaction that produced this version.
    pub tx: u64,
    /// The seed as that transaction left it.
    pub seed: Seed,
    /// What changed from the previous version, as `field: old (before) vs new
    /// (after)`; empty for the first version.
    pub changes: Vec<String>,
    /// What the writer of this version claimed to be (`--agent-name`,
    /// `--harness`, `--model`): self-asserted, never verified.
    pub claims: Option<crate::backend::Claims>,
}

/// The seed's revision as of `tx`, or 0 if it did not exist yet.
fn revision_at(b: &dyn Backend, id: &str, tx: u64) -> Result<u64> {
    Ok(b.snapshot(Some(tx))?
        .seeds
        .get(id)
        .map_or(0, |s| s.revision))
}

/// The first transaction from `lo` at which the seed's revision exceeds
/// `after` (revisions only grow). Galloping, then binary search, over `--at`
/// reads: it needs no head transaction, which a quipu server does not report,
/// because a read as of a tx past the head is the current state.
fn first_tx_after(b: &dyn Backend, id: &str, after: u64, mut lo: u64) -> Result<u64> {
    let mut hi = lo.max(1);
    while revision_at(b, id, hi)? <= after {
        lo = hi + 1;
        hi = hi.checked_mul(2).filter(|h| *h < 1 << 48).ok_or_else(|| {
            SdError::failed(format!("{id}: no version after revision {after} was found"))
        })?;
    }
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if revision_at(b, id, mid)? > after {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(lo)
}

/// `sd history <id>`: every version of one seed, oldest first, with the
/// transaction that wrote it and what changed. This is the seed's FACT
/// history in quipu (what `--at` reads), not br's local backup files.
pub fn history(b: &dyn Backend, id: &str) -> Result<Vec<HistoryEntry>> {
    let current = b.snapshot(None)?.get(id)?.clone();
    let claims = b.claims_of(id)?;
    let mut out: Vec<HistoryEntry> = Vec::new();
    let (mut after, mut lo) = (0u64, 1u64);
    while after < current.revision {
        let tx = first_tx_after(b, id, after, lo)?;
        let snap = b.snapshot(Some(tx))?;
        let Some(seed) = snap.seeds.get(id).cloned() else {
            break;
        };
        if seed.revision <= after {
            break; // never advanced: the head is not readable as a tx
        }
        let changes = out
            .last()
            .map(|p| crate::sync::diff_fields(&p.seed, &seed, "before", "after"))
            .unwrap_or_default();
        after = seed.revision;
        lo = tx + 1;
        // A version is matched to the write that produced its revision.
        let claimed = claims
            .iter()
            .find(|(rev, _)| *rev == seed.revision)
            .map(|(_, c)| c.clone());
        out.push(HistoryEntry {
            tx,
            seed,
            changes,
            claims: claimed,
        });
    }
    Ok(out)
}

/// One type's section of `sd changelog`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogGroup {
    /// The issue type.
    pub issue_type: String,
    /// Its heading ("Bugs", ...).
    pub label: String,
    /// Closed seeds of that type, most recently closed first.
    pub issues: Vec<Seed>,
}

/// Where a `--since` value starts, against `now`: a date or RFC 3339 instant
/// as given, or a relative span back from now (`+7d`, `-2w`, `12h`: br's
/// "relative like +7d" means the last 7 days).
pub fn parse_since(value: &str, now: &str) -> Result<String> {
    let v = value.trim();
    let span = v.trim_start_matches(['+', '-']);
    if span.len() > 1 && span[..span.len() - 1].chars().all(|c| c.is_ascii_digit()) {
        let forward = parse_until(&format!("+{span}"), now)?;
        let secs = parse_instant(&forward).and_then(|f| Some(f - parse_instant(now)?));
        let back = secs.and_then(|s| Some(format_instant(parse_instant(now)? - s)));
        return back.ok_or_else(|| SdError::usage(format!("cannot read --since {value:?}")));
    }
    parse_until(v, now).map_err(|_| {
        SdError::usage(format!(
            "cannot read --since {value:?}; use YYYY-MM-DD, an RFC 3339 instant, or +7d/+2w/+12h"
        ))
    })
}

/// `sd changelog`: closed seeds (closed at or after `since`, if given),
/// grouped by type in a fixed order, most recently closed first (br).
pub fn changelog(
    b: &dyn Backend,
    since: Option<&str>,
    at: Option<u64>,
) -> Result<Vec<ChangelogGroup>> {
    const ORDER: [(&str, &str); 7] = [
        ("epic", "Epics"),
        ("feature", "Features"),
        ("bug", "Bugs"),
        ("task", "Tasks"),
        ("chore", "Chores"),
        ("docs", "Docs"),
        ("question", "Questions"),
    ];
    let snap = b.snapshot(at)?;
    let closed: Vec<&Seed> = snap
        .seeds
        .values()
        .filter(|s| s.status == "closed")
        .filter(|s| since.is_none_or(|from| s.closed_at.as_deref().is_some_and(|c| c >= from)))
        .collect();
    let mut groups = Vec::new();
    for (ty, label) in ORDER {
        let mut issues: Vec<Seed> = closed
            .iter()
            .filter(|s| s.issue_type == ty)
            .map(|s| (*s).clone())
            .collect();
        if issues.is_empty() {
            continue;
        }
        issues.sort_by(|a, b| (&b.closed_at, &a.id).cmp(&(&a.closed_at, &b.id)));
        groups.push(ChangelogGroup {
            issue_type: ty.into(),
            label: label.into(),
            issues,
        });
    }
    Ok(groups)
}

/// The description sections br's templates expect, by type, with br's hint.
pub fn template_sections(issue_type: &str) -> &'static [(&'static str, &'static str)] {
    match issue_type {
        "bug" => &[
            ("## Steps to Reproduce", "Describe how to reproduce the bug"),
            (
                "## Acceptance Criteria",
                "Define criteria to verify the fix",
            ),
        ],
        "task" | "feature" => &[(
            "## Acceptance Criteria",
            "Define criteria to verify completion",
        )],
        "epic" => &[("## Success Criteria", "Define high-level success criteria")],
        _ => &[],
    }
}

/// Whether `description` has a heading (any level) named like `section`.
fn has_section(description: Option<&str>, section: &str) -> bool {
    let want = section.trim_start_matches('#').trim();
    description.is_some_and(|d| {
        d.lines().any(|l| {
            let l = l.trim();
            l.starts_with('#') && l.trim_start_matches('#').trim().eq_ignore_ascii_case(want)
        })
    })
}

/// One seed's missing template sections in `sd lint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintResult {
    /// The seed.
    pub seed: Seed,
    /// (section, hint) pairs missing from its description.
    pub missing: Vec<(&'static str, &'static str)>,
}

/// `sd lint [ids]`: seeds whose description lacks their type's template
/// sections. Without ids: open seeds, or `status` (a status, or "all"),
/// optionally of one type. Only seeds with something missing are returned.
pub fn lint(
    b: &dyn Backend,
    ids: &[String],
    issue_type: Option<&str>,
    status: Option<&str>,
    at: Option<u64>,
) -> Result<Vec<LintResult>> {
    let snap = b.snapshot(at)?;
    let issue_type = issue_type.map(model::parse_type).transpose()?;
    let status = match status {
        None => Some("open".to_string()),
        Some("all") => None,
        Some(s) => Some(model::parse_status(s)?),
    };
    let seeds: Vec<&Seed> = if ids.is_empty() {
        snap.seeds
            .values()
            .filter(|s| !s.is_tombstone())
            .filter(|s| status.as_ref().is_none_or(|st| &s.status == st))
            .filter(|s| issue_type.as_ref().is_none_or(|t| &s.issue_type == t))
            .collect()
    } else {
        ids.iter().map(|id| snap.get(id)).collect::<Result<_>>()?
    };
    Ok(seeds
        .into_iter()
        .filter_map(|s| {
            let missing: Vec<_> = template_sections(&s.issue_type)
                .iter()
                .filter(|(sec, _)| !has_section(s.description.as_deref(), sec))
                .copied()
                .collect();
            (!missing.is_empty()).then(|| LintResult {
                seed: s.clone(),
                missing,
            })
        })
        .collect())
}

/// A commit, as `sd orphans` reads it: (short hash, subject, body).
pub type CommitText = (String, String, String);

/// The tokens of a commit message that could be seed ids: runs of letters,
/// digits, `-`, `.`, `_`, with trailing `.` dropped (end of a sentence).
fn id_tokens(text: &str) -> BTreeSet<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_'))
        .map(|t| t.trim_end_matches('.'))
        .filter(|t| !t.is_empty())
        .collect()
}

/// `sd orphans`: open or in-progress seeds that a commit mentions, each with
/// the newest such commit (`commits` newest first). Ids match as whole tokens,
/// so `sd-a3f` does not match inside `sd-a3f.1`.
pub fn orphans(
    b: &dyn Backend,
    commits: &[CommitText],
    at: Option<u64>,
) -> Result<Vec<(Seed, CommitText)>> {
    let snap = b.snapshot(at)?;
    let open: BTreeMap<&str, &Seed> = snap
        .seeds
        .values()
        .filter(|s| s.status == "open" || s.status == "in_progress")
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut found: BTreeMap<String, (Seed, CommitText)> = BTreeMap::new();
    for c in commits {
        let text = format!("{}\n{}", c.1, c.2);
        for t in id_tokens(&text) {
            if let Some(s) = open.get(t) {
                found
                    .entry(s.id.clone())
                    .or_insert_with(|| ((*s).clone(), c.clone()));
            }
        }
    }
    let mut out: Vec<(Seed, CommitText)> = found.into_values().collect();
    sort_seeds_by(&mut out);
    Ok(out)
}

fn sort_seeds_by(v: &mut [(Seed, CommitText)]) {
    v.sort_by(|a, b| (a.0.priority, &a.0.id).cmp(&(b.0.priority, &b.0.id)));
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
    /// Those blockers themselves, by id (for `blocked --detailed`).
    pub blockers: BTreeMap<String, Seed>,
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
    for s in snap
        .seeds
        .values()
        .filter(|s| s.status != "closed" && !s.is_tombstone())
    {
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
    let blockers = blocked_by
        .values()
        .flatten()
        .filter_map(|id| snap.seeds.get(id).map(|s| (id.clone(), s.clone())))
        .collect();
    Ok(BlockedPage {
        page,
        blocked_by,
        blockers,
    })
}

/// `sd ready`.
#[derive(Debug, Clone, Default)]
pub struct ReadyReq {
    /// Filters (its `status` is ignored: ready means open).
    pub filter: Filter,
    /// `None` = every ready seed (the default: never silently short).
    pub limit: Option<usize>,
    /// `hybrid` (P0/P1 first, then by age), `priority` or `oldest`; `None` is
    /// `priority`, sd's order before `--sort` existed.
    pub sort: Option<String>,
    /// Also list deferred seeds: status `deferred`, or a defer date not yet reached.
    pub include_deferred: bool,
    /// With `filter.parent`: every descendant, not only direct children.
    pub recursive: bool,
}

/// The `sd ready --sort` policies (br's).
pub const READY_SORTS: &[&str] = &["hybrid", "priority", "oldest"];

/// `sd ready`: open seeds with no open `blocks` dependency and no future defer
/// date, as of `at`. Unlimited unless `limit` is given; the page says when it
/// was cut.
pub fn ready(b: &dyn Backend, ctx: &Ctx, req: &ReadyReq, at: Option<u64>) -> Result<Page> {
    let sort = req.sort.as_deref().unwrap_or("priority");
    if !READY_SORTS.contains(&sort) {
        return Err(SdError::usage(format!(
            "unknown sort {sort:?}; expected one of {}",
            READY_SORTS.join(", ")
        )));
    }
    let mut filter = req.filter.clone();
    filter.status = None;
    // --recursive widens --parent to a descendant set; without a parent it
    // would do nothing, so it is refused rather than ignored.
    let scope = match (&filter.parent, req.recursive) {
        (None, true) => {
            return Err(SdError::usage(
                "--recursive needs --parent (or use --epic <id>)",
            ))
        }
        (Some(_), true) => filter.parent.take(),
        _ => None,
    };
    let f = filter.compile()?;
    // A descendant scope walks the whole parent tree: it keeps the snapshot.
    let scoped = scoped(b, at) && scope.is_none();
    let query = SeedQuery {
        issue_type: f.issue_type.clone(),
        assignee: f.assignee.clone(),
        labels: f.labels.clone(),
        priority: f.priority,
        parent: f.parent.clone(),
        ..SeedQuery::default()
    };
    let ready_ids = if scoped {
        match b.ready_ids_where(&query)? {
            Some(ids) => ids,
            None => b.ready_ids(at)?,
        }
    } else {
        b.ready_ids(at)?
    };
    let mut snap = if scoped {
        let mut snap = b.snapshot_seeds(&ready_ids)?;
        // A scoped read finds a seed at its canonical IRI. One the ready query
        // returned but that read did not find would be silently left out of
        // the answer; refuse instead (a whole snapshot reads it by id).
        if let Some(id) = ready_ids.iter().find(|id| !snap.seeds.contains_key(*id)) {
            return Err(missed_at_canonical_iri(b, id)?);
        }
        if req.include_deferred {
            // Every deferred seed, and what it is blocked on, so the
            // unblocked ones are found below exactly as in a snapshot.
            let deferred = SeedQuery {
                statuses: Some(["deferred".to_string()].into()),
                ..query.clone()
            };
            merge_snapshots(&mut snap, b.snapshot_where(&deferred)?);
            let targets: Vec<String> = snap
                .seeds
                .values()
                .filter(|s| s.status == "deferred")
                .flat_map(|s| s.blocked_on.iter().cloned())
                .filter(|t| !snap.seeds.contains_key(t))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if !targets.is_empty() {
                merge_snapshots(&mut snap, b.snapshot_seeds(&targets)?);
            }
        }
        snap
    } else {
        b.snapshot(at)?
    };
    let under = match &scope {
        Some(p) => {
            snap.get(p)?;
            Some(descendants(&snap, p))
        }
        None => None,
    };
    let mut ids = ready_ids;
    if req.include_deferred {
        // Status `deferred` is not open, so the backend's ready query never
        // returns it; the unblocked ones are added here.
        ids.extend(
            snap.seeds
                .values()
                .filter(|s| s.status == "deferred" && snap.open_blockers(s).is_empty())
                .map(|s| s.id.clone()),
        );
    }
    let mut seeds: Vec<Seed> = ids
        .iter()
        .filter_map(|id| snap.seeds.get(id))
        .filter(|s| req.include_deferred || !is_deferred(s, &ctx.now))
        .filter(|s| under.as_ref().is_none_or(|u| u.contains(&s.id)))
        .filter(|s| f.matches(s))
        .cloned()
        .collect();
    match sort {
        "hybrid" => seeds.sort_by(|a, b| {
            (a.priority > 1, &a.created_at, &a.id).cmp(&(b.priority > 1, &b.created_at, &b.id))
        }),
        "oldest" => sort_seeds(&mut seeds, Some("created"))?,
        _ => sort_seeds(&mut seeds, Some("priority"))?,
    }
    let limit = req.limit.unwrap_or(0);
    let counts = if scoped {
        b.dependent_counts(&page_ids(&seeds, 0, limit))?
    } else {
        None
    };
    if scoped && counts.is_none() {
        with_dependents(b, &mut snap, &page_ids(&seeds, 0, limit))?;
    }
    let mut page = page(seeds, limit, &snap);
    if let Some(counts) = counts {
        page.dependent_counts = counts;
    }
    Ok(page)
}

/// The refusal for a seed a scoped read did not find at its canonical IRI,
/// naming where it is stored instead.
fn missed_at_canonical_iri(b: &dyn Backend, id: &str) -> Result<SdError> {
    let canonical = vocab::item_iri(id);
    let stored = b.subjects_of_id(id)?;
    let where_ = if stored.is_empty() {
        "no subject now carries that id (it changed while being read)".to_string()
    } else {
        format!(
            "it is stored as {}",
            stored
                .iter()
                .map(|s| format!("<{s}>"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(SdError::failed(format!(
        "the ready query returned {id}, but no seed is at its canonical IRI <{canonical}>: \
         {where_}. Refusing to answer without it; nothing was written."
    )))
}

/// Every seed below `root` in the parent chain, not `root` itself. A visited
/// set keeps a parent loop already in the ledger from repeating (br's
/// `--epic` is cycle-safe too).
fn descendants(snap: &Snapshot, root: &str) -> BTreeSet<String> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for s in snap.seeds.values() {
        if let Some(p) = &s.parent {
            children.entry(p.as_str()).or_default().push(s.id.as_str());
        }
    }
    let mut seen = BTreeSet::new();
    let mut todo = vec![root];
    while let Some(p) = todo.pop() {
        for &c in children.get(p).into_iter().flatten() {
            if c != root && seen.insert(c.to_string()) {
                todo.push(c);
            }
        }
    }
    seen
}

/// The ready definition computed directly over a snapshot, independent of the
/// backend's query. The backend's SPARQL is authoritative for `sd ready`; this
/// exists so the two can be checked against each other.
pub fn ready_by_model(snap: &Snapshot, now: &str) -> Vec<String> {
    snap.seeds
        .values()
        .filter(|s| s.status == "open" && snap.open_blockers(s).is_empty())
        .filter(|s| !is_deferred(s, now))
        // Ephemeral seeds are never ready (br); the SPARQL definition gets
        // this by reading only the project graph.
        .filter(|s| !s.ephemeral)
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
    if scoped(b, at) {
        if let Some(count) = b.aggregate_count(req)? {
            return Ok(count);
        }
    }
    let snap = if scoped(b, at) {
        let mut hidden = vec![model::TOMBSTONE];
        if !req.include_closed {
            hidden.push("closed");
        }
        matching_snapshot(b, &f, &listing_query(&f, &hidden))?
    } else {
        b.snapshot(at)?
    };
    let seeds: Vec<&Seed> = snap
        .seeds
        .values()
        .filter(|s| {
            f.status.is_some()
                || (!s.is_tombstone() && (req.include_closed || s.status != "closed"))
        })
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
    /// New design notes.
    pub design: Option<String>,
    /// New agent context (JSON text); empty clears it.
    pub agent_context: Option<String>,
    /// New acceptance criteria.
    pub acceptance_criteria: Option<String>,
    /// New external reference; empty clears it.
    pub external_ref: Option<String>,
    /// Acceptance checklist items to tick (br's --check-acceptance).
    pub check_acceptance: Vec<String>,
    /// Acceptance checklist items to untick (br's --uncheck-acceptance).
    pub uncheck_acceptance: Vec<String>,
    /// Unchecked items to append (br's --add-acceptance).
    pub add_acceptance: Vec<String>,
    /// New due date, br's forms; empty clears it.
    pub due: Option<String>,
    /// New time estimate in minutes.
    pub estimate: Option<String>,
    /// New owner; empty clears it.
    pub owner: Option<String>,
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
    /// A comment written with the change, in the same transaction.
    pub transition_comment: Option<String>,
    /// New type.
    pub issue_type: Option<String>,
    /// Replace all labels with these (comma-separated allowed).
    pub set_labels: Option<Vec<String>>,
    /// New parent; empty detaches the seed.
    pub parent: Option<String>,
    /// Allow replacing a non-empty description, notes, design, acceptance
    /// criteria or agent context with different content (br's --force). Without it that is
    /// refused, naming the field.
    pub force: bool,
}

/// `sd update`: all named seeds change in one transaction, or none do.
pub fn update(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    req: &UpdateReq,
) -> Result<(Vec<Seed>, u64)> {
    let status = req.status.as_deref().map(model::parse_status).transpose()?;
    let issue_type = req
        .issue_type
        .as_deref()
        .map(model::parse_type)
        .transpose()?;
    if status.as_deref() == Some(model::TOMBSTONE) {
        return Err(SdError::usage(
            "use sd delete to delete a seed; update cannot set the tombstone status",
        ));
    }
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
    let snap = if req.parent.is_some() {
        b.snapshot(None)?
    } else if req.claim {
        blocking_snapshot(b, ids, false)?
    } else {
        b.snapshot_items(ids)?
    };
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
            s.description = replace_text(id, "description", &before.description, d, req.force)?;
        }
        if let Some(n) = &req.notes {
            s.notes = replace_text(id, "notes", &before.notes, n, req.force)?;
        }
        if let Some(g) = &req.design {
            s.design = replace_text(id, "design", &before.design, g, req.force)?;
        }
        if let Some(c) = &req.agent_context {
            let c = match non_empty(Some(c)) {
                Some(c) => agent_context(&c)?,
                None => String::new(),
            };
            s.agent_context =
                replace_text(id, "agent context", &before.agent_context, &c, req.force)?;
        }
        let checklist_edit = !(req.check_acceptance.is_empty()
            && req.uncheck_acceptance.is_empty()
            && req.add_acceptance.is_empty());
        if checklist_edit && req.acceptance_criteria.is_some() {
            return Err(SdError::usage(
                "give --acceptance-criteria or checklist edits \
                 (--check/--uncheck/--add-acceptance), not both",
            ));
        }
        if let Some(a) = &req.acceptance_criteria {
            s.acceptance_criteria = replace_text(
                id,
                "acceptance criteria",
                &before.acceptance_criteria,
                a,
                req.force,
            )?;
        }
        if checklist_edit {
            let body = before.acceptance_criteria.clone().unwrap_or_default();
            let edited = edit_checklist(
                &body,
                &req.check_acceptance,
                &req.uncheck_acceptance,
                &req.add_acceptance,
            )
            .map_err(|e| SdError::usage(format!("{id}: {e}")))?;
            s.acceptance_criteria = non_empty(Some(&edited));
        }
        if let Some(e) = &req.external_ref {
            s.external_ref = non_empty(Some(e.trim()));
        }
        if let Some(d) = &req.due {
            s.due_at = match non_empty(Some(d.trim())) {
                Some(d) => Some(parse_when(&d, &ctx.now, "--due")?),
                None => None,
            };
        }
        if let Some(m) = &req.estimate {
            s.estimated_minutes = Some(parse_estimate(m)?);
        }
        if let Some(o) = &req.owner {
            s.owner = non_empty(Some(o.trim()));
        }
        if let Some(p) = priority {
            s.priority = p;
        }
        if let Some(a) = &req.assignee {
            s.assignee = non_empty(Some(a.trim()));
        }
        if let Some(d) = &req.defer {
            // Stored as given, in canonical spelling when it is a valid time;
            // an invalid one is refused by name when the write is validated.
            s.defer_until =
                non_empty(Some(d.trim())).map(|d| crate::model::canonical_or_same(&d, true));
        }
        if let Some(r) = &req.workflow_run {
            s.workflow_run = non_empty(Some(r.trim())).map(|r| vocab::run_iri(&r));
        }
        if let Some(t) = &issue_type {
            s.issue_type = t.clone();
        }
        if let Some(set) = &req.set_labels {
            s.labels = clean_labels(set);
        }
        if let Some(p) = &req.parent {
            s.parent = reparent(&snap, id, p)?;
            if s.parent != before.parent {
                if let Some(parent) = s.parent.clone() {
                    s.set_dependency_origin(&parent, "parent-child", &ctx.now, &ctx.actor)?;
                }
            }
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
    finish_with(
        b,
        ctx,
        &snap,
        ids,
        writes,
        "seeds:update",
        req.transition_comment.as_deref(),
    )
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
    let snap = b.snapshot_items(&ids)?;
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
                extra: Default::default(),
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
            model::TOMBSTONE => Some("deleted (not reopenable)".into()),
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
    defer_with(b, ctx, ids, until, None)
}

/// `sd defer --transition-comment`: [`defer`] with a comment in the same tx.
pub fn defer_with(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    until: Option<&str>,
    comment: Option<&str>,
) -> Result<Transitions> {
    let until = until.map(|u| parse_until(u, &ctx.now)).transpose()?;
    transition(
        b,
        ctx,
        ids,
        "seeds:defer",
        |s| match s.status.as_str() {
            "closed" => Some("cannot defer closed issue".to_string()),
            model::TOMBSTONE => Some("cannot defer a deleted seed".to_string()),
            _ => None,
        },
        |s| {
            s.status = "deferred".into();
            s.defer_until = until.clone();
        },
        comment,
    )
}

/// `sd undefer`: deferred seeds become open and lose their defer date.
pub fn undefer(b: &mut dyn Backend, ctx: &Ctx, ids: &[String]) -> Result<Transitions> {
    undefer_with(b, ctx, ids, None)
}

/// `sd undefer --transition-comment`: [`undefer`] with a comment in the same tx.
pub fn undefer_with(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    comment: Option<&str>,
) -> Result<Transitions> {
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
        comment,
    )
}

/// Resolve a `--until` value against `now` (an RFC 3339 UTC instant). Dates and
/// instants are kept as given; relative forms become a UTC instant or date.
pub fn parse_until(value: &str, now: &str) -> Result<String> {
    parse_when(value, now, "--until")
}

/// A date or instant in br's forms (`+30m`, `+2h`, `+1d`, `+1w`, `tomorrow`,
/// `YYYY-MM-DD`, RFC 3339), relative to `now`. `flag` names the flag in an error.
pub fn parse_when(value: &str, now: &str, flag: &str) -> Result<String> {
    let v = value.trim();
    let bad = || {
        SdError::usage(format!(
            "cannot read {flag} {value:?}; use +30m, +2h, +1d, +1w, tomorrow, \
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
        return Ok(crate::model::canonical_or_same(v, true));
    }
    if v.len() >= 20 && parse_instant(v).is_some() {
        return Ok(crate::model::canonical_or_same(v, true));
    }
    Err(bad())
}

/// One `- [ ]` / `- [x]` line of an acceptance checklist.
struct ChecklistItem {
    /// Index into the body's lines.
    line: usize,
    /// Byte offset of the box's inner character in that line.
    mark: usize,
    /// The item's text after the box.
    text: String,
}

fn checklist_items(lines: &[&str]) -> Vec<ChecklistItem> {
    let mut items = Vec::new();
    for (line, l) in lines.iter().enumerate() {
        let lead = l.len() - l.trim_start().len();
        let rest = &l[lead..];
        let Some(after) = rest
            .strip_prefix("- [")
            .or_else(|| rest.strip_prefix("* ["))
        else {
            continue;
        };
        let mut chars = after.chars();
        let (Some(c), Some(']')) = (chars.next(), chars.next()) else {
            continue;
        };
        if !matches!(c, ' ' | 'x' | 'X') {
            continue;
        }
        let text = chars.as_str().trim().to_string();
        items.push(ChecklistItem {
            line,
            mark: lead + 3,
            text,
        });
    }
    items
}

/// Resolve one ITEMS argument (br's form): a comma-separated list of 1-based
/// item numbers, or a text selector that must match exactly one item
/// (case-insensitive; an exact match wins, otherwise a unique substring).
fn select_items(spec: &str, items: &[ChecklistItem]) -> std::result::Result<Vec<usize>, String> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    if !parts.is_empty()
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return parts
            .iter()
            .map(|p| {
                let n: usize = p.parse().map_err(|_| format!("bad item number {p:?}"))?;
                if n == 0 || n > items.len() {
                    Err(format!(
                        "no item {n}: the checklist has {} item(s)",
                        items.len()
                    ))
                } else {
                    Ok(n - 1)
                }
            })
            .collect();
    }
    let want = spec.trim().to_lowercase();
    if want.is_empty() {
        return Err("an empty item selector".into());
    }
    let exact: Vec<usize> = (0..items.len())
        .filter(|&i| items[i].text.to_lowercase() == want)
        .collect();
    let hits = if exact.is_empty() {
        (0..items.len())
            .filter(|&i| items[i].text.to_lowercase().contains(&want))
            .collect()
    } else {
        exact
    };
    match hits.as_slice() {
        [one] => Ok(vec![*one]),
        [] => Err(format!("{spec:?} matches no checklist item")),
        many => Err(format!(
            "{spec:?} matches {} items ({}); use its number or more of its text",
            many.len(),
            many.iter()
                .map(|i| (i + 1).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// br's in-place acceptance checklist edit: tick `check`, untick `uncheck`,
/// append `add` as unchecked items. Every selector is resolved before anything
/// changes; every other byte of the field is kept. An item already in the
/// requested state is left as it is.
pub fn edit_checklist(
    body: &str,
    check: &[String],
    uncheck: &[String],
    add: &[String],
) -> std::result::Result<String, String> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let items = checklist_items(&lines);
    if items.is_empty() && !(check.is_empty() && uncheck.is_empty()) {
        return Err("has no acceptance checklist (`- [ ] ...` lines) to tick".into());
    }
    let mut want: BTreeMap<usize, char> = BTreeMap::new();
    for (specs, mark) in [(check, 'x'), (uncheck, ' ')] {
        for spec in specs {
            for i in select_items(spec, &items)? {
                if want.insert(i, mark).is_some_and(|m| m != mark) {
                    return Err(format!(
                        "item {} is both checked and unchecked by this update",
                        i + 1
                    ));
                }
            }
        }
    }
    for a in add {
        if a.trim().is_empty() {
            return Err("--add-acceptance needs text".into());
        }
    }
    let mut out: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
    for (i, mark) in want {
        let it = &items[i];
        let l = &mut out[it.line];
        let current = l[it.mark..].chars().next();
        let done = matches!(current, Some('x' | 'X'));
        if (mark == 'x') != done {
            l.replace_range(it.mark..it.mark + 1, &mark.to_string());
        }
    }
    let mut body: String = out.concat();
    for a in add {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&format!("- [ ] {}\n", a.trim()));
    }
    Ok(body)
}

/// br's bound on `estimated_minutes`: 0 to about a year.
pub const MAX_ESTIMATE_MINUTES: u32 = 525_960;

/// An `--estimate` in minutes, refused outside br's bounds.
pub fn parse_estimate(value: &str) -> Result<u32> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|m| *m <= MAX_ESTIMATE_MINUTES)
        .ok_or_else(|| {
            SdError::usage(format!(
                "--estimate {value:?}: expected whole minutes from 0 to \
                 {MAX_ESTIMATE_MINUTES} (about a year)"
            ))
        })
}

/// A stored date (`YYYY-MM-DD`, read as its first instant, UTC) or RFC 3339
/// instant, as epoch seconds.
fn epoch_of(v: &str) -> Option<i64> {
    if v.len() == 10 {
        parse_date(v).map(|d| d * 86_400)
    } else {
        parse_instant(v)
    }
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

// ---------------------------------------------------------------- delete

/// What `sd delete` did, or would do (br's shape).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteReport {
    /// Nothing was written: a preview (--dry-run, or dependents without
    /// --cascade/--force).
    pub preview: bool,
    /// The seeds deleted, or that would be.
    pub deleted: Vec<String>,
    /// Dependents that --cascade deletes too.
    pub cascade: Vec<String>,
    /// Dependents that stop a plain delete.
    pub blocked_dependents: Vec<String>,
    /// Dependents left pointing at a tombstone (--force). A tombstone never
    /// blocks, so they are not stuck.
    pub orphaned: Vec<String>,
    /// The transaction (0 for a preview).
    pub tx: u64,
}

/// `sd delete`: tombstone seeds (br's model). A seed with live dependents is
/// only previewed unless `cascade` (tombstone them too) or `force` (orphan
/// them). One transaction; each tombstone gets a `Deleted: <reason>` comment.
/// Not a close: no outcome, no closed_at.
pub fn delete(
    b: &mut dyn Backend,
    ctx: &Ctx,
    ids: &[String],
    reason: &str,
    cascade: bool,
    force: bool,
    dry_run: bool,
) -> Result<DeleteReport> {
    let ids = unique(ids);
    if ids.is_empty() {
        return Err(SdError::usage("at least one seed id is required"));
    }
    let snap = b.snapshot(None)?;
    for id in &ids {
        snap.get(id)?;
    }
    let live_dependents = |id: &str| -> Vec<String> {
        snap.dependents(id)
            .into_iter()
            .map(|(d, _)| d)
            .filter(|d| snap.seeds.get(d).is_some_and(|s| !s.is_tombstone()))
            .collect()
    };
    // The cascade closure: every live seed that (transitively) depends on a target.
    let mut cascade_ids: Vec<String> = Vec::new();
    let mut queue: Vec<String> = ids.clone();
    while let Some(id) = queue.pop() {
        for d in live_dependents(&id) {
            if !ids.contains(&d) && !cascade_ids.contains(&d) {
                cascade_ids.push(d.clone());
                queue.push(d);
            }
        }
    }
    let direct: Vec<String> = {
        let mut v: Vec<String> = ids
            .iter()
            .flat_map(|id| live_dependents(id))
            .filter(|d| !ids.contains(d))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let mut report = DeleteReport {
        deleted: ids.clone(),
        cascade: cascade_ids.clone(),
        blocked_dependents: direct.clone(),
        ..DeleteReport::default()
    };
    if dry_run || (!direct.is_empty() && !cascade && !force) {
        report.preview = true;
        return Ok(report);
    }
    let targets: Vec<String> = if cascade {
        ids.iter().chain(cascade_ids.iter()).cloned().collect()
    } else {
        ids.clone()
    };
    report.blocked_dependents.clear();
    report.cascade = if cascade { cascade_ids } else { Vec::new() };
    report.orphaned = if cascade { Vec::new() } else { direct };
    report.deleted = targets.clone();
    let (mut writes, mut comments) = (Vec::new(), Vec::new());
    let reason = reason.trim();
    for id in &targets {
        let before = snap.get(id)?;
        if before.is_tombstone() {
            continue;
        }
        let mut s = before.clone();
        s.status = model::TOMBSTONE.into();
        // A tombstone is not a close, even for a seed that was closed first:
        // drop the close fields (history keeps them, and --at reads them).
        s.closed_at = None;
        s.close_reason = None;
        s.outcome = None;
        s.updated_at = ctx.now.clone();
        s.revision = before.revision + 1;
        writes.push(SeedWrite {
            seed: s,
            expected_revision: Some(before.revision),
        });
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
            text: format!(
                "Deleted: {}",
                if reason.is_empty() { "delete" } else { reason }
            ),
            created_at: ctx.now.clone(),
            extra: Default::default(),
        });
    }
    report.tx = if writes.is_empty() {
        snap.tx
    } else {
        b.commit(
            &WriteBatch {
                seeds: writes,
                comments,
                source: "seeds:delete".into(),
                ..WriteBatch::default()
            },
            ctx,
        )?
    };
    Ok(report)
}

/// The new parent for `update --parent`: empty detaches; otherwise it must
/// exist, not be the seed itself, and not make the seed its own ancestor.
fn reparent(snap: &Snapshot, id: &str, parent: &str) -> Result<Option<String>> {
    let parent = parent.trim();
    if parent.is_empty() {
        return Ok(None);
    }
    snap.get(parent)?;
    // A visited set, so a parent loop ALREADY in the ledger (a raw write or a
    // bad merge; the verbs cannot make one) is reported instead of spinning
    // forever (wu, seeds#45 review).
    let mut seen = BTreeSet::new();
    let mut cur = Some(parent.to_string());
    while let Some(p) = cur {
        if p == id {
            return Err(SdError::refused(format!(
                "{parent} is {id} or one of its descendants; a seed cannot be its own ancestor"
            )));
        }
        if !seen.insert(p.clone()) {
            return Err(SdError::refused(format!(
                "the ancestors of {parent} already loop through {p}; fix that parent chain \
                 (sd doctor lists it) before reparenting"
            )));
        }
        cur = snap.seeds.get(&p).and_then(|s| s.parent.clone());
    }
    Ok(Some(parent.to_string()))
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
    close_as(b, ctx, ids, reason, None, force, None)
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
    transition_comment: Option<&str>,
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
    // The seeds, their comments (a transition comment's index) and their
    // direct blockers: everything the close checks.
    let snap = blocking_snapshot(b, ids, false)?;
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
    let (seeds, tx) = finish_with(
        b,
        ctx,
        &snap,
        ids,
        writes,
        "seeds:close",
        transition_comment,
    )?;
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
    finish_with(b, ctx, snap, ids, writes, source, None)
}

/// One comment per seed a write actually changes, all in the write's own
/// transaction (br's `--transition-comment`). Seeds left unchanged get none.
fn transition_comments(
    snap: &Snapshot,
    ctx: &Ctx,
    writes: &[SeedWrite],
    text: Option<&str>,
) -> Vec<Comment> {
    let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return Vec::new();
    };
    writes
        .iter()
        .map(|w| Comment {
            seed: w.seed.id.clone(),
            index: snap
                .comments_on(&w.seed.id)
                .iter()
                .map(|c| c.index)
                .max()
                .unwrap_or(0)
                + 1,
            author: ctx.actor.clone(),
            text: text.to_string(),
            created_at: ctx.now.clone(),
            extra: Default::default(),
        })
        .collect()
}

fn finish_with(
    b: &mut dyn Backend,
    ctx: &Ctx,
    snap: &Snapshot,
    ids: &[String],
    writes: Vec<SeedWrite>,
    source: &str,
    transition_comment: Option<&str>,
) -> Result<(Vec<Seed>, u64)> {
    let comments = transition_comments(snap, ctx, &writes, transition_comment);
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
                comments,
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
    let snap = if dep_type == "parent-child" {
        b.snapshot(None)?
    } else if dep_type == "blocks" {
        blocking_snapshot(b, &[issue.into(), depends_on.into()], true)?
    } else {
        b.snapshot_items(&[issue.into(), depends_on.into()])?
    };
    let before = snap.get(issue)?;
    let target = snap.get(depends_on)?;
    for s in [before, target] {
        if s.is_tombstone() {
            return Err(SdError::refused(format!(
                "{} is deleted; a deleted seed cannot gain a dependency",
                s.id
            )));
        }
    }
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
    s.set_dependency_origin(depends_on, &dep_type, &ctx.now, &ctx.actor)?;
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

/// Load the blocking context needed by a claim (direct targets) or cycle check
/// (the reachable closure). Missing targets are remembered so a dangling edge
/// cannot loop the lookup. Never combine observations from different revisions.
fn blocking_snapshot(b: &dyn Backend, ids: &[String], transitive: bool) -> Result<Snapshot> {
    let mut snap = b.snapshot_items(ids)?;
    let mut seen: BTreeSet<String> = ids.iter().cloned().collect();
    loop {
        let next: Vec<String> = snap
            .seeds
            .values()
            .flat_map(|s| s.blocked_on.iter())
            .filter(|id| !seen.contains(*id) && !snap.seeds.contains_key(*id))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if next.is_empty() {
            break;
        }
        seen.extend(next.iter().cloned());
        let extra = b.snapshot_items(&next)?;
        if extra.tx != snap.tx {
            return Err(SdError::conflict(
                "blocking context changed while reading; nothing was written",
            ));
        }
        snap.seeds.extend(extra.seeds);
        snap.comments.extend(extra.comments);
        if !transitive {
            break;
        }
    }
    Ok(snap)
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

/// Seeds that `id` was blocking and that now have no open blocker at all, as
/// of the current state (br's `close --suggest-next`). A seed still blocked by
/// something else is not listed.
pub fn unblocked_by(b: &dyn Backend, id: &str) -> Result<Vec<Seed>> {
    let snap = b.snapshot(None)?;
    let mut out: Vec<Seed> = snap
        .seeds
        .values()
        .filter(|s| s.blocked_on.contains(id))
        .filter(|s| s.status != "closed" && !s.is_tombstone())
        .filter(|s| snap.open_blockers(s).is_empty())
        .cloned()
        .collect();
    sort_seeds(&mut out, Some("priority"))?;
    Ok(out)
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
    let snap = b.snapshot_items(&[id.to_string()])?;
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
        extra: Default::default(),
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
/// The new value of a free-text field, refusing to replace existing text
/// with different text unless `force` (br's overwrite guard, its GitHub
/// #467). Empty -> value and the same value again both pass, so an
/// idempotent re-run never trips it; clearing existing text is a
/// replacement too. Every backend's update passes through here.
fn replace_text(
    id: &str,
    field: &str,
    before: &Option<String>,
    new: &str,
    force: bool,
) -> Result<Option<String>> {
    let new = non_empty(Some(new));
    if !force && before.is_some() && *before != new {
        return Err(SdError::refused(format!(
            "{id} already has a {field}, and this update would replace it with different \
             content; nothing was written. Re-run with --force to replace it (the old text \
             stays in sd history)"
        )));
    }
    Ok(new)
}

/// An agent context must be JSON (any value, as br accepts). The native CLI
/// has already normalized it to compact JSON; this guards other callers.
fn agent_context(text: &str) -> Result<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text)
        .map_err(|e| SdError::usage(format!("agent context is not valid JSON: {e}")))?;
    Ok(text.to_string())
}

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

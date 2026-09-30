//! Moving a ledger between stores: import, replace, and three-way sync.
//!
//! All of it is planning over [`Snapshot`]s plus one [`Backend::commit`] per
//! store, so it is pure and runs on wasm32 too; the native CLI supplies the
//! files and the network.
//!
//! **Conflicts are never resolved silently.** Import refuses a seed that
//! differs on both sides unless the caller names a side (`--prefer`), and sync
//! merges field by field against the common base, reporting every field both
//! sides changed differently. A report with conflicts writes nothing.

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::{Backend, Ctx, SeedWrite, WriteBatch};
use crate::error::{Result, SdError};
use crate::model::{Comment, Seed, Snapshot};

/// Which side wins where two ledgers disagree, when the caller says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefer {
    /// The ledger being imported.
    Incoming,
    /// The store being imported into.
    Existing,
}

/// One seed (or comment) the two sides disagree about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The seed id.
    pub id: String,
    /// What differs, e.g. `status: "closed" (local) vs "in_progress" (remote)`.
    pub fields: Vec<String>,
}

impl Conflict {
    fn line(&self) -> String {
        format!("{}: {}", self.id, self.fields.join("; "))
    }
}

/// What an import or sync did (or would do).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Seeds created in the target.
    pub created: Vec<String>,
    /// Seeds changed in the target.
    pub updated: Vec<String>,
    /// Seeds removed from the target (replace and sync only).
    pub removed: Vec<String>,
    /// Seeds already identical.
    pub unchanged: usize,
    /// Comments added.
    pub comments_added: usize,
    /// Disagreements. Non-empty means nothing was written.
    pub conflicts: Vec<Conflict>,
    /// The target's transaction, when something was written and the store
    /// reports one (a local store does; a quipu server's `/update` does not).
    pub tx: u64,
    /// Whether a write was committed to the target. Unlike `tx`, this is
    /// true for every store, so "nothing changed" is checkable everywhere.
    pub wrote: bool,
}

/// The error a report with conflicts turns into.
pub fn conflict_error(what: &str, conflicts: &[Conflict], hint: &str) -> SdError {
    SdError::conflict(format!(
        "{what}: {} conflicting seed(s); nothing was written. {hint}\n  {}",
        conflicts.len(),
        conflicts
            .iter()
            .map(Conflict::line)
            .collect::<Vec<_>>()
            .join("\n  ")
    ))
}

/// Everything about a seed except its revision and update time: two seeds
/// with equal content say the same thing about the work.
fn content(s: &Seed) -> Seed {
    Seed {
        revision: 0,
        updated_at: String::new(),
        ..s.clone()
    }
}

/// The writes that turn `target` into `merged`. Seeds keep `merged`'s
/// revision; each write carries the revision read from `target`.
pub fn plan(target: &Snapshot, merged: &Snapshot, source: &str) -> (WriteBatch, Report) {
    let mut batch = WriteBatch {
        source: source.into(),
        ..WriteBatch::default()
    };
    let mut report = Report::default();
    for (id, m) in &merged.seeds {
        match target.seeds.get(id) {
            None => {
                batch.seeds.push(SeedWrite {
                    seed: m.clone(),
                    expected_revision: None,
                });
                report.created.push(id.clone());
            }
            Some(t) if t == m => report.unchanged += 1,
            Some(t) => {
                batch.seeds.push(SeedWrite {
                    seed: m.clone(),
                    expected_revision: Some(t.revision),
                });
                report.updated.push(id.clone());
            }
        }
    }
    for (id, t) in &target.seeds {
        if !merged.seeds.contains_key(id) {
            batch.delete_seeds.push((id.clone(), t.revision));
            report.removed.push(id.clone());
        }
    }
    let key = |c: &Comment| (c.seed.clone(), c.index);
    let tc: BTreeMap<_, _> = target.comments.iter().map(|c| (key(c), c)).collect();
    let mc: BTreeMap<_, _> = merged.comments.iter().map(|c| (key(c), c)).collect();
    for (k, c) in &mc {
        match tc.get(k) {
            Some(t) if t == c => {}
            Some(_) => {
                batch.delete_comments.push(k.clone());
                batch.comments.push((*c).clone());
                report.comments_added += 1;
            }
            None => {
                batch.comments.push((*c).clone());
                report.comments_added += 1;
            }
        }
    }
    for k in tc.keys() {
        if !mc.contains_key(k) && merged.seeds.contains_key(&k.0) {
            batch.delete_comments.push(k.clone());
        }
    }
    (batch, report)
}

/// Import `incoming` into the store behind `b`.
///
/// - A seed only in `incoming` is created; an identical one is left alone.
/// - A seed on both sides that differs is a conflict unless `prefer` names a
///   side. With [`Prefer::Incoming`] it takes the incoming content at a
///   revision above both sides (so no reader holding the old revision can
///   write over it); with [`Prefer::Existing`] it keeps the store's.
/// - With `replace`, the store becomes exactly `incoming`, revisions included:
///   seeds it lacks are removed. This is how a repo-local store follows its
///   pendant after a `git pull`.
pub fn import(
    b: &mut dyn Backend,
    ctx: &Ctx,
    incoming: &Snapshot,
    prefer: Option<Prefer>,
    replace: bool,
) -> Result<Report> {
    let target = b.snapshot(None)?;
    let (merged, conflicts) = if replace {
        (incoming.clone(), Vec::new())
    } else {
        merge_two(&target, incoming, prefer)
    };
    if !conflicts.is_empty() {
        return Err(conflict_error(
            "import",
            &conflicts,
            "Pass --prefer pendant or --prefer store to choose a side for every conflict.",
        ));
    }
    let (batch, mut report) = plan(&target, &merged, "seeds:import");
    if !batch.is_empty() {
        report.tx = b.commit(&batch, ctx)?;
        report.wrote = true;
    }
    Ok(report)
}

fn merge_two(
    target: &Snapshot,
    incoming: &Snapshot,
    prefer: Option<Prefer>,
) -> (Snapshot, Vec<Conflict>) {
    let mut merged = target.clone();
    let mut conflicts = Vec::new();
    for (id, inc) in &incoming.seeds {
        match target.seeds.get(id) {
            None => {
                merged.seeds.insert(id.clone(), inc.clone());
            }
            Some(t) if content(t) == content(inc) => {}
            Some(t) => match prefer {
                Some(Prefer::Incoming) => {
                    let mut s = inc.clone();
                    s.revision = t.revision.max(inc.revision) + 1;
                    merged.seeds.insert(id.clone(), s);
                }
                Some(Prefer::Existing) => {}
                None => conflicts.push(Conflict {
                    id: id.clone(),
                    fields: diff_fields(t, inc, "store", "pendant"),
                }),
            },
        }
    }
    let key = |c: &Comment| (c.seed.clone(), c.index);
    let have: BTreeMap<_, _> = target
        .comments
        .iter()
        .map(|c| (key(c), c.clone()))
        .collect();
    for c in &incoming.comments {
        match have.get(&key(c)) {
            None => merged.comments.push(c.clone()),
            Some(t) if t == c => {}
            Some(_) => match prefer {
                Some(Prefer::Incoming) => {
                    merged.comments.retain(|x| key(x) != key(c));
                    merged.comments.push(c.clone());
                }
                Some(Prefer::Existing) => {}
                None => conflicts.push(Conflict {
                    id: c.seed.clone(),
                    fields: vec![format!("comment {} differs", c.index)],
                }),
            },
        }
    }
    merged
        .comments
        .sort_by(|a, b| (&a.seed, a.index).cmp(&(&b.seed, b.index)));
    (merged, conflicts)
}

/// The fields in which two versions of a seed differ, for a conflict report.
pub fn diff_fields(a: &Seed, b: &Seed, a_name: &str, b_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut f = |name: &str, x: String, y: String| {
        if x != y {
            out.push(format!("{name}: {x} ({a_name}) vs {y} ({b_name})"));
        }
    };
    f("title", format!("{:?}", a.title), format!("{:?}", b.title));
    f(
        "status",
        format!("{:?}", a.status),
        format!("{:?}", b.status),
    );
    f("priority", a.priority.to_string(), b.priority.to_string());
    f(
        "type",
        format!("{:?}", a.issue_type),
        format!("{:?}", b.issue_type),
    );
    f(
        "assignee",
        format!("{:?}", a.assignee),
        format!("{:?}", b.assignee),
    );
    let summary = |d: &Option<String>| match d {
        None => "none".to_string(),
        Some(d) => format!("{} chars", d.chars().count()),
    };
    if a.description != b.description {
        f(
            "description",
            summary(&a.description),
            format!("{} (text differs)", summary(&b.description)),
        );
    }
    f("notes", format!("{:?}", a.notes), format!("{:?}", b.notes));
    f("owner", format!("{:?}", a.owner), format!("{:?}", b.owner));
    f(
        "labels",
        format!("{:?}", a.labels),
        format!("{:?}", b.labels),
    );
    f(
        "blocked_on",
        format!("{:?}", a.blocked_on),
        format!("{:?}", b.blocked_on),
    );
    f(
        "close_reason",
        format!("{:?}", a.close_reason),
        format!("{:?}", b.close_reason),
    );
    f(
        "outcome",
        format!("{:?}", a.outcome),
        format!("{:?}", b.outcome),
    );
    f(
        "defer_until",
        format!("{:?}", a.defer_until),
        format!("{:?}", b.defer_until),
    );
    f(
        "parent",
        format!("{:?}", a.parent),
        format!("{:?}", b.parent),
    );
    out
}

// ---------------------------------------------------------------- three-way

/// The result of a three-way merge.
#[derive(Debug, Clone, Default)]
pub struct Merge {
    /// The merged ledger.
    pub merged: Snapshot,
    /// Fields both sides changed differently since the base.
    pub conflicts: Vec<Conflict>,
}

/// Names for the two sides in conflict reports.
#[derive(Debug, Clone, Copy)]
pub struct Sides<'a> {
    /// The first side, e.g. `local` or `ours`.
    pub a: &'a str,
    /// The second side, e.g. `remote` or `theirs`.
    pub b: &'a str,
}

fn pick<T: Clone + PartialEq>(
    field: &str,
    base: Option<&T>,
    local: &T,
    remote: &T,
    conflicts: &mut Vec<String>,
    sides: Sides,
    show: impl Fn(&T) -> String,
) -> T {
    if local == remote {
        return local.clone();
    }
    match base {
        Some(b) if b == local => remote.clone(),
        Some(b) if b == remote => local.clone(),
        _ => {
            conflicts.push(format!(
                "{field}: {} ({}) vs {} ({})",
                show(local),
                sides.a,
                show(remote),
                sides.b
            ));
            local.clone()
        }
    }
}

fn dbg<T: std::fmt::Debug>(v: &T) -> String {
    format!("{v:?}")
}

fn pick_set(
    base: Option<&BTreeSet<String>>,
    local: &BTreeSet<String>,
    remote: &BTreeSet<String>,
) -> BTreeSet<String> {
    let empty = BTreeSet::new();
    let base = base.unwrap_or(&empty);
    // Keep what neither side removed; add what either side added.
    let kept: BTreeSet<String> = base
        .iter()
        .filter(|x| local.contains(*x) && remote.contains(*x))
        .cloned()
        .collect();
    let added_l = local.difference(base).cloned();
    let added_r = remote.difference(base).cloned();
    kept.into_iter().chain(added_l).chain(added_r).collect()
}

fn merge_seed(base: Option<&Seed>, l: &Seed, r: &Seed, sides: Sides) -> (Seed, Vec<String>) {
    let mut c = Vec::new();
    let status_group = |s: &Seed| {
        (
            s.status.clone(),
            s.closed_at.clone(),
            s.close_reason.clone(),
            s.outcome.clone(),
        )
    };
    let (status, closed_at, close_reason, outcome) = pick(
        "status",
        base.map(status_group).as_ref(),
        &status_group(l),
        &status_group(r),
        &mut c,
        sides,
        |(st, _, reason, _)| match reason {
            Some(r) => format!("{st:?} (reason {r:?})"),
            None => format!("{st:?}"),
        },
    );
    let m = Seed {
        id: l.id.clone(),
        title: pick(
            "title",
            base.map(|b| &b.title),
            &l.title,
            &r.title,
            &mut c,
            sides,
            dbg,
        ),
        description: pick(
            "description",
            base.map(|b| &b.description),
            &l.description,
            &r.description,
            &mut c,
            sides,
            dbg,
        ),
        notes: pick(
            "notes",
            base.map(|b| &b.notes),
            &l.notes,
            &r.notes,
            &mut c,
            sides,
            dbg,
        ),
        owner: pick(
            "owner",
            base.map(|b| &b.owner),
            &l.owner,
            &r.owner,
            &mut c,
            sides,
            dbg,
        ),
        status,
        priority: pick(
            "priority",
            base.map(|b| &b.priority),
            &l.priority,
            &r.priority,
            &mut c,
            sides,
            dbg,
        ),
        issue_type: pick(
            "type",
            base.map(|b| &b.issue_type),
            &l.issue_type,
            &r.issue_type,
            &mut c,
            sides,
            dbg,
        ),
        assignee: pick(
            "assignee",
            base.map(|b| &b.assignee),
            &l.assignee,
            &r.assignee,
            &mut c,
            sides,
            dbg,
        ),
        labels: pick_set(base.map(|b| &b.labels), &l.labels, &r.labels),
        created_at: pick(
            "created_at",
            base.map(|b| &b.created_at),
            &l.created_at,
            &r.created_at,
            &mut c,
            sides,
            dbg,
        ),
        created_by: pick(
            "created_by",
            base.map(|b| &b.created_by),
            &l.created_by,
            &r.created_by,
            &mut c,
            sides,
            dbg,
        ),
        updated_at: l.updated_at.clone().max(r.updated_at.clone()),
        closed_at,
        close_reason,
        outcome,
        defer_until: pick(
            "defer_until",
            base.map(|b| &b.defer_until),
            &l.defer_until,
            &r.defer_until,
            &mut c,
            sides,
            dbg,
        ),
        blocked_on: pick_set(base.map(|b| &b.blocked_on), &l.blocked_on, &r.blocked_on),
        related: pick_set(base.map(|b| &b.related), &l.related, &r.related),
        parent: pick(
            "parent",
            base.map(|b| &b.parent),
            &l.parent,
            &r.parent,
            &mut c,
            sides,
            dbg,
        ),
        discovered_from: pick_set(
            base.map(|b| &b.discovered_from),
            &l.discovered_from,
            &r.discovered_from,
        ),
        workflow_run: pick(
            "workflow_run",
            base.map(|b| &b.workflow_run),
            &l.workflow_run,
            &r.workflow_run,
            &mut c,
            sides,
            dbg,
        ),
        revision: 0,
    };
    let revision = if content(&m) == content(r) {
        r.revision
    } else if content(&m) == content(l) {
        l.revision
    } else {
        l.revision.max(r.revision) + 1
    };
    let updated_at = if content(&m) == content(r) {
        r.updated_at.clone()
    } else if content(&m) == content(l) {
        l.updated_at.clone()
    } else {
        m.updated_at.clone()
    };
    (
        Seed {
            revision,
            updated_at,
            ..m
        },
        c,
    )
}

/// Merge `local` and `remote` against their common `base` (the ledger as of
/// the last sync; empty before the first).
///
/// Per seed: a side that did not change since the base yields to the side
/// that did; a seed both sides changed is merged field by field, and only a
/// field both sides changed to different values is a conflict. Set-valued
/// fields (labels, dependencies) take every addition and every removal from
/// both sides. A seed one side deleted and the other changed is a conflict.
/// Comments are append-only: both sides' new comments are kept, and where
/// both used the same number, the local one is renumbered after the remote's.
pub fn merge3(base: &Snapshot, local: &Snapshot, remote: &Snapshot) -> Merge {
    merge3_named(
        base,
        local,
        remote,
        Sides {
            a: "local",
            b: "remote",
        },
    )
}

/// [`merge3`] with the two sides named for the conflict report (the git merge
/// driver calls them `ours` and `theirs`).
pub fn merge3_named(base: &Snapshot, local: &Snapshot, remote: &Snapshot, sides: Sides) -> Merge {
    let mut merged = Snapshot::default();
    let mut conflicts = Vec::new();
    let ids: BTreeSet<&String> = local.seeds.keys().chain(remote.seeds.keys()).collect();
    for id in ids {
        let b = base.seeds.get(id);
        match (local.seeds.get(id), remote.seeds.get(id)) {
            (Some(l), Some(r)) => {
                if l == r {
                    merged.seeds.insert(id.clone(), l.clone());
                    continue;
                }
                let (m, fields) = merge_seed(b, l, r, sides);
                if fields.is_empty() {
                    merged.seeds.insert(id.clone(), m);
                } else {
                    conflicts.push(Conflict {
                        id: id.clone(),
                        fields,
                    });
                }
            }
            (Some(only), None) | (None, Some(only)) => match b {
                // Created on one side since the base: keep it.
                None => {
                    merged.seeds.insert(id.clone(), only.clone());
                }
                // Deleted on the other side: honour the delete only if this
                // side left it untouched.
                Some(b) if content(b) == content(only) => {}
                Some(_) => conflicts.push(Conflict {
                    id: id.clone(),
                    fields: vec!["deleted on one side, changed on the other".into()],
                }),
            },
            (None, None) => {}
        }
    }

    // Comments.
    let key = |c: &Comment| (c.seed.clone(), c.index);
    let base_keys: BTreeSet<_> = base.comments.iter().map(key).collect();
    let mut by_key: BTreeMap<(String, u64), Comment> = BTreeMap::new();
    for c in &remote.comments {
        by_key.insert(key(c), c.clone());
    }
    let mut next: BTreeMap<String, u64> = BTreeMap::new();
    for c in remote.comments.iter().chain(&local.comments) {
        let n = next.entry(c.seed.clone()).or_insert(0);
        *n = (*n).max(c.index);
    }
    // The same comment can sit under a different number on the other side: a
    // sync whose remote write landed but whose response was lost already
    // renumbered and delivered it. Match on what the comment IS (seed,
    // author, text, time), never on the number alone, or a retry duplicates it.
    let same = |a: &Comment, b: &Comment| {
        a.seed == b.seed && a.author == b.author && a.text == b.text && a.created_at == b.created_at
    };
    for c in &local.comments {
        if by_key.values().any(|r| same(r, c)) {
            continue;
        }
        match by_key.get(&key(c)) {
            None => {
                by_key.insert(key(c), c.clone());
            }
            Some(r) if r == c => {}
            Some(_) if base_keys.contains(&key(c)) => conflicts.push(Conflict {
                id: c.seed.clone(),
                fields: vec![format!("comment {} was edited", c.index)],
            }),
            Some(_) => {
                let n = next.entry(c.seed.clone()).or_insert(0);
                *n += 1;
                let mut moved = c.clone();
                moved.index = *n;
                by_key.insert(key(&moved), moved);
            }
        }
    }
    merged.comments = by_key
        .into_values()
        .filter(|c| merged.seeds.contains_key(&c.seed))
        .collect();
    Merge { merged, conflicts }
}

/// Sync two stores through a three-way merge: plan both sides from one merge,
/// then write the remote first and the local second, each with a
/// compare-and-set on the revisions it read. Nothing is written when there
/// are conflicts; if the remote write loses a race, the local store is left
/// untouched and the sync can simply be run again.
///
/// **Removals are refused unless `allow_deletes`.** No verb deletes a seed,
/// so a seed that is in the base and on one side but missing on the other
/// almost always means the other side is a different, reset or restored
/// store, not that someone deleted it. Sync names the count and writes
/// nothing; pass `allow_deletes` (`--allow-remote-deletes`) to apply them.
pub fn sync(
    base: &Snapshot,
    local: &mut dyn Backend,
    remote: &mut dyn Backend,
    ctx: &Ctx,
    allow_deletes: bool,
) -> Result<(Snapshot, Report, Report)> {
    let l = local.snapshot(None)?;
    let r = remote.snapshot(None)?;
    let m = merge3(base, &l, &r);
    if !m.conflicts.is_empty() {
        return Err(conflict_error(
            "sync",
            &m.conflicts,
            "Change one side so the fields agree (sd update), then sync again.",
        ));
    }
    let (rb, mut rr) = plan(&r, &m.merged, "seeds:sync");
    let (lb, mut lr) = plan(&l, &m.merged, "seeds:sync");
    if !allow_deletes && (!lr.removed.is_empty() || !rr.removed.is_empty()) {
        return Err(SdError::refused(format!(
            "sync would remove {} seed(s) from the local store ({}) and {} from the remote ({}) \
             because the other side does not have them; nothing was written. That usually \
             means the remote was reset or is a different store. If the removals are \
             intended, run again with --allow-remote-deletes.",
            lr.removed.len(),
            lr.removed.join(", "),
            rr.removed.len(),
            rr.removed.join(", ")
        )));
    }
    if !rb.is_empty() {
        rr.tx = remote.commit(&rb, ctx)?;
        rr.wrote = true;
    }
    if !lb.is_empty() {
        lr.tx = local.commit(&lb, ctx)?;
        lr.wrote = true;
    }
    Ok((m.merged, lr, rr))
}

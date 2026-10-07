//! Pushing a write too large for one request: split it into ordered batches.
//!
//! A quipu server caps a request body (quipu refuses `/update` bodies over 64
//! MiB with HTTP 413). A first sync of a whole board is one batch far over
//! that: 27,983 beads made a 286 MB body. [`split_for_cap`] cuts such a batch
//! into batches that each fit, and [`commit_capped`] commits them in order.
//!
//! Each batch is its own transaction, so a push can stop part way. That is
//! safe to re-run: sync plans the remote write as the difference between the
//! remote and the merge, and a seed that already landed identically is not
//! written again. The order keeps every batch valid on its own:
//!
//! * a seed comes after the seeds it is blocked on, and seeds that block each
//!   other (a cycle) travel together, because the shapes refuse an edge to a
//!   seed that is not there yet;
//! * a comment travels with its seed, except the comments past the clause
//!   limit, which follow it in later batches;
//! * removals, and comments renumbered by sync, go last, in one batch.
//!
//! A batch is also bounded by its guard clauses: each new seed adds two
//! `FILTER NOT EXISTS`, each comment one, each replaced or removed item one
//! `UNION` branch. quipu-server aborts on an `/update` nesting a couple of
//! thousand of them (aegis-rq1afp), long before the body is large, so a
//! backend that knows such a limit reports it as
//! [`Backend::max_write_clauses`].
//!
//! The split is planned from an over-estimate of each item's request size.
//! The backend still checks the real size and refuses before sending; a
//! batch refused that way is halved and retried, so an estimate that is low
//! costs a retry, never a failed push.

use std::collections::{BTreeMap, BTreeSet};

use crate::backend::{is_too_large, Backend, Ctx, WriteBatch};
use crate::error::{Result, SdError};
use crate::validate::push_ntriples;
use crate::vocab;

/// Request bytes one item can cost beyond its own triples: the existence or
/// revision guard, the replace pattern with its modelled-predicate filter,
/// and its line in the write record.
const ITEM_OVERHEAD: usize = 2048;
/// Request bytes a batch costs before any item: the update skeleton and the
/// write record's own facts.
const BATCH_OVERHEAD: usize = 16 * 1024;
/// Form encoding turns one byte into at most three (`%XX`).
const ENCODING: usize = 3;

/// One indivisible piece of a batch: a set of seeds with their comments, or
/// the trailing removals.
#[derive(Default)]
struct Unit {
    batch: WriteBatch,
    bytes: usize,
    clauses: usize,
}

/// Guard clauses a seed write adds: absent from both graphs when new, one
/// replace branch when it updates.
fn seed_clauses(w: &crate::backend::SeedWrite) -> usize {
    if w.expected_revision.is_none() {
        2
    } else {
        1
    }
}

/// The guard clauses `batch` nests in one update (an upper bound).
pub fn clause_count(batch: &WriteBatch) -> usize {
    batch.seeds.iter().map(seed_clauses).sum::<usize>()
        + batch.comments.len()
        + batch.delete_seeds.len()
        + batch.delete_comments.len()
}

fn seed_bytes(w: &crate::backend::SeedWrite) -> usize {
    let mut nt = String::new();
    push_ntriples(&mut nt, &vocab::item_iri(&w.seed.id), &w.seed.facts());
    ENCODING * (nt.len() + ITEM_OVERHEAD)
}

fn comment_bytes(c: &crate::model::Comment) -> usize {
    let mut nt = String::new();
    push_ntriples(&mut nt, &vocab::comment_iri(&c.seed, c.index), &c.facts());
    ENCODING * (nt.len() + ITEM_OVERHEAD)
}

/// An over-estimate of the request bytes `batch` needs.
pub fn estimate_bytes(batch: &WriteBatch) -> usize {
    BATCH_OVERHEAD
        + batch.seeds.iter().map(seed_bytes).sum::<usize>()
        + batch.comments.iter().map(comment_bytes).sum::<usize>()
        + ENCODING * ITEM_OVERHEAD * (batch.delete_seeds.len() + batch.delete_comments.len())
}

/// The strongly connected components of the `blocked_on` graph among the
/// batch's own seeds, blockers first (Tarjan emits a component only after
/// every component it reaches, and an edge points at the blocker).
fn components(batch: &WriteBatch) -> Vec<Vec<usize>> {
    let index: BTreeMap<&str, usize> = batch
        .seeds
        .iter()
        .enumerate()
        .map(|(i, w)| (w.seed.id.as_str(), i))
        .collect();
    let edges: Vec<Vec<usize>> = batch
        .seeds
        .iter()
        .map(|w| {
            w.seed
                .blocked_on
                .iter()
                .filter_map(|b| index.get(b.as_str()).copied())
                .collect()
        })
        .collect();
    let n = batch.seeds.len();
    let (mut order, mut low) = (vec![usize::MAX; n], vec![0usize; n]);
    let (mut on_stack, mut stack, mut out) = (vec![false; n], Vec::new(), Vec::new());
    let mut next = 0usize;
    for root in 0..n {
        if order[root] != usize::MAX {
            continue;
        }
        // Iterative, so a long dependency chain cannot overflow the stack.
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        order[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (v, ref mut e)) = work.last_mut() {
            if let Some(&w) = edges[v].get(*e) {
                *e += 1;
                if order[w] == usize::MAX {
                    order[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(order[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == order[v] {
                let mut comp = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                comp.sort_unstable();
                out.push(comp);
            }
        }
    }
    out
}

/// `clauses` bounds a unit only where it can be cut: the comments of a seed.
fn units(batch: &WriteBatch, clauses: usize) -> Vec<Unit> {
    let renumbered: BTreeSet<(&str, u64)> = batch
        .delete_comments
        .iter()
        .map(|(s, i)| (s.as_str(), *i))
        .collect();
    let mut comments_of: BTreeMap<&str, Vec<&crate::model::Comment>> = BTreeMap::new();
    let mut last = Unit::default();
    let mut loose = Vec::new();
    let in_batch: BTreeSet<&str> = batch.seeds.iter().map(|w| w.seed.id.as_str()).collect();
    for c in &batch.comments {
        if renumbered.contains(&(c.seed.as_str(), c.index)) {
            // Its absence guard is waived by the paired removal, so the two
            // must share a transaction.
            last.bytes += comment_bytes(c);
            last.clauses += 1;
            last.batch.comments.push(c.clone());
        } else if in_batch.contains(c.seed.as_str()) {
            comments_of.entry(c.seed.as_str()).or_default().push(c);
        } else {
            loose.push(Unit {
                bytes: comment_bytes(c),
                clauses: 1,
                batch: WriteBatch {
                    comments: vec![c.clone()],
                    ..WriteBatch::default()
                },
            });
        }
    }
    let mut out: Vec<Unit> = Vec::new();
    for comp in components(batch) {
        let mut u = Unit::default();
        // Comments past the clause limit continue in units of their own,
        // right behind the seed: a seed can carry more comments than one
        // write may guard (aegis-h7xuql has 558).
        let mut overflow: Vec<Unit> = Vec::new();
        for i in comp {
            let w = &batch.seeds[i];
            u.bytes += seed_bytes(w);
            u.clauses += seed_clauses(w);
            u.batch.seeds.push(w.clone());
            for c in comments_of.get(w.seed.id.as_str()).into_iter().flatten() {
                if overflow.last().map_or(u.clauses, |t| t.clauses) >= clauses {
                    overflow.push(Unit::default());
                }
                let tail = overflow.last_mut().unwrap_or(&mut u);
                tail.bytes += comment_bytes(c);
                tail.clauses += 1;
                tail.batch.comments.push((*c).clone());
            }
        }
        out.push(u);
        out.extend(overflow);
    }
    out.extend(loose);
    last.batch.delete_seeds = batch.delete_seeds.clone();
    last.batch.delete_comments = batch.delete_comments.clone();
    last.bytes +=
        ENCODING * ITEM_OVERHEAD * (batch.delete_seeds.len() + batch.delete_comments.len());
    last.clauses += batch.delete_seeds.len() + batch.delete_comments.len();
    if !last.batch.is_empty() {
        out.push(last);
    }
    out
}

/// Split `batch` into batches of at most `cap` estimated request bytes and
/// `clauses` guard clauses ([`clause_count`]), in an order where each is
/// valid once the ones before it have landed. A batch that fits is returned
/// whole. A single unit over either limit (one seed, or one dependency cycle)
/// becomes a batch of its own, and the backend's check then refuses it by
/// name.
pub fn split_for_cap(batch: &WriteBatch, cap: usize, clauses: usize) -> Vec<WriteBatch> {
    if estimate_bytes(batch) <= cap && clause_count(batch) <= clauses {
        return vec![batch.clone()];
    }
    let budget = cap.saturating_sub(BATCH_OVERHEAD).max(1);
    let mut out: Vec<WriteBatch> = Vec::new();
    let mut cur = WriteBatch::default();
    let (mut used, mut nested) = (0usize, 0usize);
    for u in units(batch, clauses) {
        if !cur.is_empty() && (used + u.bytes > budget || nested + u.clauses > clauses) {
            out.push(std::mem::take(&mut cur));
            used = 0;
            nested = 0;
        }
        cur.seeds.extend(u.batch.seeds);
        cur.comments.extend(u.batch.comments);
        cur.delete_seeds.extend(u.batch.delete_seeds);
        cur.delete_comments.extend(u.batch.delete_comments);
        used += u.bytes;
        nested += u.clauses;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    for b in &mut out {
        b.source.clone_from(&batch.source);
    }
    out
}

/// Halve `batch` along its unit boundaries, keeping their order. `None`
/// when it is one unit and cannot be split.
fn halve(batch: &WriteBatch) -> Option<(WriteBatch, WriteBatch)> {
    let us = units(batch, usize::MAX);
    if us.len() < 2 {
        return None;
    }
    let mid = us.len() / 2;
    let join = |part: &mut dyn Iterator<Item = Unit>| {
        let mut b = WriteBatch {
            source: batch.source.clone(),
            ..WriteBatch::default()
        };
        for u in part {
            b.seeds.extend(u.batch.seeds);
            b.comments.extend(u.batch.comments);
            b.delete_seeds.extend(u.batch.delete_seeds);
            b.delete_comments.extend(u.batch.delete_comments);
        }
        b
    };
    let mut it = us.into_iter();
    let first = join(&mut it.by_ref().take(mid));
    let second = join(&mut it);
    Some((first, second))
}

/// Progress of a split push, for the caller to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchDone {
    /// Batches committed so far (1-based).
    pub done: usize,
    /// Batches planned. It can grow when a batch is refused as too large and
    /// halved.
    pub planned: usize,
    /// The committed batch's transaction.
    pub tx: u64,
    /// Seeds, comments and removals in the committed batch.
    pub items: usize,
}

fn items(b: &WriteBatch) -> usize {
    b.seeds.len() + b.comments.len() + b.delete_seeds.len() + b.delete_comments.len()
}

/// Commit `batch` to `target`, split by [`split_for_cap`] when the target has
/// a write limit and the batch is over it. Returns the last transaction.
///
/// When a batch after the first fails, the error keeps its kind and says how
/// many batches landed, so a re-run (which skips what landed) is the remedy.
pub fn commit_capped(
    target: &mut dyn Backend,
    batch: &WriteBatch,
    ctx: &Ctx,
    on_batch: &mut dyn FnMut(BatchDone),
) -> Result<u64> {
    let (bytes, clauses) = (target.max_write_bytes(), target.max_write_clauses());
    if bytes.is_none() && clauses.is_none() {
        return target.commit(batch, ctx);
    }
    let mut queue: std::collections::VecDeque<WriteBatch> = split_for_cap(
        batch,
        bytes.unwrap_or(usize::MAX),
        clauses.unwrap_or(usize::MAX),
    )
    .into();
    if queue.len() == 1 {
        // One batch: exactly the single atomic commit as before, including a
        // TOO_LARGE refusal when even the whole write is one indivisible unit.
        let only = queue.pop_front().unwrap_or_default();
        return match target.commit(&only, ctx) {
            Err(e) if is_too_large(&e) => match halve(&only) {
                Some((a, b)) => {
                    queue.push_back(a);
                    queue.push_back(b);
                    push_queue(target, queue, ctx, on_batch, 0, 0)
                }
                None => Err(e),
            },
            r => r,
        };
    }
    push_queue(target, queue, ctx, on_batch, 0, 0)
}

fn push_queue(
    target: &mut dyn Backend,
    mut queue: std::collections::VecDeque<WriteBatch>,
    ctx: &Ctx,
    on_batch: &mut dyn FnMut(BatchDone),
    mut done: usize,
    mut tx: u64,
) -> Result<u64> {
    while let Some(b) = queue.pop_front() {
        match target.commit(&b, ctx) {
            Ok(t) => {
                done += 1;
                tx = t;
                on_batch(BatchDone {
                    done,
                    planned: done + queue.len(),
                    tx,
                    items: items(&b),
                });
            }
            Err(e) if is_too_large(&e) => match halve(&b) {
                Some((first, second)) => {
                    queue.push_front(second);
                    queue.push_front(first);
                }
                None => return Err(partial(e, done, done + 1 + queue.len())),
            },
            Err(e) => return Err(partial(e, done, done + 1 + queue.len())),
        }
    }
    Ok(tx)
}

fn partial(e: SdError, done: usize, planned: usize) -> SdError {
    if done == 0 {
        return e;
    }
    SdError::new(
        e.kind,
        format!(
            "the write was split into {planned} batches; {done} landed, then batch {} failed: \
             {}. The batches that landed are complete transactions. Resolve the failure as its \
             own message says, then re-run the same command to continue: what landed is not \
             written again.",
            done + 1,
            e.message
        ),
    )
}

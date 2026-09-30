//! The work-item model: a seed, its comments, and how both map to facts.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value as Json};

use crate::error::{Result, SdError};
use crate::vocab::{self, term};

/// Status values a seed can hold.
pub const STATUSES: &[&str] = &["open", "in_progress", "blocked", "deferred", "closed"];
/// Types a seed can have (br's set).
pub const TYPES: &[&str] = &[
    "task", "bug", "feature", "epic", "chore", "docs", "question",
];
/// Dependency types `dep add --type` accepts. Only `blocks` affects `ready`.
pub const DEP_TYPES: &[&str] = &["blocks", "related", "parent-child", "discovered-from"];
/// The default priority, as in br.
pub const DEFAULT_PRIORITY: u8 = 2;

/// Validate a status token.
pub fn parse_status(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase().replace('-', "_");
    if STATUSES.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(SdError::usage(format!(
            "unknown status {s:?}; expected one of {}",
            STATUSES.join(", ")
        )))
    }
}

/// Validate a type token.
pub fn parse_type(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if TYPES.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(SdError::usage(format!(
            "unknown type {s:?}; expected one of {}",
            TYPES.join(", ")
        )))
    }
}

/// Parse a priority: `0`..`4`, optionally written `P0`..`P4`.
pub fn parse_priority(s: &str) -> Result<u8> {
    let t = s.trim();
    let digits = t.strip_prefix(['P', 'p']).unwrap_or(t);
    match digits.parse::<u8>() {
        Ok(p) if p <= 4 => Ok(p),
        _ => Err(SdError::usage(format!(
            "priority must be 0-4 (or P0-P4), got {s:?}"
        ))),
    }
}

/// Validate a dependency type.
pub fn parse_dep_type(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if DEP_TYPES.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(SdError::usage(format!(
            "unknown dependency type {s:?}; expected one of {}",
            DEP_TYPES.join(", ")
        )))
    }
}

/// One work item.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Seed {
    /// The id, e.g. `sd-a3f` or `sd-a3f.1`.
    pub id: String,
    /// The title (`rdfs:label`).
    pub title: String,
    /// Long description.
    pub description: Option<String>,
    /// Free-form notes.
    pub notes: Option<String>,
    /// One of [`STATUSES`].
    pub status: String,
    /// 0 (highest) to 4.
    pub priority: u8,
    /// One of [`TYPES`].
    pub issue_type: String,
    /// Who holds it.
    pub assignee: Option<String>,
    /// Labels, sorted.
    pub labels: BTreeSet<String>,
    /// When it was created (ISO-8601 UTC).
    pub created_at: String,
    /// Who created it.
    pub created_by: Option<String>,
    /// When it last changed.
    pub updated_at: String,
    /// When it was closed, if it is closed.
    pub closed_at: Option<String>,
    /// Why it was closed.
    pub close_reason: Option<String>,
    /// Hidden from `ready` until this date or instant.
    pub defer_until: Option<String>,
    /// `blocks` dependencies: this seed cannot proceed past these ids.
    pub blocked_on: BTreeSet<String>,
    /// `related` dependencies.
    pub related: BTreeSet<String>,
    /// `parent-child`: this seed's parent.
    pub parent: Option<String>,
    /// `discovered-from` dependencies.
    pub discovered_from: BTreeSet<String>,
    /// The shuttle workflow run that created or drives this seed, as the
    /// run's IRI (`urn:shuttle:run:<id>`). The run's definition is reachable
    /// from the run through camayoc's `aegis:runOf`. See
    /// `docs/book/src/formulas.md`.
    pub workflow_run: Option<String>,
    /// The compare-and-set token: 1 at create, +1 on every write to the seed.
    pub revision: u64,
}

/// One comment on a seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// The seed it belongs to.
    pub seed: String,
    /// 1-based position among that seed's comments; also its id.
    pub index: u64,
    /// Who wrote it.
    pub author: String,
    /// What it says.
    pub text: String,
    /// When (ISO-8601 UTC).
    pub created_at: String,
}

/// The object of a fact, before it is interned into a particular store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Obj {
    /// An IRI.
    Iri(String),
    /// A string literal.
    Str(String),
    /// An integer literal.
    Int(i64),
}

/// A (predicate IRI, object) pair about one subject.
pub type Fact = (String, Obj);

impl Seed {
    /// The facts that describe this seed, in a stable order.
    pub fn facts(&self) -> Vec<Fact> {
        let mut f: Vec<Fact> = vec![
            (vocab::RDF_TYPE.into(), Obj::Iri(term::work_item())),
            (term::source_kind(), Obj::Str("declared".into())),
            (term::identifier(), Obj::Str(self.id.clone())),
            (vocab::RDFS_LABEL.into(), Obj::Str(self.title.clone())),
            (term::status(), Obj::Str(self.status.clone())),
            (term::priority(), Obj::Int(i64::from(self.priority))),
            (term::issue_type(), Obj::Str(self.issue_type.clone())),
            (term::created_at(), Obj::Str(self.created_at.clone())),
            (term::updated_at(), Obj::Str(self.updated_at.clone())),
            (term::revision(), Obj::Int(self.revision as i64)),
        ];
        let opt = |f: &mut Vec<Fact>, p: String, v: &Option<String>| {
            if let Some(v) = v {
                f.push((p, Obj::Str(v.clone())));
            }
        };
        opt(&mut f, term::description(), &self.description);
        opt(&mut f, term::notes(), &self.notes);
        opt(&mut f, term::created_by(), &self.created_by);
        opt(&mut f, term::closed_at(), &self.closed_at);
        opt(&mut f, term::close_reason(), &self.close_reason);
        opt(&mut f, term::defer_until(), &self.defer_until);
        if self.status == "closed" {
            f.push((term::outcome(), Obj::Str("done".into())));
        }
        if let Some(a) = &self.assignee {
            f.push((term::assigned_to(), Obj::Iri(vocab::principal_iri(a))));
        }
        for l in &self.labels {
            f.push((term::label(), Obj::Str(l.clone())));
        }
        for d in &self.blocked_on {
            f.push((term::blocked_on(), Obj::Iri(vocab::item_iri(d))));
        }
        for d in &self.related {
            f.push((term::related_to(), Obj::Iri(vocab::item_iri(d))));
        }
        if let Some(p) = &self.parent {
            f.push((term::child_of(), Obj::Iri(vocab::item_iri(p))));
        }
        for d in &self.discovered_from {
            f.push((term::discovered_from(), Obj::Iri(vocab::item_iri(d))));
        }
        if let Some(r) = &self.workflow_run {
            f.push((term::workflow_run(), Obj::Iri(r.clone())));
        }
        f
    }

    /// Rebuild a seed from its facts. `None` when the facts do not describe a
    /// seed (no `aegis:WorkItem` type or no identifier).
    pub fn from_facts(facts: &[Fact]) -> Option<Seed> {
        let mut by: BTreeMap<&str, Vec<&Obj>> = BTreeMap::new();
        for (p, o) in facts {
            by.entry(p.as_str()).or_default().push(o);
        }
        let is_item = by
            .get(vocab::RDF_TYPE)
            .is_some_and(|v| v.contains(&&Obj::Iri(term::work_item())));
        if !is_item {
            return None;
        }
        let s = |p: String| -> Option<String> {
            by.get(p.as_str()).and_then(|v| {
                v.iter().find_map(|o| match o {
                    Obj::Str(s) => Some(s.clone()),
                    _ => None,
                })
            })
        };
        let i = |p: String| -> Option<i64> {
            by.get(p.as_str()).and_then(|v| {
                v.iter().find_map(|o| match o {
                    Obj::Int(n) => Some(*n),
                    _ => None,
                })
            })
        };
        let ids = |p: String| -> BTreeSet<String> {
            by.get(p.as_str())
                .map(|v| {
                    v.iter()
                        .filter_map(|o| match o {
                            Obj::Iri(iri) => vocab::item_id(iri),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let strs = |p: String| -> BTreeSet<String> {
            by.get(p.as_str())
                .map(|v| {
                    v.iter()
                        .filter_map(|o| match o {
                            Obj::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let id = s(term::identifier())?;
        Some(Seed {
            id,
            title: s(vocab::RDFS_LABEL.into()).unwrap_or_default(),
            description: s(term::description()),
            notes: s(term::notes()),
            status: s(term::status()).unwrap_or_else(|| "open".into()),
            priority: i(term::priority())
                .and_then(|p| u8::try_from(p).ok())
                .unwrap_or(DEFAULT_PRIORITY),
            issue_type: s(term::issue_type()).unwrap_or_else(|| "task".into()),
            assignee: by.get(term::assigned_to().as_str()).and_then(|v| {
                v.iter().find_map(|o| match o {
                    Obj::Iri(iri) => vocab::principal_name(iri),
                    Obj::Str(s) => Some(s.clone()),
                    Obj::Int(_) => None,
                })
            }),
            labels: strs(term::label()),
            created_at: s(term::created_at()).unwrap_or_default(),
            created_by: s(term::created_by()),
            updated_at: s(term::updated_at()).unwrap_or_default(),
            closed_at: s(term::closed_at()),
            close_reason: s(term::close_reason()),
            defer_until: s(term::defer_until()),
            blocked_on: ids(term::blocked_on()),
            related: ids(term::related_to()),
            parent: ids(term::child_of()).into_iter().next(),
            discovered_from: ids(term::discovered_from()),
            workflow_run: by.get(term::workflow_run().as_str()).and_then(|v| {
                v.iter().find_map(|o| match o {
                    Obj::Iri(iri) => Some(iri.clone()),
                    _ => None,
                })
            }),
            revision: i(term::revision())
                .and_then(|r| u64::try_from(r).ok())
                .unwrap_or(0),
        })
    }

    /// Every dependency this seed declares, as (depends-on id, type).
    pub fn dependencies(&self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = Vec::new();
        out.extend(self.blocked_on.iter().map(|d| (d.clone(), "blocks")));
        out.extend(self.parent.iter().map(|d| (d.clone(), "parent-child")));
        out.extend(self.related.iter().map(|d| (d.clone(), "related")));
        out.extend(
            self.discovered_from
                .iter()
                .map(|d| (d.clone(), "discovered-from")),
        );
        out
    }

    /// Whether this seed declares a dependency of `dep_type` on `target`.
    pub fn has_dep(&self, target: &str, dep_type: &str) -> bool {
        match dep_type {
            "blocks" => self.blocked_on.contains(target),
            "related" => self.related.contains(target),
            "parent-child" => self.parent.as_deref() == Some(target),
            "discovered-from" => self.discovered_from.contains(target),
            _ => false,
        }
    }

    /// Add a dependency. Returns false when it was already there.
    pub fn add_dep(&mut self, target: &str, dep_type: &str) -> bool {
        if self.has_dep(target, dep_type) {
            return false;
        }
        match dep_type {
            "blocks" => {
                self.blocked_on.insert(target.to_string());
            }
            "related" => {
                self.related.insert(target.to_string());
            }
            "parent-child" => self.parent = Some(target.to_string()),
            _ => {
                self.discovered_from.insert(target.to_string());
            }
        }
        true
    }

    /// Remove a dependency. Returns false when it was not there.
    pub fn remove_dep(&mut self, target: &str, dep_type: &str) -> bool {
        if !self.has_dep(target, dep_type) {
            return false;
        }
        match dep_type {
            "blocks" => {
                self.blocked_on.remove(target);
            }
            "related" => {
                self.related.remove(target);
            }
            "parent-child" => self.parent = None,
            _ => {
                self.discovered_from.remove(target);
            }
        }
        true
    }

    /// The bd-shaped JSON object for this seed (the keys `list` and `ready`
    /// print). Every key is always present; absent values are `null`.
    pub fn to_json(&self) -> Json {
        json!({
            "id": self.id,
            "title": self.title,
            "description": self.description,
            "notes": self.notes,
            "status": self.status,
            "priority": self.priority,
            "issue_type": self.issue_type,
            "assignee": self.assignee,
            "labels": self.labels.iter().collect::<Vec<_>>(),
            "created_at": self.created_at,
            "created_by": self.created_by,
            "updated_at": self.updated_at,
            "closed_at": self.closed_at,
            "close_reason": self.close_reason,
            "defer_until": self.defer_until,
            "parent": self.parent,
            "dependency_count": self.dependencies().len(),
            "workflow_run": self.workflow_run,
            "revision": self.revision,
        })
    }
}

impl Comment {
    /// The facts that describe this comment.
    pub fn facts(&self) -> Vec<Fact> {
        vec![
            (vocab::RDF_TYPE.into(), Obj::Iri(term::comment())),
            (term::comment_on(), Obj::Iri(vocab::item_iri(&self.seed))),
            (term::comment_index(), Obj::Int(self.index as i64)),
            (term::author(), Obj::Str(self.author.clone())),
            (term::text(), Obj::Str(self.text.clone())),
            (term::created_at(), Obj::Str(self.created_at.clone())),
        ]
    }

    /// Rebuild a comment from its facts.
    pub fn from_facts(facts: &[Fact]) -> Option<Comment> {
        let mut seed = None;
        let mut index = None;
        let mut author = None;
        let mut text = None;
        let mut created_at = None;
        let mut typed = false;
        for (p, o) in facts {
            match (p.as_str(), o) {
                (vocab::RDF_TYPE, Obj::Iri(c)) if *c == term::comment() => typed = true,
                (p, Obj::Iri(i)) if p == term::comment_on() => seed = vocab::item_id(i),
                (p, Obj::Int(n)) if p == term::comment_index() => index = u64::try_from(*n).ok(),
                (p, Obj::Str(s)) if p == term::author() => author = Some(s.clone()),
                (p, Obj::Str(s)) if p == term::text() => text = Some(s.clone()),
                (p, Obj::Str(s)) if p == term::created_at() => created_at = Some(s.clone()),
                _ => {}
            }
        }
        if !typed {
            return None;
        }
        Some(Comment {
            seed: seed?,
            index: index?,
            author: author.unwrap_or_default(),
            text: text.unwrap_or_default(),
            created_at: created_at.unwrap_or_default(),
        })
    }

    /// bd's comment JSON shape.
    pub fn to_json(&self) -> Json {
        json!({
            "id": self.index,
            "issue_id": self.seed,
            "author": self.author,
            "text": self.text,
            "created_at": self.created_at,
        })
    }
}

/// Every seed and comment in one project, as of one transaction.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Seeds by id.
    pub seeds: BTreeMap<String, Seed>,
    /// Comments, ordered by (seed, index).
    pub comments: Vec<Comment>,
    /// The transaction this snapshot reflects (0 for an empty store).
    pub tx: u64,
}

impl Snapshot {
    /// Build a snapshot from facts grouped by subject IRI.
    pub fn from_subjects(by_subject: &BTreeMap<String, Vec<Fact>>) -> Snapshot {
        let mut snap = Snapshot::default();
        for facts in by_subject.values() {
            if let Some(seed) = Seed::from_facts(facts) {
                snap.seeds.insert(seed.id.clone(), seed);
            } else if let Some(c) = Comment::from_facts(facts) {
                snap.comments.push(c);
            }
        }
        snap.comments
            .sort_by(|a, b| (&a.seed, a.index).cmp(&(&b.seed, b.index)));
        snap
    }

    /// A seed by id, or a not-found error.
    pub fn get(&self, id: &str) -> Result<&Seed> {
        self.seeds.get(id).ok_or_else(|| SdError::not_found(id))
    }

    /// The seeds that declare any dependency on `id`, as (dependent id, type).
    pub fn dependents(&self, id: &str) -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        for s in self.seeds.values() {
            for (target, t) in s.dependencies() {
                if target == id {
                    out.push((s.id.clone(), t));
                }
            }
        }
        out
    }

    /// The ids of `seed`'s `blocks` targets that are not closed.
    pub fn open_blockers(&self, seed: &Seed) -> Vec<String> {
        seed.blocked_on
            .iter()
            .filter(|b| self.seeds.get(*b).is_some_and(|s| s.status != "closed"))
            .cloned()
            .collect()
    }

    /// Comments on one seed, in order.
    pub fn comments_on(&self, id: &str) -> Vec<&Comment> {
        self.comments.iter().filter(|c| c.seed == id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Seed {
        Seed {
            id: "sd-abc".into(),
            title: "Ship it".into(),
            description: Some("the \"parser\"\nnow".into()),
            status: "open".into(),
            priority: 1,
            issue_type: "bug".into(),
            assignee: Some("ian".into()),
            labels: ["x".to_string(), "y".to_string()].into(),
            created_at: "2026-09-30T00:00:00Z".into(),
            updated_at: "2026-09-30T00:00:00Z".into(),
            blocked_on: ["sd-def".to_string()].into(),
            parent: Some("sd-p".into()),
            workflow_run: Some("urn:shuttle:run:triage-7".into()),
            revision: 3,
            ..Seed::default()
        }
    }

    #[test]
    fn seed_facts_round_trip() {
        let s = sample();
        assert_eq!(Seed::from_facts(&s.facts()), Some(s));
    }

    #[test]
    fn closed_seed_carries_camayoc_outcome() {
        let mut s = sample();
        s.status = "closed".into();
        assert!(s
            .facts()
            .contains(&(term::outcome(), Obj::Str("done".into()))));
    }

    #[test]
    fn parsers_accept_br_forms_and_refuse_typos() {
        assert_eq!(parse_priority("P1").unwrap(), 1);
        assert_eq!(parse_priority("4").unwrap(), 4);
        assert!(parse_priority("5").is_err());
        assert_eq!(parse_status("in-progress").unwrap(), "in_progress");
        assert!(parse_status("done").is_err());
        assert!(parse_type("tsak").is_err());
        assert!(parse_dep_type("block").is_err());
    }
}

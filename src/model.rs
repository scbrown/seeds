//! The work-item model: a seed, its comments, and how both map to facts.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value as Json};

use crate::error::{Result, SdError};
use crate::vocab::{self, term};

/// Status values a seed can hold.
pub const STATUSES: &[&str] = &[
    "open",
    "hooked",
    "in_progress",
    "blocked",
    "deferred",
    "closed",
    TOMBSTONE,
];

/// The status `sd delete` sets. A tombstone is kept (so sync propagates the
/// delete as an ordinary change and `--at` still reads what was there), but it
/// is hidden from every listing, never blocks, cannot gain a dependency, and is
/// not a close: it carries no outcome (aegis-w3k75d.8).
pub const TOMBSTONE: &str = "tombstone";
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

/// How a closed seed ended: the governed `quechua:outcome` values.
pub const OUTCOMES: [&str; 4] = ["done", "abandoned", "superseded", "failed"];

/// Parse a close outcome (one of [`OUTCOMES`]).
pub fn parse_outcome(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if OUTCOMES.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(SdError::usage(format!(
            "unknown outcome {s:?}; expected one of {}",
            OUTCOMES.join(", ")
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
    /// The title (`schema:name`, and `rdfs:label` with the same value).
    pub title: String,
    /// Long description.
    pub description: Option<String>,
    /// Free-form notes.
    pub notes: Option<String>,
    /// Design notes (br's `design`).
    pub design: Option<String>,
    /// Governing instructions for an agent (br's `agent_context`): compact
    /// JSON text.
    pub agent_context: Option<String>,
    /// Acceptance criteria (br's `acceptance_criteria`).
    pub acceptance_criteria: Option<String>,
    /// A reference to the same work elsewhere (br's `external_ref`).
    pub external_ref: Option<String>,
    /// When the work is due (br's `due_at`): a date or an RFC 3339 instant.
    pub due_at: Option<String>,
    /// A time estimate in minutes (br's `estimated_minutes`).
    pub estimated_minutes: Option<u32>,
    /// Who owns the work (br's `owner`, usually an email); distinct from the
    /// assignee, who is doing it now.
    pub owner: Option<String>,
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
    /// How it ended, when closed: one of [`OUTCOMES`] (`done` when a close
    /// did not say). A machine-readable field, so a workflow can branch on it
    /// without parsing the reason.
    pub outcome: Option<String>,
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
    /// Whether the seed lives in the project's ephemeral graph (br's
    /// `--ephemeral`): visible to every read, never shared or exported, never
    /// ready. Not a fact: it is which graph the seed's facts are in.
    pub ephemeral: bool,
    /// Facts this sd does not model (a NEWER sd wrote them), as
    /// (predicate, object). Read by `from_facts`, written back by `facts`,
    /// so import, sync and renumber carry them instead of dropping them
    /// (aegis-w3k75d.14).
    pub extra: BTreeSet<(String, Obj)>,
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
    /// Facts this sd does not model, as on [`Seed::extra`].
    pub extra: BTreeSet<(String, Obj)>,
}

/// The object of a fact, before it is interned into a particular store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Obj {
    /// An IRI.
    Iri(String),
    /// A string literal.
    Str(String),
    /// An integer literal.
    Int(i64),
    /// A language-tagged literal: the lexical form and the BCP47 tag, apart
    /// (never `"x@en"` in a string). seeds writes none; a NEWER sd may, and
    /// it must round-trip (aegis-w3k75d.14).
    Lang {
        /// The lexical form, without the tag.
        lexical: String,
        /// The tag, without the `@`.
        lang: String,
    },
    /// Any other typed literal (`xsd:boolean`, `xsd:date`, `xsd:dateTime`,
    /// `xsd:decimal`, a custom datatype ...), lexical form verbatim, so a
    /// newer sd's dates and booleans keep their datatype through this sd.
    Typed {
        /// The lexical form, as written.
        lexical: String,
        /// The datatype IRI, in full.
        datatype: String,
    },
}

/// `xsd:integer`.
pub const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
/// `xsd:string`.
pub const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
/// `xsd:boolean`.
pub const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";
/// `xsd:double`.
pub const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";

impl Obj {
    /// A literal from its RDF parts, classified the way quipu's own ingest
    /// does (`rdf::literal_to_value`): a language tag wins; `xsd:integer`
    /// that parses is [`Obj::Int`]; no datatype or `xsd:string` is
    /// [`Obj::Str`]; anything else keeps its datatype as [`Obj::Typed`].
    pub fn literal(lexical: String, datatype: Option<&str>, lang: Option<&str>) -> Obj {
        if let Some(lang) = lang.filter(|l| !l.is_empty()) {
            return Obj::Lang {
                lexical,
                lang: lang.to_string(),
            };
        }
        match datatype {
            None | Some("") | Some(XSD_STRING) => Obj::Str(lexical),
            Some(XSD_INTEGER) => match lexical.parse() {
                Ok(n) => Obj::Int(n),
                Err(_) => Obj::Typed {
                    lexical,
                    datatype: XSD_INTEGER.into(),
                },
            },
            Some(dt) => Obj::Typed {
                lexical,
                datatype: dt.to_string(),
            },
        }
    }
}

/// A (predicate IRI, object) pair about one subject.
pub type Fact = (String, Obj);

impl Seed {
    /// Whether `sd delete` tombstoned this seed.
    pub fn is_tombstone(&self) -> bool {
        self.status == TOMBSTONE
    }

    /// The facts that describe this seed, in a stable order.
    pub fn facts(&self) -> Vec<Fact> {
        let mut f: Vec<Fact> = vec![
            (vocab::RDF_TYPE.into(), Obj::Iri(term::work_item())),
            (term::source_kind(), Obj::Str("declared".into())),
            (term::identifier(), Obj::Str(self.id.clone())),
            (term::name(), Obj::Str(self.title.clone())),
            (vocab::RDFS_LABEL.into(), Obj::Str(self.title.clone())),
            (term::status(), Obj::Str(self.status.clone())),
            (term::priority(), Obj::Int(i64::from(self.priority))),
            (term::issue_type(), Obj::Str(self.issue_type.clone())),
            (term::created_at(), date_time(&self.created_at)),
            (term::updated_at(), date_time(&self.updated_at)),
            (term::revision(), Obj::Int(self.revision as i64)),
        ];
        if let Some(a) = action_status(&self.status, self.outcome.as_deref()) {
            f.push((term::action_status(), Obj::Iri(vocab::schema(a))));
        }
        let opt = |f: &mut Vec<Fact>, p: String, v: &Option<String>| {
            if let Some(v) = v {
                f.push((p, Obj::Str(v.clone())));
            }
        };
        let person = |f: &mut Vec<Fact>, p: String, v: &Option<String>| {
            if let Some(v) = v {
                f.push((p, Obj::Iri(vocab::principal_iri(v))));
            }
        };
        opt(&mut f, term::description(), &self.description);
        opt(&mut f, term::notes(), &self.notes);
        opt(&mut f, term::design(), &self.design);
        opt(&mut f, term::agent_context(), &self.agent_context);
        opt(
            &mut f,
            term::acceptance_criteria(),
            &self.acceptance_criteria,
        );
        opt(&mut f, term::external_ref(), &self.external_ref);
        if let Some(d) = &self.due_at {
            f.push((term::due_at(), date_or_date_time(d)));
        }
        if let Some(m) = self.estimated_minutes {
            f.push((term::estimated_minutes(), minutes_duration(m)));
        }
        person(&mut f, term::owner(), &self.owner);
        person(&mut f, term::created_by(), &self.created_by);
        if let Some(c) = &self.closed_at {
            f.push((term::closed_at(), date_time(c)));
        }
        opt(&mut f, term::close_reason(), &self.close_reason);
        if let Some(d) = &self.defer_until {
            f.push((term::defer_until(), date_or_date_time(d)));
        }
        if self.status == "closed" {
            let outcome = self.outcome.clone().unwrap_or_else(|| "done".into());
            f.push((term::outcome(), Obj::Str(outcome)));
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
        f.extend(self.extra.iter().cloned());
        f
    }

    /// Every predicate this build writes on a seed or a comment. A write
    /// replaces only these; any other predicate on the entity (written by a
    /// NEWER sd that models more) is carried forward untouched, so an older
    /// writer never erases what it does not understand (aegis-w3k75d.13). The
    /// set is derived from `facts()` on a seed and a comment with every field
    /// set, so it cannot fall behind `facts()`.
    pub fn modelled_predicates() -> &'static BTreeSet<String> {
        static SET: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
        SET.get_or_init(|| {
            let comment = Comment {
                seed: "x".into(),
                index: 1,
                author: "a".into(),
                text: "t".into(),
                created_at: "c".into(),
                extra: BTreeSet::new(),
            };
            Seed::maximal()
                .facts()
                .into_iter()
                .chain(comment.facts())
                .map(|(p, _)| p)
                .collect()
        })
    }

    /// A seed with EVERY field set, so its `facts()` names every predicate a
    /// seed can carry. A test fails if a field is left unset here (a new field
    /// must be added, or clearing it would stop working).
    #[doc(hidden)]
    pub fn maximal() -> Seed {
        {
            let some = |s: &str| Some(s.to_string());
            let one = |s: &str| [s.to_string()].into_iter().collect::<BTreeSet<_>>();
            Seed {
                id: "x".into(),
                title: "t".into(),
                description: some("d"),
                notes: some("n"),
                design: some("g"),
                agent_context: some("{}"),
                acceptance_criteria: some("k"),
                external_ref: some("e"),
                due_at: some("2026-01-01"),
                estimated_minutes: Some(30),
                owner: some("o"),
                status: "closed".into(),
                issue_type: "task".into(),
                assignee: some("a"),
                labels: one("l"),
                created_at: "c".into(),
                created_by: some("b"),
                updated_at: "u".into(),
                closed_at: some("c"),
                close_reason: some("r"),
                outcome: some("done"),
                defer_until: some("f"),
                blocked_on: one("y"),
                related: one("y"),
                parent: some("p"),
                discovered_from: one("y"),
                workflow_run: some("urn:w"),
                ..Seed::default()
            }
        }
    }

    /// Rebuild a seed from its facts. `None` when the facts do not describe a
    /// seed (no `schema:Action` type or no identifier). The derived
    /// `schema:actionStatus` and the `rdfs:label` beside `schema:name` are
    /// modelled, so they are recomputed on the next write, never carried as
    /// extras.
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
        // A time's lexical form, whatever datatype it arrived with (an older
        // store wrote plain strings).
        // Canonical when valid, so a seed read from any store compares equal
        // to the same seed read from any other.
        let t = |p: String| -> Option<String> {
            by.get(p.as_str()).and_then(|v| {
                v.iter().find_map(|o| match o {
                    Obj::Str(s) | Obj::Typed { lexical: s, .. } => Some(canonical_or_same(s, true)),
                    _ => None,
                })
            })
        };
        // A principal: its IRI's name, or a plain string as written.
        let who = |p: String| -> Option<String> {
            by.get(p.as_str()).and_then(|v| {
                v.iter().find_map(|o| match o {
                    Obj::Iri(iri) => vocab::principal_name(iri),
                    Obj::Str(s) => Some(s.clone()),
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
            title: s(term::name())
                .or_else(|| s(vocab::RDFS_LABEL.into()))
                .unwrap_or_default(),
            description: s(term::description()),
            notes: s(term::notes()),
            design: s(term::design()),
            agent_context: s(term::agent_context()),
            acceptance_criteria: s(term::acceptance_criteria()),
            external_ref: s(term::external_ref()),
            due_at: t(term::due_at()),
            estimated_minutes: t(term::estimated_minutes())
                .as_deref()
                .and_then(parse_minutes_duration),
            owner: who(term::owner()),
            status: s(term::status()).unwrap_or_else(|| "open".into()),
            priority: i(term::priority())
                .and_then(|p| u8::try_from(p).ok())
                .unwrap_or(DEFAULT_PRIORITY),
            issue_type: s(term::issue_type()).unwrap_or_else(|| "task".into()),
            assignee: who(term::assigned_to()),
            labels: strs(term::label()),
            created_at: t(term::created_at()).unwrap_or_default(),
            created_by: who(term::created_by()),
            updated_at: t(term::updated_at()).unwrap_or_default(),
            closed_at: t(term::closed_at()),
            close_reason: s(term::close_reason()),
            outcome: s(term::outcome()),
            defer_until: t(term::defer_until()),
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
            // Set by Snapshot::from_graphs, which knows the graph.
            ephemeral: false,
            extra: unread_estimate(facts)
                .chain(unmodelled(facts, &term::work_item()))
                .collect(),
        })
    }

    /// Why this seed's times cannot be written, one line per bad field, empty
    /// when they all can. Nothing invalid is repaired: a value that is not a
    /// valid `xsd:dateTime` (or, for due and defer, `xsd:date`) is refused. A
    /// valid one is written in canonical form, the same instant
    /// ([`canonical_time`]).
    pub fn time_problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut check = |field: &str, v: Option<&str>, date_ok: bool| {
            if let Some(v) = v {
                if canonical_time(v, date_ok).is_none() {
                    out.push(format!(
                        "{}: {field} {v:?} is not {}",
                        self.id,
                        if date_ok {
                            "an xsd:date (YYYY-MM-DD) or an xsd:dateTime"
                        } else {
                            "an xsd:dateTime"
                        }
                    ));
                }
            }
        };
        check("created_at", Some(&self.created_at), false);
        check("updated_at", Some(&self.updated_at), false);
        check("closed_at", self.closed_at.as_deref(), false);
        check("due_at", self.due_at.as_deref(), true);
        check("defer_until", self.defer_until.as_deref(), true);
        out
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

    /// Bridge-owned carry stays in `extra` on reads, but replaces old values on writes.
    pub(crate) fn owned_extra_predicates() -> [String; 2] {
        [vocab::seeds("beadsJson"), vocab::seeds("dependencyOrigin")]
    }

    /// Creation attribution keyed by (target, dependency type), never item creator.
    pub(crate) fn dependency_origins(
        &self,
    ) -> Result<BTreeMap<(String, String), (String, String)>> {
        let mut origins = BTreeMap::new();
        for (_, value) in self
            .extra
            .iter()
            .filter(|(p, _)| *p == vocab::seeds("dependencyOrigin"))
        {
            let Obj::Str(text) = value else {
                return Err(SdError::refused("dependency origin must be a JSON string"));
            };
            let (target, kind, at, actor): (String, String, String, String) =
                serde_json::from_str(text)
                    .map_err(|_| SdError::refused("malformed dependency origin"))?;
            if origins.insert((target, kind), (at, actor)).is_some() {
                return Err(SdError::conflict("duplicate dependency origin"));
            }
        }
        Ok(origins)
    }

    pub(crate) fn set_dependency_origin(
        &mut self,
        target: &str,
        kind: &str,
        at: &str,
        actor: &str,
    ) -> Result<()> {
        let mut origins = self.dependency_origins()?;
        origins.insert((target.into(), kind.into()), (at.into(), actor.into()));
        let predicate = vocab::seeds("dependencyOrigin");
        self.extra.retain(|(p, _)| *p != predicate);
        for ((target, kind), (at, actor)) in origins {
            self.extra.insert((
                predicate.clone(),
                Obj::Str(json!([target, kind, at, actor]).to_string()),
            ));
        }
        Ok(())
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
            "design": self.design,
            "agent_context": self.agent_context,
            "acceptance_criteria": self.acceptance_criteria,
            "external_ref": self.external_ref,
            "due_at": self.due_at,
            "estimated_minutes": self.estimated_minutes,
            "owner": self.owner,
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
            "outcome": self.outcome,
            "defer_until": self.defer_until,
            "parent": self.parent,
            "dependency_count": self.dependencies().len(),
            "workflow_run": self.workflow_run,
            "revision": self.revision,
            "ephemeral": self.ephemeral,
        })
    }
}

impl Comment {
    /// The facts that describe this comment.
    pub fn facts(&self) -> Vec<Fact> {
        let mut f = vec![
            (vocab::RDF_TYPE.into(), Obj::Iri(term::comment())),
            (term::comment_on(), Obj::Iri(vocab::item_iri(&self.seed))),
            (term::comment_index(), Obj::Int(self.index as i64)),
            (term::author(), Obj::Iri(vocab::principal_iri(&self.author))),
            (term::text(), Obj::Str(self.text.clone())),
            (term::created_at(), date_time(&self.created_at)),
        ];
        f.extend(self.extra.iter().cloned());
        f
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
                (p, Obj::Iri(i)) if p == term::author() => author = vocab::principal_name(i),
                (p, Obj::Str(s)) if p == term::author() => author = Some(s.clone()),
                (p, Obj::Str(s)) if p == term::text() => text = Some(s.clone()),
                (p, Obj::Str(s) | Obj::Typed { lexical: s, .. }) if p == term::created_at() => {
                    created_at = Some(canonical_or_same(s, false))
                }
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
            extra: unmodelled(facts, &term::comment()),
        })
    }

    /// Why this comment's time cannot be written (see
    /// [`Seed::time_problems`]).
    pub fn time_problems(&self) -> Vec<String> {
        if canonical_time(&self.created_at, false).is_some() {
            return Vec::new();
        }
        vec![format!(
            "{} comment {}: created_at {:?} is not an xsd:dateTime",
            self.seed, self.index, self.created_at
        )]
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

/// The derived `schema:actionStatus` (its local name) for a status and
/// outcome, written in the same write as the status so the two cannot drift
/// (the shapes check they agree). `None` for a tombstone, which is not an
/// action any more. A closed seed with no outcome counts as `done`, the
/// outcome `facts()` writes for it.
pub fn action_status(status: &str, outcome: Option<&str>) -> Option<&'static str> {
    match status {
        "open" | "hooked" | "blocked" | "deferred" => Some("PotentialActionStatus"),
        "in_progress" => Some("ActiveActionStatus"),
        "closed" => match outcome.unwrap_or("done") {
            "done" => Some("CompletedActionStatus"),
            _ => Some("FailedActionStatus"),
        },
        _ => None,
    }
}

/// An instant as an `xsd:dateTime` literal, in its canonical lexical form
/// (an invalid one is kept as given, and [`Seed::time_problems`] refuses it).
fn date_time(v: &str) -> Obj {
    Obj::Typed {
        lexical: canonical_time(v, false).unwrap_or_else(|| v.to_string()),
        datatype: vocab::XSD_DATE_TIME.into(),
    }
}

/// A due or defer value: `xsd:date` when it is a bare `YYYY-MM-DD`, else
/// `xsd:dateTime`, in canonical form ([`Seed::time_problems`] refuses one
/// that is neither).
fn date_or_date_time(v: &str) -> Obj {
    if is_xsd_date(v) {
        Obj::Typed {
            lexical: canonical_time(v, true).unwrap_or_else(|| v.to_string()),
            datatype: vocab::XSD_DATE.into(),
        }
    } else {
        date_time(v)
    }
}

/// A minute estimate as an `xsd:duration` of that many minutes, in canonical
/// form: `PT30M`, `PT1H30M`, `P1D`, `PT0S`.
fn minutes_duration(m: u32) -> Obj {
    let lexical = format!("PT{m}M");
    Obj::Typed {
        lexical: canonical_duration(&lexical).unwrap_or(lexical),
        datatype: vocab::XSD_DURATION.into(),
    }
}

/// The XSD canonical lexical form of a valid time: an `xsd:dateTime`, or,
/// when `date_ok`, a bare `YYYY-MM-DD` `xsd:date`. `None` for anything else.
///
/// Canonicalisation keeps the VALUE and changes only the spelling: trailing
/// zeros in fractional seconds go (`.500Z` is `.5Z`, `.000Z` is `Z`),
/// `+00:00` is `Z`, and `24:00:00` is the next day's `00:00:00`. It runs
/// through `oxsdatatypes`, the code a quipu server's `/update` stores typed
/// literals with, so a seed's bytes are the same in a local store, on a
/// server and after a sync between them.
pub fn canonical_time(v: &str, date_ok: bool) -> Option<String> {
    if date_ok && is_xsd_date(v) {
        return v.parse::<oxsdatatypes::Date>().ok().map(|d| d.to_string());
    }
    if !is_xsd_date_time(v) {
        return None;
    }
    v.parse::<oxsdatatypes::DateTime>()
        .ok()
        .map(|d| d.to_string())
}

/// The XSD canonical lexical form of a valid `xsd:duration`.
pub fn canonical_duration(v: &str) -> Option<String> {
    v.parse::<oxsdatatypes::Duration>()
        .ok()
        .map(|d| d.to_string())
}

/// `v` canonicalised as [`canonical_time`] does, or unchanged when it is not
/// a valid time (so the refusal names what the caller gave).
pub fn canonical_or_same(v: &str, date_ok: bool) -> String {
    canonical_time(v, date_ok).unwrap_or_else(|| v.to_string())
}

/// The whole minutes in a day/hour/minute duration. seeds writes the XSD
/// canonical form (`PT1H30M`, `P1D`, `PT0S`) and reads any day/hour/minute
/// spelling (`PT90M` too) as the same minutes. Anything else (years, months, a sign, a
/// fraction, seconds that are not zero) is not read as minutes: the fact is
/// carried as an extra instead, so it is neither rewritten nor dropped.
fn parse_minutes_duration(v: &str) -> Option<u32> {
    let rest = v.strip_prefix('P')?;
    let (day_part, time_part) = match rest.split_once('T') {
        Some((d, t)) if !t.is_empty() => (d, Some(t)),
        Some(_) => return None,
        None => (rest, None),
    };
    if day_part.is_empty() && time_part.is_none() {
        return None;
    }
    // Number-designator pairs in the order XSD requires, each at most once.
    let fields = |s: &str, order: &[u8]| -> Option<Vec<(u8, u64)>> {
        let mut out = Vec::new();
        let mut n = String::new();
        let mut next = 0;
        for c in s.bytes() {
            if c.is_ascii_digit() {
                n.push(c as char);
                continue;
            }
            let at = order[next..].iter().position(|&d| d == c)? + next;
            if n.is_empty() {
                return None;
            }
            out.push((c, n.parse().ok()?));
            n.clear();
            next = at + 1;
        }
        n.is_empty().then_some(out)
    };
    let mut minutes: u64 = 0;
    for (d, n) in fields(day_part, b"D")? {
        debug_assert_eq!(d, b'D');
        minutes = minutes.checked_add(n.checked_mul(1440)?)?;
    }
    if let Some(t) = time_part {
        for (d, n) in fields(t, b"HMS")? {
            minutes = match d {
                b'H' => minutes.checked_add(n.checked_mul(60)?)?,
                b'M' => minutes.checked_add(n)?,
                _ if n == 0 => minutes,
                _ => return None,
            };
        }
    }
    u32::try_from(minutes).ok()
}

/// A `schema:timeRequired` this sd cannot read as minutes, kept as an extra.
fn unread_estimate(facts: &[Fact]) -> impl Iterator<Item = (String, Obj)> + '_ {
    let p = term::estimated_minutes();
    facts
        .iter()
        .filter(move |(q, o)| {
            *q == p
                && !matches!(o, Obj::Str(s) | Obj::Typed { lexical: s, .. }
                    if parse_minutes_duration(s).is_some())
        })
        .cloned()
}

/// Whether `s` is a bare `YYYY-MM-DD` that names a real day: the only
/// `xsd:date` form seeds writes (no time zone).
pub fn is_xsd_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && digits(&b[..4])
        && real_day(&b[..4], &b[5..7], &b[8..10])
}

/// Whether `s` is a valid `xsd:dateTime` lexical form (XML Schema 1.1):
/// `YYYY-MM-DDThh:mm:ss`, optional fractional seconds, optional `Z` or
/// `+hh:mm`/`-hh:mm`. A four-digit year only (seeds' instants are all
/// four-digit), `24:00:00` allowed as the end of a day. Pure byte checks, so
/// the wasm build carries it.
pub fn is_xsd_date_time(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return false;
    }
    if b[13] != b':' || b[16] != b':' {
        return false;
    }
    if !digits(&b[..4]) || !real_day(&b[..4], &b[5..7], &b[8..10]) {
        return false;
    }
    let (Some(h), Some(mi), Some(sec)) = (two(&b[11..13]), two(&b[14..16]), two(&b[17..19])) else {
        return false;
    };
    let mut rest = &b[19..];
    let mut fraction_nonzero = false;
    if let Some(f) = rest.strip_prefix(b".") {
        let n = f.iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return false;
        }
        fraction_nonzero = f[..n].iter().any(|&c| c != b'0');
        rest = &f[n..];
    }
    let time_ok =
        (h < 24 && mi < 60 && sec < 60) || (h == 24 && mi == 0 && sec == 0 && !fraction_nonzero);
    if !time_ok {
        return false;
    }
    match rest {
        b"" | b"Z" => true,
        [sign, tz @ ..] if (*sign == b'+' || *sign == b'-') && tz.len() == 5 && tz[2] == b':' => {
            match (two(&tz[..2]), two(&tz[3..])) {
                (Some(th), Some(tm)) => (th < 14 && tm < 60) || (th == 14 && tm == 0),
                _ => false,
            }
        }
        _ => false,
    }
}

fn digits(b: &[u8]) -> bool {
    b.iter().all(u8::is_ascii_digit)
}

/// A two-digit field's value.
fn two(b: &[u8]) -> Option<u32> {
    (b.len() == 2 && digits(b)).then(|| u32::from(b[0] - b'0') * 10 + u32::from(b[1] - b'0'))
}

/// Whether year/month/day (as ASCII digits) name a real Gregorian day.
fn real_day(y: &[u8], m: &[u8], d: &[u8]) -> bool {
    let (Some(m), Some(d)) = (two(m), two(d)) else {
        return false;
    };
    let y: u32 = std::str::from_utf8(y)
        .ok()
        .and_then(|y| y.parse().ok())
        .unwrap_or(0);
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

/// The facts among `facts` this sd does not model: any predicate outside
/// [`Seed::modelled_predicates`], and any `rdf:type` besides the entity's own
/// class (`own_type`), which a newer sd may add as a second type.
fn unmodelled(facts: &[Fact], own_type: &str) -> BTreeSet<(String, Obj)> {
    let modelled = Seed::modelled_predicates();
    facts
        .iter()
        .filter(|(p, o)| {
            if p == vocab::RDF_TYPE {
                *o != Obj::Iri(own_type.to_string())
            } else {
                !modelled.contains(p)
            }
        })
        .cloned()
        .collect()
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
    /// THE read of a project: the project graph and its ephemeral graph,
    /// unioned, with every seed from the second marked ephemeral. Every
    /// backend's `snapshot` goes through this, so no read path can see one
    /// graph without the other.
    pub fn from_graphs(
        project: &BTreeMap<String, Vec<Fact>>,
        ephemeral: &BTreeMap<String, Vec<Fact>>,
    ) -> Snapshot {
        let mut snap = Snapshot::from_subjects(project);
        let eph = Snapshot::from_subjects(ephemeral);
        for (id, mut seed) in eph.seeds {
            seed.ephemeral = true;
            snap.seeds.insert(id, seed);
        }
        snap.comments.extend(eph.comments);
        snap.comments
            .sort_by(|a, b| (&a.seed, a.index).cmp(&(&b.seed, b.index)));
        snap
    }

    /// The part of the project a ledger carries: every seed except the
    /// ephemeral ones, and their comments. sync, import and the pendant
    /// compare THIS, so an ephemeral seed is never pushed, pulled or deleted
    /// by a ledger that cannot hold it.
    pub fn shared(&self) -> Snapshot {
        let seeds: BTreeMap<String, Seed> = self
            .seeds
            .iter()
            .filter(|(_, s)| !s.ephemeral)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let comments = self
            .comments
            .iter()
            .filter(|c| seeds.contains_key(&c.seed))
            .cloned()
            .collect();
        Snapshot {
            seeds,
            comments,
            tx: self.tx,
        }
    }

    /// Build a snapshot from facts grouped by subject IRI (one graph).
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
            .filter(|b| {
                self.seeds
                    .get(*b)
                    .is_some_and(|s| s.status != "closed" && !s.is_tombstone())
            })
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

    // aegis-w3k75d.13: modelled_predicates() is derived from Seed::maximal(),
    // so a field left unset there would drop out of it, and clearing that field
    // would silently stop retracting its old value. Every field must be set.
    #[test]
    fn the_maximal_seed_sets_every_field_so_no_predicate_is_missed() {
        let j = Seed::maximal().to_json();
        for (k, v) in j.as_object().unwrap() {
            let empty = v.is_null() || v.as_array().is_some_and(Vec::is_empty);
            assert!(!empty, "Seed::maximal() leaves {k} unset; set it there");
        }
        let p = Seed::modelled_predicates();
        assert!(p.contains(&term::outcome()) && p.contains(&term::workflow_run()));
        assert!(p.contains(&term::text()), "comment predicates are in too");
    }

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
    fn closed_seed_carries_the_governed_outcome() {
        let mut s = sample();
        s.status = "closed".into();
        assert!(s
            .facts()
            .contains(&(term::outcome(), Obj::Str("done".into()))));
    }

    // aegis-bqgdr3 table v1.1: every field round-trips through the typed,
    // schema.org-first facts, with the derived terms never read back as extras.
    #[test]
    fn the_maximal_seed_round_trips_with_no_extras() {
        let mut s = Seed::maximal();
        s.created_at = "2026-09-06T18:47:22.616951891Z".into();
        s.updated_at = "2026-09-06T18:47:22Z".into();
        s.closed_at = Some("2026-09-07T00:00:00+02:00".into());
        s.defer_until = Some("2026-10-03T14:00:00Z".into());
        s.revision = 7;
        let back = Seed::from_facts(&s.facts()).unwrap();
        assert!(back.extra.is_empty(), "{:?}", back.extra);
        assert_eq!(back, s);
        let f = s.facts();
        let name = f.iter().filter(|(p, _)| *p == term::name()).count();
        let label = f.iter().filter(|(p, _)| p == vocab::RDFS_LABEL).count();
        assert_eq!((name, label), (1, 1), "name and label once each");
        assert!(f.contains(&(
            term::estimated_minutes(),
            Obj::Typed {
                lexical: "PT30M".into(),
                datatype: vocab::XSD_DURATION.into()
            }
        )));
        assert!(f.contains(&(
            term::due_at(),
            Obj::Typed {
                lexical: "2026-01-01".into(),
                datatype: vocab::XSD_DATE.into()
            }
        )));
        assert!(f.contains(&(term::created_by(), Obj::Iri(vocab::principal_iri("b")))));
    }

    #[test]
    fn action_status_is_derived_from_status_and_outcome() {
        let cases = [
            ("open", None, Some("PotentialActionStatus")),
            ("hooked", None, Some("PotentialActionStatus")),
            ("blocked", None, Some("PotentialActionStatus")),
            ("deferred", None, Some("PotentialActionStatus")),
            ("in_progress", None, Some("ActiveActionStatus")),
            ("closed", None, Some("CompletedActionStatus")),
            ("closed", Some("done"), Some("CompletedActionStatus")),
            ("closed", Some("abandoned"), Some("FailedActionStatus")),
            ("closed", Some("superseded"), Some("FailedActionStatus")),
            ("closed", Some("failed"), Some("FailedActionStatus")),
            (TOMBSTONE, None, None),
        ];
        for (status, outcome, want) in cases {
            assert_eq!(action_status(status, outcome), want, "{status} {outcome:?}");
        }
    }

    #[test]
    fn a_duration_seeds_does_not_write_is_carried_not_dropped() {
        let mut f = sample().facts();
        let odd = (
            term::estimated_minutes(),
            Obj::Typed {
                lexical: "P1M".into(),
                datatype: vocab::XSD_DURATION.into(),
            },
        );
        f.push(odd.clone());
        let s = Seed::from_facts(&f).unwrap();
        assert_eq!(
            s.estimated_minutes, None,
            "a month is not coerced to minutes"
        );
        assert!(s.facts().contains(&odd), "and written back as it was");
    }

    // A quipu server's /update stores the XSD canonical form of a duration
    // (measured: PT90M reads back as PT1H30M, PT0M as PT0S). Those are the
    // same minutes, so they must read as the estimate, not as an extra.
    #[test]
    fn canonical_durations_read_back_as_the_same_minutes() {
        for (lexical, want) in [
            ("PT90M", Some(90)),
            ("PT1H30M", Some(90)),
            ("PT0S", Some(0)),
            ("P1D", Some(1440)),
            ("P1DT1M", Some(1441)),
            ("PT2H", Some(120)),
            ("PT1M0S", Some(1)),
            ("PT1M30S", None),
            ("PT1.5M", None),
            ("P1M", None),
            ("P1Y", None),
            ("-PT5M", None),
            ("PT", None),
            ("P", None),
            ("PT30H1H", None),
            ("PTM", None),
        ] {
            assert_eq!(parse_minutes_duration(lexical), want, "{lexical}");
        }
        // Written canonical, read back as the same minutes.
        let mut s = sample();
        for (m, lexical) in [(90, "PT1H30M"), (0, "PT0S"), (1440, "P1D"), (30, "PT30M")] {
            s.estimated_minutes = Some(m);
            let f = s.facts();
            assert!(f.contains(&(
                term::estimated_minutes(),
                Obj::Typed {
                    lexical: lexical.into(),
                    datatype: vocab::XSD_DURATION.into()
                }
            )));
            assert_eq!(Seed::from_facts(&f).unwrap(), s);
        }
    }

    #[test]
    fn xsd_lexical_forms_are_checked_not_coerced() {
        for ok in [
            "2026-10-03T14:00:00Z",
            "2026-08-30T13:35:59.014436677Z",
            "2026-09-01T22:41:04.116Z",
            "2026-10-03T14:00:00",
            "2026-10-03T14:00:00-05:00",
            "2026-10-03T14:00:00+14:00",
            "2026-10-03T24:00:00Z",
            "2024-02-29T00:00:00Z",
        ] {
            assert!(is_xsd_date_time(ok), "{ok}");
        }
        for bad in [
            "2026-10-03 14:00:00Z",
            "2026-10-03T14:00Z",
            "2026-10-03T14:00:00.Z",
            "2026-10-03T14:00:00z",
            "2026-10-03T14:00:00+0500",
            "2026-10-03T14:00:00+15:00",
            "2026-10-03T24:00:01Z",
            "2026-10-03T14:60:00Z",
            "2026-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-10-03T14:00:00Zjunk",
            "2026-10-03",
            "",
        ] {
            assert!(!is_xsd_date_time(bad), "{bad}");
        }
        assert!(is_xsd_date("2026-10-03"));
        for bad in ["2026-10-3", "2026-02-30", "2026-10-03Z", "tomorrow"] {
            assert!(!is_xsd_date(bad), "{bad}");
        }
        let mut s = sample();
        s.defer_until = Some("next tuesday".into());
        s.due_at = Some("2026-10-09".into());
        s.closed_at = Some("2026-10-09".into());
        let p = s.time_problems();
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(
            p[0].contains("closed_at \"2026-10-09\" is not an xsd:dateTime"),
            "{p:?}"
        );
        assert!(p[1].contains("defer_until \"next tuesday\""), "{p:?}");
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

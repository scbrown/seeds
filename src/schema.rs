//! The `--json` contract as data: every object's fields and their JSON types.
//!
//! `sd schema` emits JSON Schemas generated from these tables, and
//! `tests/json_schema.rs` pins the key sets it checks from the same tables, so
//! the published schema and the enforced contract cannot drift apart.

use serde_json::{json, Map, Value as Json};

use crate::error::ErrorKind;
use crate::model::{OUTCOMES, STATUSES, TYPES};

/// The JSON type of one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A string.
    Str,
    /// A string or null.
    OptStr,
    /// An integer.
    Int,
    /// An integer or null.
    OptInt,
    /// A number (integer or fraction).
    Num,
    /// A boolean.
    Bool,
    /// An array of strings.
    Strs,
    /// One of these strings.
    Enum(&'static [&'static str]),
    /// One of these strings, or null.
    OptEnum(&'static [&'static str]),
}

/// A seed object: `show`, `list`, `ready`, `create`, `update`, `close`.
pub const SEED: &[(&str, Kind)] = &[
    ("id", Kind::Str),
    ("title", Kind::Str),
    ("description", Kind::OptStr),
    ("notes", Kind::OptStr),
    ("design", Kind::OptStr),
    ("agent_context", Kind::OptStr),
    ("acceptance_criteria", Kind::OptStr),
    ("external_ref", Kind::OptStr),
    ("due_at", Kind::OptStr),
    ("estimated_minutes", Kind::OptInt),
    ("owner", Kind::OptStr),
    ("status", Kind::Enum(STATUSES)),
    ("priority", Kind::Int),
    ("issue_type", Kind::Enum(TYPES)),
    ("assignee", Kind::OptStr),
    ("labels", Kind::Strs),
    ("created_at", Kind::Str),
    ("created_by", Kind::OptStr),
    ("updated_at", Kind::Str),
    ("closed_at", Kind::OptStr),
    ("close_reason", Kind::OptStr),
    ("outcome", Kind::OptEnum(&OUTCOMES)),
    ("defer_until", Kind::OptStr),
    ("parent", Kind::OptStr),
    ("dependency_count", Kind::Int),
    ("workflow_run", Kind::OptStr),
    ("revision", Kind::Int),
    ("ephemeral", Kind::Bool),
];

/// One end of a dependency, inside `show`'s `dependencies`/`dependents`.
pub const EDGE: &[(&str, Kind)] = &[
    ("id", Kind::Str),
    ("title", Kind::OptStr),
    ("status", Kind::OptStr),
    ("priority", Kind::Int),
    ("dependency_type", Kind::Str),
];

/// A comment: `comments add`, `comments list`, `show`'s `comments`.
pub const COMMENT: &[(&str, Kind)] = &[
    ("id", Kind::Int),
    ("issue_id", Kind::Str),
    ("author", Kind::Str),
    ("text", Kind::Str),
    ("created_at", Kind::Str),
];

/// The names of a table's fields, sorted: the key set `--json` must print.
pub fn keys(fields: &[(&'static str, Kind)]) -> Vec<&'static str> {
    let mut k: Vec<&'static str> = fields.iter().map(|(n, _)| *n).collect();
    k.sort_unstable();
    k
}

fn kind(k: Kind) -> Json {
    match k {
        Kind::Str => json!({"type": "string"}),
        Kind::OptStr => json!({"type": ["string", "null"]}),
        Kind::Int => json!({"type": "integer"}),
        Kind::OptInt => json!({"type": ["integer", "null"]}),
        Kind::Num => json!({"type": "number"}),
        Kind::Bool => json!({"type": "boolean"}),
        Kind::Strs => json!({"type": "array", "items": {"type": "string"}}),
        Kind::Enum(v) => json!({"type": "string", "enum": v}),
        Kind::OptEnum(v) => {
            let mut e: Vec<Json> = v.iter().map(|s| json!(s)).collect();
            e.push(Json::Null);
            json!({"type": ["string", "null"], "enum": e})
        }
    }
}

/// A closed-world object schema: every field required, no others.
pub fn object(fields: &[(&str, Kind)], extra: &[(&str, Json)]) -> Json {
    let mut props = Map::new();
    let mut required: Vec<String> = Vec::new();
    for (name, k) in fields {
        props.insert((*name).to_string(), kind(*k));
        required.push((*name).to_string());
    }
    for (name, schema) in extra {
        props.insert((*name).to_string(), schema.clone());
        required.push((*name).to_string());
    }
    json!({"type": "object", "properties": props, "required": required,
           "additionalProperties": false})
}

fn seed_with(extra: &[(&str, Json)]) -> Json {
    object(SEED, extra)
}

/// Every target `sd schema` can emit, with a one-line description.
pub const TARGETS: &[(&str, &str)] = &[
    (
        "issue",
        "a seed object (show, ready, create, update, close)",
    ),
    (
        "issue-with-counts",
        "a list/full-search row: a seed plus dependent_count",
    ),
    (
        "search-issue",
        "a search row, with nullable dependency/dependent counts in title mode",
    ),
    (
        "issue-details",
        "a show row: a seed plus dependencies, dependents, comments",
    ),
    ("ready-issue", "a ready row"),
    ("stale-issue", "a stale row"),
    (
        "blocked-issue",
        "a blocked row: a seed plus blocked_by, blocked_by_count, dependent_count",
    ),
    ("comment", "a comment"),
    ("statistics", "stats output"),
    (
        "info",
        "info counts with explicit omitted or exact comment count",
    ),
    (
        "error",
        "the --json error envelope (stdout, with a non-zero exit)",
    ),
    ("commands", "each verb's top-level --json shape"),
];

/// The schema for one target, or None for an unknown name.
pub fn target(name: &str) -> Option<Json> {
    let edge = object(EDGE, &[]);
    let comment = object(COMMENT, &[]);
    let dependent_count = ("dependent_count", kind(Kind::Int));
    Some(match name {
        "issue" | "ready-issue" | "stale-issue" => seed_with(&[]),
        "issue-with-counts" => seed_with(&[dependent_count]),
        "search-issue" => seed_with(&[
            ("dependency_count", kind(Kind::OptInt)),
            ("dependent_count", kind(Kind::OptInt)),
        ]),
        "issue-details" => seed_with(&[
            (
                "dependencies",
                json!({"type": "array", "items": edge.clone()}),
            ),
            ("dependents", json!({"type": "array", "items": edge})),
            ("comments", json!({"type": "array", "items": comment})),
        ]),
        "blocked-issue" => seed_with(&[
            dependent_count,
            ("blocked_by", kind(Kind::Strs)),
            ("blocked_by_count", kind(Kind::Int)),
        ]),
        "comment" => comment,
        "info" => {
            json!({"type":"object", "required":["issue_count","comment_count","comment_count_status"],
            "properties":{"issue_count":{"type":"integer","minimum":0},"comment_count":{"type":["integer","null"],"minimum":0},
                "comment_count_status":{"enum":["not_computed","exact"]}},
            "allOf":[
                {"if":{"properties":{"comment_count_status":{"const":"not_computed"}}},"then":{"properties":{"comment_count":{"type":"null"}}}},
                {"if":{"properties":{"comment_count_status":{"const":"exact"}}},"then":{"properties":{"comment_count":{"type":"integer"}}}}
            ]})
        }
        "statistics" => {
            let ints = [
                "total_issues",
                "open_issues",
                "in_progress_issues",
                "closed_issues",
                "blocked_issues",
                "deferred_issues",
                "draft_issues",
                "ready_issues",
                "tombstone_issues",
                "pinned_issues",
                "epics_eligible_for_closure",
            ];
            let mut fields: Vec<(&str, Kind)> = ints.iter().map(|n| (*n, Kind::Int)).collect();
            fields.push(("average_lead_time_hours", Kind::Num));
            json!({"type": "object", "required": ["summary"], "properties": {
                "summary": object(&fields, &[]),
                "breakdowns": {"type": "array", "items": {"type": "object",
                    "required": ["dimension", "counts"], "properties": {
                        "dimension": {"type": "string", "enum": ["type", "priority", "assignee", "label"]},
                        "counts": {"type": "array", "items": object(&[("key", Kind::Str), ("count", Kind::Int)], &[])},
                    }}},
            }})
        }
        "error" => {
            let codes: Vec<&str> = ErrorKind::ALL.iter().map(|k| k.name()).collect();
            json!({"type": "object", "required": ["error"], "properties": {"error":
                object(&[("code", Kind::Str), ("message", Kind::Str)], &[])
                    .as_object()
                    .map(|o| {
                        let mut o = o.clone();
                        o["properties"]["code"] = json!({"type": "string", "enum": codes});
                        Json::Object(o)
                    })
                    .unwrap_or(Json::Null)}})
        }
        "commands" => commands(),
        _ => return None,
    })
}

/// Each verb's top-level `--json` shape: `object`, `array` of a named item,
/// or an `envelope` whose `issues` are a named item, with a jq filter to the rows.
pub fn commands() -> Json {
    let rows: &[(&str, &str, &str, &str)] = &[
        ("create", "object", "issue", "."),
        ("show", "array", "issue-details", ".[]"),
        ("list", "envelope", "issue-with-counts", ".issues[]"),
        ("search", "envelope", "search-issue", ".issues[]"),
        ("blocked", "envelope", "blocked-issue", ".issues[]"),
        ("ready", "array", "ready-issue", ".[]"),
        ("stale", "array", "stale-issue", ".[]"),
        ("update", "array", "issue", ".[]"),
        ("close", "array", "issue", ".[]"),
        ("comments add", "object", "comment", "."),
        ("comments list", "array", "comment", ".[]"),
        ("stats", "object", "statistics", ".summary"),
        ("info", "object", "info", "."),
    ];
    let mut m = Map::new();
    for (verb, shape, item, jq) in rows {
        m.insert(
            (*verb).to_string(),
            json!({"shape": shape, "item": item, "jq": jq}),
        );
    }
    Json::Object(m)
}

/// `sd schema all`: every target in one bundle.
pub fn all(version: &str) -> Json {
    let mut schemas = Map::new();
    for (name, _) in TARGETS {
        if *name != "commands" {
            if let Some(s) = target(name) {
                schemas.insert((*name).to_string(), s);
            }
        }
    }
    json!({"tool": "sd", "version": version,
           "$schema": "https://json-schema.org/draft/2020-12/schema",
           "schemas": schemas, "commands": commands()})
}

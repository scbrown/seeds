//! Lossless beads JSONL bridge. The original JSON is an unmodelled fact;
//! modeled edits overlay it, retaining absent/null distinctions and metadata.
use crate::{
    error::{Result, SdError},
    model::{Comment, Obj, Seed, Snapshot},
    vocab,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Records keyed by exact external ID, never fuzzy resolved.
pub type Records = BTreeMap<String, Value>;
fn shadow() -> String {
    vocab::seeds("beadsJson")
}

/// Parse JSONL, refusing duplicate IDs and malformed records before writes.
pub fn parse(text: &str) -> Result<Records> {
    let mut out = Records::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value = serde_json::from_str(line)
            .map_err(|e| SdError::usage(format!("line {}: {e}", n + 1)))?;
        let id = required(&v, "id")?.to_string();
        if id.is_empty()
            || id
                .chars()
                .any(|c| c.is_whitespace() || "<>\"{}|\\^`".contains(c))
        {
            return Err(SdError::usage(format!("invalid id {id:?}")));
        }
        if out.insert(id.clone(), v).is_some() {
            return Err(SdError::conflict(format!("duplicate id {id}")));
        }
    }
    Ok(out)
}
fn required<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| SdError::usage(format!("{}: {k} must be a string", v["id"])))
}
fn array<'a>(v: &'a Value, k: &str) -> Result<&'a [Value]> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        _ => Err(SdError::usage(format!("{}: {k} must be an array", v["id"]))),
    }
}
fn seed(v: &Value) -> Result<Seed> {
    let mut s = Seed {
        id: required(v, "id")?.into(),
        title: required(v, "title")?.into(),
        status: required(v, "status")?.into(),
        issue_type: required(v, "issue_type")?.into(),
        created_at: required(v, "created_at")?.into(),
        updated_at: required(v, "updated_at")?.into(),
        revision: 1,
        ..Seed::default()
    };
    s.priority = v["priority"]
        .as_u64()
        .filter(|p| *p <= 4)
        .ok_or_else(|| SdError::usage(format!("{}: invalid priority", s.id)))?
        as u8;
    macro_rules! optional { ($($k:ident),*) => { $(s.$k = match v.get(stringify!($k)) { None | Some(Value::Null) => None, Some(Value::String(t)) => Some(t.clone()), _ => return Err(SdError::usage(format!("{}: {} must be a string or null",s.id,stringify!($k)))) };)* }; }
    optional!(
        description,
        notes,
        design,
        agent_context,
        acceptance_criteria,
        external_ref,
        due_at,
        owner,
        assignee,
        created_by,
        closed_at,
        close_reason,
        outcome,
        defer_until,
        workflow_run
    );
    // Seed::facts gives closed items the default outcome "done". Match
    // that projection while retaining the original absence/null in carry.
    if s.status == "closed" && s.outcome.is_none() {
        s.outcome = Some("done".into());
    }
    s.estimated_minutes = match v.get("estimated_minutes") {
        None | Some(Value::Null) => None,
        Some(x) => Some(
            x.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| SdError::usage("invalid estimated_minutes"))?,
        ),
    };
    for l in array(v, "labels")? {
        s.labels.insert(
            l.as_str()
                .ok_or_else(|| SdError::usage("label must be a string"))?
                .into(),
        );
    }
    for d in array(v, "dependencies")? {
        let target = required(d, "depends_on_id")?;
        let kind = required(d, "type")?;
        match kind {
            "blocks" | "related" | "discovered-from" => {
                s.add_dep(target, kind);
            }
            "relates-to" => {
                s.add_dep(target, "related");
            }
            "parent-child" => {
                if s.parent.as_deref().is_some_and(|p| p != target) {
                    return Err(SdError::usage(format!("{}: multiple parents", s.id)));
                }
                s.add_dep(target, kind);
            }
            _ => {} // Unknown edge types remain verbatim in the JSON fact.
        }
    }
    if let Some(extension) = v.get("_seeds").filter(|e| e["format"] == "seeds-facts-v1") {
        s.revision = extension["revision"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= i64::MAX as u64)
            .ok_or_else(|| SdError::usage("invalid seeds revision"))?;
        s.extra = extra_facts(&extension["facts"])?;
    }
    Ok(s)
}
fn extra_facts(v: &Value) -> Result<BTreeSet<(String, Obj)>> {
    let facts: BTreeSet<(String, Obj)> = serde_json::from_value(v.clone())
        .map_err(|e| SdError::usage(format!("invalid carried facts: {e}")))?;
    let modeled = Seed::modelled_predicates();
    if facts
        .iter()
        .any(|(p, _)| modeled.contains(p) || *p == shadow())
    {
        return Err(SdError::usage(
            "carried facts cannot override modeled fields",
        ));
    }
    Ok(facts)
}

/// Project every record to WorkItem facts, carrying the complete input too.
pub fn decode(records: &Records) -> Result<Snapshot> {
    let mut snap = Snapshot::default();
    for (id, v) in records {
        let mut s = seed(v)?;
        if s.id != *id {
            return Err(SdError::usage("record key and id disagree"));
        }
        s.extra.insert((shadow(), Obj::Str(v.to_string())));
        let comment_start = snap.comments.len();
        for (i, c) in array(v, "comments")?.iter().enumerate() {
            snap.comments.push(Comment {
                seed: id.clone(),
                index: c
                    .get("_seeds")
                    .filter(|e| e["format"] == "seeds-facts-v1")
                    .and_then(|e| e["index"].as_u64())
                    .unwrap_or(i as u64 + 1),
                author: required(c, "author")?.into(),
                text: required(c, "text")?.into(),
                created_at: required(c, "created_at")?.into(),
                extra: match c.get("_seeds").filter(|e| e["format"] == "seeds-facts-v1") {
                    Some(e) => extra_facts(&e["facts"])?,
                    None => BTreeSet::new(),
                },
            });
        }
        let indexes: Vec<_> = snap.comments[comment_start..]
            .iter()
            .map(|c| c.index)
            .collect();
        if indexes.iter().any(|i| *i == 0 || *i > i64::MAX as u64)
            || indexes.iter().collect::<BTreeSet<_>>().len() != indexes.len()
        {
            return Err(SdError::usage(format!(
                "{id}: invalid or duplicate comment indexes"
            )));
        }
        snap.seeds.insert(id.clone(), s);
    }
    Ok(snap)
}
fn raw(s: &Seed) -> Result<Option<Value>> {
    let values: Vec<_> = s.extra.iter().filter(|(p, _)| *p == shadow()).collect();
    match values.as_slice() {
        [] => Ok(None),
        [(_, Obj::Str(v))] => serde_json::from_str(v)
            .map(Some)
            .map_err(|e| SdError::failed(format!("corrupt beads carry: {e}"))),
        _ => Err(SdError::conflict(format!(
            "{}: multiple beadsJson values",
            s.id
        ))),
    }
}
const FIELDS: &[&str] = &[
    "id",
    "title",
    "description",
    "notes",
    "design",
    "agent_context",
    "acceptance_criteria",
    "external_ref",
    "due_at",
    "estimated_minutes",
    "owner",
    "status",
    "priority",
    "issue_type",
    "assignee",
    "labels",
    "created_at",
    "created_by",
    "updated_at",
    "closed_at",
    "close_reason",
    "outcome",
    "defer_until",
    "workflow_run",
];

/// Export original records with current modeled values overlaid only when changed.
pub fn encode(snap: &Snapshot) -> Result<Records> {
    let mut out = Records::new();
    let mut comments: BTreeMap<&str, Vec<&Comment>> = BTreeMap::new();
    for c in &snap.comments {
        comments.entry(&c.seed).or_default().push(c);
    }
    for (id, s) in &snap.seeds {
        let original = raw(s)?;
        let baseline = original.as_ref().map(seed).transpose()?;
        let mut v = original.clone().unwrap_or_else(|| json!({}));
        let native_extra: BTreeSet<_> = s
            .extra
            .iter()
            .filter(|(p, _)| *p != shadow())
            .cloned()
            .collect();
        if original.is_none()
            || !native_extra.is_empty()
            || v.get("_seeds")
                .is_some_and(|e| e["format"] == "seeds-facts-v1")
        {
            if v.get("_seeds")
                .is_some_and(|e| e["format"] != "seeds-facts-v1")
            {
                return Err(SdError::conflict(format!(
                    "{id}: _seeds is occupied by foreign data"
                )));
            }
            v["_seeds"] =
                json!({"format":"seeds-facts-v1","revision":s.revision,"facts":native_extra});
        }
        let current = s.to_json();
        let before = baseline.as_ref().map(Seed::to_json);
        for k in FIELDS {
            if before.as_ref().is_none_or(|b| b[*k] != current[*k]) {
                v[*k] = current[*k].clone();
            }
        }
        let old_deps = original
            .as_ref()
            .map(|v| array(v, "dependencies"))
            .transpose()?
            .unwrap_or(&[]);
        let mut deps = Vec::new();
        let current_deps = s.dependencies();
        for d in old_deps {
            let kind = required(d, "type")?;
            let target = required(d, "depends_on_id")?;
            let normalized = if kind == "relates-to" {
                "related"
            } else {
                kind
            };
            if !crate::model::DEP_TYPES.contains(&normalized) || s.has_dep(target, normalized) {
                deps.push(d.clone());
            }
        }
        for (target, kind) in current_deps {
            if !deps.iter().any(|d| {
                d["depends_on_id"] == target
                    && (d["type"] == kind || (kind == "related" && d["type"] == "relates-to"))
            }) {
                deps.push(json!({"issue_id":id,"depends_on_id":target,"type":kind,"created_at":s.updated_at,"created_by":s.created_by.clone().unwrap_or_default(),"metadata":"{}","thread_id":""}));
            }
        }
        if deps != old_deps || original.is_none() {
            v["dependencies"] = json!(deps);
        }
        let old_comments = original
            .as_ref()
            .map(|v| array(v, "comments"))
            .transpose()?
            .unwrap_or(&[]);
        let mut cs = Vec::new();
        for c in comments.get(id.as_str()).into_iter().flatten() {
            let mut val = old_comments
                .iter()
                .enumerate()
                .find(|(i, v)| {
                    v.get("_seeds")
                        .filter(|e| e["format"] == "seeds-facts-v1")
                        .and_then(|e| e["index"].as_u64())
                        .unwrap_or(*i as u64 + 1)
                        == c.index
                })
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| json!({"id":c.index,"issue_id":id}));
            if !c.extra.is_empty()
                || original.is_none()
                || val
                    .get("_seeds")
                    .is_some_and(|e| e["format"] == "seeds-facts-v1")
            {
                if val
                    .get("_seeds")
                    .is_some_and(|e| e["format"] != "seeds-facts-v1")
                {
                    return Err(SdError::conflict(format!(
                        "{id}: comment _seeds is occupied"
                    )));
                }
                val["_seeds"] = json!({"format":"seeds-facts-v1","facts":c.extra,"index":c.index});
            }
            val["author"] = json!(c.author);
            val["text"] = json!(c.text);
            val["created_at"] = json!(c.created_at);
            cs.push(val);
        }
        if cs != old_comments || original.is_none() {
            v["comments"] = json!(cs);
        }
        out.insert(id.clone(), v);
    }
    Ok(out)
}

/// Deterministic JSONL, one exact ID per line.
pub fn render(records: &Records) -> String {
    records.values().map(|v| format!("{v}\n")).collect()
}

/// Field-level semantic differences (including absent versus null).
pub fn diff(a: &Records, b: &Records) -> Vec<Value> {
    let mut out = Vec::new();
    for id in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
        match (a.get(id), b.get(id)) {
            (Some(x), Some(y)) => walk(id, "", Some(x), Some(y), &mut out),
            (x, y) => out.push(json!({"id":id,"field":"$record","before":x,"after":y})),
        }
    }
    out
}
fn walk(id: &str, path: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<Value>) {
    if a == b {
        return;
    }
    if let (Some(Value::Object(x)), Some(Value::Object(y))) = (a, b) {
        for k in x.keys().chain(y.keys()).collect::<BTreeSet<_>>() {
            walk(id, &format!("{path}/{k}"), x.get(k), y.get(k), out);
        }
    } else if let (Some(Value::Array(x)), Some(Value::Array(y))) = (a, b) {
        for i in 0..x.len().max(y.len()) {
            walk(id, &format!("{path}/{i}"), x.get(i), y.get(i), out);
        }
    } else {
        out.push(json!({"id":id,"field":path,"before_present":a.is_some(),"after_present":b.is_some(),"before":a,"after":b}));
    }
}

// Compare UTC instants at mixed fractional precision without mistaking the
// lexical suffix `Z` for a fraction greater than every decimal digit.
fn utc_key(v: &Value) -> Option<(String, String)> {
    let s = v.as_str()?.strip_suffix('Z')?;
    if s.len() < 19 || !s.is_ascii() {
        return None;
    }
    let head = &s[..19];
    if !head.bytes().enumerate().all(|(i, c)| match i {
        4 | 7 => c == b'-',
        10 => c == b'T',
        13 | 16 => c == b':',
        _ => c.is_ascii_digit(),
    }) {
        return None;
    }
    let fraction = if s.len() == 19 {
        ""
    } else {
        s[19..].strip_prefix('.')?
    };
    if fraction.len() > 9 || !fraction.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((head.into(), format!("{fraction:0<9}")))
}

/// Three-way JSON merge. Scalar disagreements refuse the entire operation;
/// labels, dependencies and comments union, retaining nested metadata.
pub fn merge(base: &Records, local: &Records, peer: &Records) -> Result<Records> {
    let mut out = Records::new();
    let mut conflicts = Vec::new();
    for id in base
        .keys()
        .chain(local.keys())
        .chain(peer.keys())
        .collect::<BTreeSet<_>>()
    {
        let (b, l, r) = (base.get(id), local.get(id), peer.get(id));
        let value = if l == r {
            l.cloned()
        } else if l == b {
            r.cloned()
        } else if r == b {
            l.cloned()
        } else if let (Some(l), Some(r)) = (l, r) {
            let mut v = l.clone();
            for k in l
                .as_object()
                .unwrap()
                .keys()
                .chain(r.as_object().unwrap().keys())
                .chain(
                    b.and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|m| m.keys()),
                )
                .collect::<BTreeSet<_>>()
            {
                let (bv, lv, rv) = (b.and_then(|v| v.get(k)), l.get(k), r.get(k));
                let merged = if lv == rv {
                    lv.cloned()
                } else if lv == bv {
                    rv.cloned()
                } else if rv == bv {
                    lv.cloned()
                } else if k == "updated_at" {
                    match (lv.and_then(utc_key), rv.and_then(utc_key)) {
                        (Some(lk), Some(rk)) => Some(if lk > rk {
                            lv.unwrap().clone()
                        } else {
                            rv.unwrap().clone()
                        }),
                        _ => {
                            conflicts.push(format!("{id}/{k}: non-UTC timestamp"));
                            None
                        }
                    }
                } else if ["labels", "dependencies", "comments"].contains(&k.as_str()) {
                    match (lv.and_then(Value::as_array), rv.and_then(Value::as_array)) {
                        (Some(a), Some(b)) => {
                            let mut u = a.clone();
                            for x in b {
                                if !u.contains(x) {
                                    u.push(x.clone());
                                }
                            }
                            Some(json!(u))
                        }
                        _ => {
                            conflicts.push(format!("{id}/{k}"));
                            None
                        }
                    }
                } else {
                    conflicts.push(format!("{id}/{k}"));
                    None
                };
                if let Some(x) = merged {
                    v[k] = x;
                } else {
                    v.as_object_mut().unwrap().remove(k);
                }
            }
            Some(v)
        } else {
            conflicts.push(format!("{id}/$record"));
            None
        };
        if let Some(v) = value {
            out.insert(id.clone(), v);
        }
    }
    if !conflicts.is_empty() {
        return Err(SdError::conflict(format!(
            "beads sync conflicts; nothing written: {}",
            conflicts.join(", ")
        )));
    }
    Ok(out)
}

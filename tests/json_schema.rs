//! `--json` is a contract: these tests pin the exact key set of every verb's
//! output, so a renamed or dropped key fails here rather than in a caller.
#![cfg(feature = "native")]
// These drive the native `sd` binary, so they use the process and the
// filesystem the core is barred from (clippy.toml).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

// THE GOLDEN LIST. Literal on purpose: it is the independent witness. If a
// key were removed from src/schema.rs AND from the output together, a pin
// derived from schema.rs would still pass; this list would not (sattler,
// seeds#35 review). schema.rs must equal it, and output must match both.
const SEED_KEYS: &[&str] = &[
    "assignee",
    "close_reason",
    "closed_at",
    "created_at",
    "created_by",
    "defer_until",
    "dependency_count",
    "description",
    "id",
    "issue_type",
    "labels",
    "notes",
    "owner",
    "outcome",
    "parent",
    "priority",
    "revision",
    "status",
    "title",
    "updated_at",
    "workflow_run",
];
const EDGE_KEYS: &[&str] = &["dependency_type", "id", "priority", "status", "title"];
const COMMENT_KEYS: &[&str] = &["author", "created_at", "id", "issue_id", "text"];

#[test]
fn the_published_schema_tables_equal_the_golden_key_lists() {
    fn sorted(k: &[&'static str]) -> Vec<&'static str> {
        let mut v = k.to_vec();
        v.sort_unstable();
        v
    }
    assert_eq!(seeds::schema::keys(seeds::schema::SEED), sorted(SEED_KEYS));
    assert_eq!(seeds::schema::keys(seeds::schema::EDGE), sorted(EDGE_KEYS));
    assert_eq!(
        seeds::schema::keys(seeds::schema::COMMENT),
        sorted(COMMENT_KEYS)
    );
}

struct Store {
    dir: PathBuf,
}

impl Store {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("seeds-json-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn json(&self, args: &[&str]) -> Value {
        let o = Command::new(env!("CARGO_BIN_EXE_sd"))
            .args(args)
            .arg("--json")
            .current_dir(&self.dir)
            .env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join("cfg"))
            .env("SEEDS_ACTOR", "tester")
            .env_remove("SEEDS_QUIPU_STORE")
            .env_remove("SEEDS_QUIPU_URL")
            .env_remove("SEEDS_GRAPH")
            .env_remove("SEEDS_PREFIX")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "sd {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }

    /// Run without requiring success (for error-envelope checks).
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_sd"))
            .args(args)
            .current_dir(&self.dir)
            .env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join("cfg"))
            .env("SEEDS_ACTOR", "tester")
            .env_remove("SEEDS_QUIPU_STORE")
            .env_remove("SEEDS_QUIPU_URL")
            .env_remove("SEEDS_GRAPH")
            .env_remove("SEEDS_PREFIX")
            .output()
            .unwrap()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn set(k: &[&str]) -> BTreeSet<String> {
    k.iter().map(|s| s.to_string()).collect()
}

fn with(base: &[&str], extra: &[&str]) -> BTreeSet<String> {
    let mut s = set(base);
    s.extend(extra.iter().map(|x| x.to_string()));
    s
}

#[test]
fn every_verb_prints_its_documented_keys() {
    let st = Store::new("keys");

    let a = st.json(&["create", "a", "-l", "x", "-d", "desc"]);
    assert_eq!(keys(&a), with(SEED_KEYS, &["tx"]), "create");
    let a = a["id"].as_str().unwrap().to_string();
    let b = st.json(&["create", "b"])["id"]
        .as_str()
        .unwrap()
        .to_string();

    let dep = st.json(&["dep", "add", &a, &b]);
    assert_eq!(
        keys(&dep),
        set(&[
            "action",
            "depends_on_id",
            "issue_id",
            "status",
            "tx",
            "type"
        ]),
        "dep add"
    );
    assert_eq!(dep["action"], "added");

    let dl = st.json(&["dep", "list", &a]);
    assert_eq!(
        keys(&dl[0]),
        set(&[
            "depends_on_id",
            "issue_id",
            "priority",
            "status",
            "title",
            "type"
        ]),
        "dep list"
    );

    let c = st.json(&["comments", "add", &a, "hello"]);
    assert_eq!(keys(&c), with(COMMENT_KEYS, &["tx"]), "comments add");
    let cl = st.json(&["comments", "list", &a]);
    assert_eq!(keys(&cl[0]), set(COMMENT_KEYS), "comments list");

    let show = st.json(&["show", &a, &b]);
    assert_eq!(show.as_array().unwrap().len(), 2);
    assert_eq!(
        keys(&show[0]),
        with(SEED_KEYS, &["comments", "dependencies", "dependents"]),
        "show"
    );
    assert_eq!(keys(&show[0]["dependencies"][0]), set(EDGE_KEYS));
    assert_eq!(keys(&show[1]["dependents"][0]), set(EDGE_KEYS));
    assert_eq!(keys(&show[0]["comments"][0]), set(COMMENT_KEYS));

    let list = st.json(&["list"]);
    assert_eq!(
        keys(&list),
        set(&["has_more", "issues", "limit", "offset", "total"]),
        "list"
    );
    assert_eq!(
        keys(&list["issues"][0]),
        with(SEED_KEYS, &["dependent_count"]),
        "list issues carry br's dependent_count"
    );

    let ready = st.json(&["ready"]);
    assert!(ready.is_array(), "ready is a bare array, as in br");
    assert_eq!(keys(&ready[0]), set(SEED_KEYS), "ready");

    assert_eq!(keys(&st.json(&["count"])), set(&["count"]), "count");
    let by = st.json(&["count", "--by", "status"]);
    assert_eq!(keys(&by), set(&["groups", "total"]), "count --by");
    assert_eq!(keys(&by["groups"][0]), set(&["count", "group"]));

    let up = st.json(&["update", &a, "--add-label", "y"]);
    assert!(up.is_array());
    assert_eq!(keys(&up[0]), with(SEED_KEYS, &["tx"]), "update");

    let closed = st.json(&["close", &b, "--reason", "done"]);
    assert!(closed.is_array());
    assert_eq!(keys(&closed[0]), with(SEED_KEYS, &["tx"]), "close");
    assert_eq!(closed[0]["status"], "closed");

    let rm = st.json(&["dep", "remove", &a, &b]);
    assert_eq!(rm["action"], "removed");
}

#[test]
fn value_types_are_stable() {
    let st = Store::new("types");
    let s = st.json(&["create", "typed", "-p", "P3"]);
    assert!(s["priority"].is_u64());
    assert!(s["revision"].is_u64());
    assert!(s["labels"].is_array());
    assert!(
        s["assignee"].is_null(),
        "absent values are null, never missing"
    );
    assert!(s["created_at"].as_str().unwrap().ends_with('Z'));
}

/// A small JSON Schema check: type (incl. unions), enum, required,
/// additionalProperties false, items. Enough for sd's closed-world schemas.
fn conforms(v: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let ty = |t: &str| match t {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "null" => v.is_null(),
        _ => false,
    };
    match &schema["type"] {
        Value::String(t) if !ty(t) => return Err(format!("{path}: not {t}: {v}")),
        Value::Array(ts) if !ts.iter().any(|t| ty(t.as_str().unwrap_or(""))) => {
            return Err(format!("{path}: not any of {ts:?}: {v}"))
        }
        _ => {}
    }
    if let Some(e) = schema["enum"].as_array() {
        if !e.contains(v) {
            return Err(format!("{path}: {v} not in enum"));
        }
    }
    if let (Some(o), Some(props)) = (v.as_object(), schema["properties"].as_object()) {
        for r in schema["required"].as_array().into_iter().flatten() {
            let r = r.as_str().unwrap();
            if !o.contains_key(r) {
                return Err(format!("{path}: missing {r}"));
            }
        }
        for (k, val) in o {
            match props.get(k) {
                Some(s) => conforms(val, s, &format!("{path}.{k}"))?,
                None if schema["additionalProperties"] == Value::Bool(false) => {
                    return Err(format!("{path}: unexpected key {k}"))
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(sch)) = (v.as_array(), schema.get("items")) {
        for (i, it) in items.iter().enumerate() {
            conforms(it, sch, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

#[test]
fn real_output_conforms_to_the_published_schemas() {
    let st = Store::new("schema");
    let a = st.json(&["create", "a", "-d", "desc", "-l", "x"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = st.json(&["create", "b"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    st.json(&["dep", "add", &b, &a]);
    st.json(&["comments", "add", &a, "hello"]);
    let schema = |t: &str| st.json(&["schema", t]);
    let strip_tx = |mut v: Value| {
        // Writes add tx on top of the seed object (documented; not a seed field).
        if let Some(o) = v.as_object_mut() {
            o.remove("tx");
        }
        v
    };
    let check = |v: &Value, t: &str| conforms(v, &schema(t), t).unwrap();
    for row in st.json(&["show", &a, &b]).as_array().unwrap() {
        check(row, "issue-details");
    }
    for row in st.json(&["list"])["issues"].as_array().unwrap() {
        check(row, "issue-with-counts");
    }
    for row in st.json(&["blocked"])["issues"].as_array().unwrap() {
        check(row, "blocked-issue");
    }
    for row in st.json(&["ready"]).as_array().unwrap() {
        check(row, "ready-issue");
    }
    check(
        &st.json(&["stats", "--by-type", "--by-label"]),
        "statistics",
    );
    check(
        &strip_tx(st.json(&["comments", "add", &a, "again"])),
        "comment",
    );
    let err: Value =
        serde_json::from_slice(&st.run(&["show", "sd-nope", "--json"]).stdout).unwrap();
    check(&err, "error");
    // CONTROL: the validator can fail. A seed with an extra key must not pass.
    let mut bad = st.json(&["show", &a])[0].clone();
    bad["surprise"] = Value::Bool(true);
    assert!(conforms(&bad, &schema("issue-details"), "bad").is_err());
}

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
    assert_eq!(keys(&a), set(SEED_KEYS), "create");
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
    assert_eq!(keys(&c), set(COMMENT_KEYS), "comments add");
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
    assert_eq!(keys(&list["issues"][0]), set(SEED_KEYS));

    let ready = st.json(&["ready"]);
    assert!(ready.is_array(), "ready is a bare array, as in br");
    assert_eq!(keys(&ready[0]), set(SEED_KEYS), "ready");

    assert_eq!(keys(&st.json(&["count"])), set(&["count"]), "count");
    let by = st.json(&["count", "--by", "status"]);
    assert_eq!(keys(&by), set(&["groups", "total"]), "count --by");
    assert_eq!(keys(&by["groups"][0]), set(&["count", "group"]));

    let up = st.json(&["update", &a, "--add-label", "y"]);
    assert!(up.is_array());
    assert_eq!(keys(&up[0]), set(SEED_KEYS), "update");

    let closed = st.json(&["close", &b, "--reason", "done"]);
    assert!(closed.is_array());
    assert_eq!(keys(&closed[0]), set(SEED_KEYS), "close");
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

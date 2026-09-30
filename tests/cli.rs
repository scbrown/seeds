//! End-to-end: the built `sd` binary, its configuration, its store file, and
//! concurrent processes writing one store.
#![cfg(feature = "native")]
// These drive the native `sd` binary, so they use the process and the
// filesystem the core is barred from (clippy.toml).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("seeds-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.config")).unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        Self { root }
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    fn cmd(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_sd"));
        c.args(args)
            .current_dir(cwd)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("SEEDS_ACTOR", "tester")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in [
            "SEEDS_QUIPU_STORE",
            "SEEDS_QUIPU_URL",
            "SEEDS_GRAPH",
            "SEEDS_PREFIX",
        ] {
            c.env_remove(k);
        }
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(&self.work(), args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "sd {args:?} failed ({:?}): {}",
            o.status.code(),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&self.ok(&a)).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ids(v: &Value) -> Vec<String> {
    let arr = v.as_array().or_else(|| v["issues"].as_array()).unwrap();
    arr.iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

#[test]
fn works_out_of_the_box_with_no_config_at_all() {
    let sb = Sandbox::new("oob");
    // Before anything is written: an empty answer that SAYS why it is empty.
    let o = sb.run(&["ready", "--json"]);
    assert_eq!(code(&o), 0);
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "[]");
    assert!(String::from_utf8_lossy(&o.stderr).contains("created on first write"));

    let id = sb
        .ok(&["create", "first seed", "--silent"])
        .trim()
        .to_string();
    assert!(id.starts_with("sd-"), "{id}");
    assert!(sb.work().join(".seeds/seeds.db").is_file());
    assert_eq!(ids(&sb.json(&["ready"])), vec![id]);
}

#[test]
fn the_transcript_create_dep_ready_close_ready() {
    let sb = Sandbox::new("transcript");
    let a = sb
        .ok(&["create", "Write the parser", "-p", "1", "--silent"])
        .trim()
        .to_string();
    let b = sb
        .ok(&["create", "Design the grammar", "--silent"])
        .trim()
        .to_string();
    sb.ok(&["dep", "add", &a, &b]);
    assert_eq!(ids(&sb.json(&["ready"])), vec![b.clone()]);
    sb.ok(&["close", &b, "--reason", "grammar written"]);
    assert_eq!(ids(&sb.json(&["ready"])), vec![a.clone()]);
    let shown = sb.json(&["show", &a]);
    assert_eq!(shown[0]["dependencies"][0]["id"], b.as_str());
    assert_eq!(shown[0]["dependencies"][0]["status"], "closed");
}

#[test]
fn a_blocker_that_has_its_own_blocker_can_be_added() {
    // c -> b -> a. Adding the c -> b edge used to be refused by the shapes:
    // b was validated with its own blockedOn edge to a, and a was not in the
    // validated graph.
    let sb = Sandbox::new("chain");
    let a = sb.ok(&["create", "a", "--silent"]).trim().to_string();
    let b = sb
        .ok(&["create", "b", "--deps", &a, "--silent"])
        .trim()
        .to_string();
    let c = sb
        .ok(&["create", "c", "--deps", &b, "--silent"])
        .trim()
        .to_string();
    let d = sb.ok(&["create", "d", "--silent"]).trim().to_string();
    sb.ok(&["dep", "add", &d, &c]);
    assert_eq!(ids(&sb.json(&["ready"])), vec![a.clone()]);
    sb.ok(&["close", &a, "--reason", "done"]);
    assert_eq!(ids(&sb.json(&["ready"])), vec![b]);
    // The edges that were not written still hold: a dangling blocker is refused.
    let o = sb.run(&["create", "e", "--deps", "sd-nope"]);
    assert!(!o.status.success());
}

#[test]
fn a_workflow_step_creates_its_seed_once() {
    let sb = Sandbox::new("keyed");
    let args = [
        "create",
        "triage the report",
        "--workflow-run",
        "r1",
        "--step",
        "triage",
        "--silent",
    ];
    let a = sb.ok(&args).trim().to_string();
    assert!(a.starts_with("sd-w"), "{a}");
    // The retry names the same seed and writes nothing new.
    let again = sb.run(&[
        "create",
        "triage the report",
        "--workflow-run",
        "r1",
        "--step",
        "triage",
    ]);
    assert!(again.status.success());
    assert!(String::from_utf8_lossy(&again.stdout).contains("exists"));
    assert_eq!(sb.ok(&args).trim(), a);
    assert_eq!(ids(&sb.json(&["list"])), vec![a.clone()]);
    // The retry wrote nothing, so it names no transaction.
    let j = sb.json(&[
        "create",
        "triage the report",
        "--workflow-run",
        "r1",
        "--step",
        "triage",
    ]);
    assert_eq!(j["id"], a.as_str());
    assert_eq!(j["tx"], Value::Null);
    // A second visit to the same step is a new seed.
    let b = sb
        .ok(&[
            "create",
            "triage again",
            "--workflow-run",
            "r1",
            "--step",
            "triage",
            "--visit",
            "2",
            "--silent",
        ])
        .trim()
        .to_string();
    assert_ne!(a, b);
    // The run is recorded on the seed.
    assert_eq!(
        sb.json(&["show", &a])[0]["workflow_run"],
        "urn:shuttle:run:r1"
    );
    // Misuse is a usage error (exit 2), never a silent random id.
    for bad in [
        vec!["create", "x", "--step", "triage"],
        vec!["create", "x", "--workflow-run", "r1", "--visit", "2"],
        vec![
            "create",
            "x",
            "--workflow-run",
            "r1",
            "--step",
            "s",
            "--visit",
            "0",
        ],
    ] {
        assert_eq!(sb.run(&bad).status.code(), Some(2), "{bad:?}");
    }
}

#[test]
fn concurrent_creates_of_one_workflow_step_make_one_seed() {
    let sb = Sandbox::new("keyed-race");
    sb.ok(&["create", "warm the store", "--silent"]);
    const N: usize = 8;
    let children: Vec<_> = (0..N)
        .map(|_| {
            sb.cmd(
                &sb.work(),
                &[
                    "create",
                    "raced",
                    "--workflow-run",
                    "r9",
                    "--step",
                    "s",
                    "--silent",
                ],
            )
            .spawn()
            .unwrap()
        })
        .collect();
    let mut got = std::collections::BTreeSet::new();
    for c in children {
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        got.insert(String::from_utf8_lossy(&o.stdout).trim().to_string());
    }
    assert_eq!(got.len(), 1, "every caller must get the same seed: {got:?}");
    let listed = sb.json(&["list"]);
    let raced = listed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["title"] == "raced")
        .count();
    assert_eq!(raced, 1, "exactly one seed for (run, step, visit)");
}

#[test]
fn close_records_how_a_seed_ended() {
    let sb = Sandbox::new("outcome");
    let a = sb.ok(&["create", "a", "--silent"]).trim().to_string();
    let b = sb.ok(&["create", "b", "--silent"]).trim().to_string();
    assert_eq!(
        sb.json(&["show", &a])[0]["outcome"],
        Value::Null,
        "open: no outcome"
    );
    sb.ok(&["close", &a, "--reason", "shipped"]);
    assert_eq!(sb.json(&["show", &a])[0]["outcome"], "done", "the default");
    sb.ok(&[
        "close",
        &b,
        "--reason",
        "not needed",
        "--outcome",
        "abandoned",
    ]);
    assert_eq!(sb.json(&["show", &b])[0]["outcome"], "abandoned");
    // An unknown outcome is a usage error and writes nothing.
    let c = sb.ok(&["create", "c", "--silent"]).trim().to_string();
    let bad = sb.run(&["close", &c, "--outcome", "wontdo"]);
    assert_eq!(bad.status.code(), Some(2));
    assert_eq!(sb.json(&["show", &c])[0]["status"], "open");
    // Reopening clears it; closing again without --outcome is done again.
    sb.ok(&["update", &b, "--status", "open"]);
    assert_eq!(sb.json(&["show", &b])[0]["outcome"], Value::Null);
    sb.ok(&["close", &b, "--reason", "after all"]);
    assert_eq!(sb.json(&["show", &b])[0]["outcome"], "done");
}

#[test]
fn concurrent_label_writes_from_separate_processes_lose_nothing() {
    let sb = Sandbox::new("concurrent-labels");
    let id = sb
        .ok(&["create", "contended", "--silent"])
        .trim()
        .to_string();
    const N: usize = 8;
    let children: Vec<_> = (0..N)
        .map(|i| {
            sb.cmd(
                &sb.work(),
                &["update", &id, "--add-label", &format!("l{i}")],
            )
            .spawn()
            .unwrap()
        })
        .collect();
    for c in children {
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success());
    }
    let s = &sb.json(&["show", &id])[0];
    let labels: Vec<&str> = s["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(labels.len(), N, "every concurrent write landed: {labels:?}");
    assert_eq!(s["revision"], 1 + N as u64);
}

#[test]
fn simultaneous_claims_on_a_fresh_seed_have_exactly_one_winner() {
    let sb = Sandbox::new("claim-race");
    let id = sb.ok(&["create", "race", "--silent"]).trim().to_string();
    let procs: Vec<_> = (0..6)
        .map(|i| {
            sb.cmd(
                &sb.work(),
                &["update", &id, "--claim", "--actor", &format!("racer{i}")],
            )
            .spawn()
            .unwrap()
        })
        .collect();
    let codes: Vec<i32> = procs
        .into_iter()
        .map(|p| code(&p.wait_with_output().unwrap()))
        .collect();
    assert_eq!(codes.iter().filter(|c| **c == 0).count(), 1, "{codes:?}");
    assert_eq!(codes.iter().filter(|c| **c == 4).count(), 5, "{codes:?}");
    let winner = sb.json(&["show", &id])[0]["assignee"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(winner.starts_with("racer"));
    // And a latecomer is refused too.
    assert_eq!(
        code(&sb.run(&["update", &id, "--claim", "--actor", "late"])),
        4
    );
    assert_eq!(sb.json(&["show", &id])[0]["assignee"], winner.as_str());
}

#[test]
fn project_toml_beats_user_toml_and_env_beats_both() {
    let sb = Sandbox::new("config-precedence");
    std::fs::create_dir_all(sb.root.join("home/.config/seeds")).unwrap();
    std::fs::write(
        sb.root.join("home/.config/seeds/config.toml"),
        "[quipu]\nstore = \"user.db\"\n[project]\nprefix = \"usr\"\n",
    )
    .unwrap();
    // Only the user file: it decides.
    let id = sb.ok(&["create", "u", "--silent"]).trim().to_string();
    assert!(id.starts_with("usr-"), "{id}");
    assert!(sb.root.join("home/.config/seeds/user.db").is_file());

    // A project file (found by walking up from a subdirectory) beats it.
    std::fs::create_dir_all(sb.work().join(".seeds")).unwrap();
    std::fs::write(
        sb.work().join(".seeds/config.toml"),
        "[quipu]\nstore = \"proj.db\"\n[project]\nprefix = \"prj\"\n",
    )
    .unwrap();
    let sub = sb.work().join("deep/er");
    std::fs::create_dir_all(&sub).unwrap();
    let o = sb.cmd(&sub, &["create", "p", "--silent"]).output().unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("prj-"));
    assert!(sb.work().join("proj.db").is_file());

    // The environment beats both files.
    let env_db = sb.root.join("env.db");
    let o = sb
        .cmd(&sub, &["create", "e", "--silent"])
        .env("SEEDS_QUIPU_STORE", &env_db)
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(env_db.is_file());
}

#[test]
fn a_config_that_sets_both_store_and_url_is_refused() {
    let sb = Sandbox::new("config-both");
    std::fs::create_dir_all(sb.work().join(".seeds")).unwrap();
    std::fs::write(
        sb.work().join(".seeds/config.toml"),
        "[quipu]\nstore = \"a.db\"\nurl = \"https://quipu.example.org\"\n",
    )
    .unwrap();
    let o = sb.run(&["ready", "--json"]);
    assert_eq!(code(&o), 6);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "CONFIG");

    let o = sb
        .cmd(&sb.root, &["ready"])
        .env("SEEDS_QUIPU_STORE", "a.db")
        .env("SEEDS_QUIPU_URL", "https://quipu.example.org")
        .output()
        .unwrap();
    assert_eq!(code(&o), 6);
}

#[test]
fn an_unreachable_url_is_an_error_and_never_falls_back_to_local() {
    let sb = Sandbox::new("unreachable");
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }; // listener dropped: the port is now closed
    let url = format!("http://127.0.0.1:{port}");
    let o = sb
        .cmd(&sb.work(), &["create", "must not land", "--json"])
        .env("SEEDS_QUIPU_URL", &url)
        .output()
        .unwrap();
    assert_eq!(code(&o), 7, "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stderr).contains("does not fall back"));
    // The project id may be created (it names the remote graph), but no
    // local store is: nothing fell back to local.
    assert!(
        !sb.work().join(".seeds/seeds.db").exists(),
        "no local store was created"
    );
}

#[test]
fn at_on_a_write_is_a_usage_error() {
    let sb = Sandbox::new("at-write");
    let id = sb.ok(&["create", "x", "--silent"]).trim().to_string();
    assert_eq!(code(&sb.run(&["close", &id, "--at", "1"])), 2);
}

#[test]
fn errors_carry_their_exit_codes_and_a_json_body() {
    let sb = Sandbox::new("errors");
    sb.ok(&["create", "x", "--silent"]);
    let o = sb.run(&["show", "sd-nope", "--json"]);
    assert_eq!(code(&o), 3);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "NOT_FOUND");
    assert_eq!(code(&sb.run(&["create", "x", "-p", "9"])), 2);
    assert_eq!(code(&sb.run(&["frobnicate"])), 2);
}

#[test]
fn close_without_a_reason_warns_but_closes() {
    let sb = Sandbox::new("close-warn");
    let id = sb.ok(&["create", "x", "--silent"]).trim().to_string();
    let o = sb.run(&["close", &id]);
    assert_eq!(code(&o), 0);
    assert!(String::from_utf8_lossy(&o.stderr).contains("--reason"));
}

#[test]
fn ready_says_when_limit_truncates_it() {
    let sb = Sandbox::new("ready-limit");
    for i in 0..3 {
        sb.ok(&["create", &format!("s{i}"), "--silent"]);
    }
    let o = sb.run(&["ready", "--limit", "2", "--json"]);
    assert_eq!(code(&o), 0);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert!(String::from_utf8_lossy(&o.stderr).contains("TRUNCATED to 2 of 3"));
    let o = sb.run(&["ready", "--json"]);
    assert!(!String::from_utf8_lossy(&o.stderr).contains("TRUNCATED"));
}

#[test]
fn comments_from_a_file_and_pins_through_the_cli() {
    let sb = Sandbox::new("comments-pin");
    // Text-mode create reports its transaction: "created <line> (tx N)".
    let out = sb.ok(&["create", "Ship the parser", "-p", "1"]);
    let id = out.split_whitespace().nth(2).unwrap().to_string();
    let tx1 = out
        .rsplit("(tx ")
        .next()
        .unwrap()
        .trim_end()
        .trim_end_matches(')')
        .to_string();
    assert!(sb
        .json(&["dep", "list", &id])
        .as_array()
        .unwrap()
        .is_empty());
    let f = sb.root.join("body.md");
    std::fs::write(&f, "a `backtick` and $(not run)\nsecond line").unwrap();
    sb.ok(&["comments", "add", &id, "--file", f.to_str().unwrap()]);
    let cs = sb.json(&["comments", "list", &id]);
    assert_eq!(cs[0]["text"], "a `backtick` and $(not run)\nsecond line");

    sb.ok(&[
        "update",
        &id,
        "--title",
        "Ship the streaming parser",
        "-p",
        "0",
    ]);
    let now = &sb.json(&["show", &id])[0];
    assert_eq!(now["title"], "Ship the streaming parser");
    let then = &sb.json(&["show", &id, "--at", &tx1])[0];
    assert_eq!(then["title"], "Ship the parser");
    assert_eq!(then["priority"], 1);
}

#[test]
fn a_seed_records_the_shuttle_run_that_drives_it() {
    let sb = Sandbox::new("workflow-run");
    let v = sb.json(&["create", "stamped", "--workflow-run", "release-42"]);
    assert_eq!(v["workflow_run"], "urn:shuttle:run:release-42");
    let id = v["id"].as_str().unwrap().to_string();
    let v = sb.json(&["update", &id, "--workflow-run", "urn:shuttle:run:other"]);
    assert_eq!(v[0]["workflow_run"], "urn:shuttle:run:other");
    let v = sb.json(&["update", &id, "--workflow-run", ""]);
    assert!(v[0]["workflow_run"].is_null());
}

#[test]
fn label_verbs_take_br_argument_shapes_and_emit_br_json() {
    let sb = Sandbox::new("label");
    let a = sb.json(&["create", "a"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = sb.json(&["create", "b"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Positional label (last argument) and -l both work, across several seeds.
    let added = sb.json(&["label", "add", &a, &b, "infra"]);
    assert_eq!(added[0]["status"], "added");
    assert_eq!(added[1]["issue_id"], b.as_str());
    assert!(added[0]["tx"].as_u64().unwrap() > 0);
    assert_eq!(
        sb.json(&["label", "add", &a, "-l", "infra"])[0]["status"],
        "exists"
    );
    assert_eq!(
        sb.json(&["label", "list", &a]),
        serde_json::json!(["infra"])
    );
    assert_eq!(
        sb.json(&["label", "list-all"]),
        serde_json::json!([{"label": "infra", "count": 2}])
    );
    let renamed = sb.json(&["label", "rename", "infra", "ops"]);
    assert_eq!(renamed["affected_issues"], 2);
    assert_eq!(
        sb.json(&["label", "remove", &b, "ops"])[0]["status"],
        "removed"
    );
    assert_eq!(sb.json(&["label", "list"]), serde_json::json!(["ops"]));
    // One positional and no -l is a usage error, as in br; nothing is written.
    let o = sb.run(&["label", "add", &a]);
    assert_eq!(code(&o), 2);
    let o = sb.run(&["label", "add", "sd-nope", "x"]);
    assert_eq!(code(&o), 3);
}

#[test]
fn every_write_reports_its_tx_and_at_reads_that_state_back() {
    // sd-non.2: the tx was only in the human line, so a script could not pin.
    let sb = Sandbox::new("write-tx");
    let created = sb.json(&["create", "first title"]);
    let id = created["id"].as_str().unwrap().to_string();
    let t0 = created["tx"].as_u64().unwrap();
    let up = sb.json(&["update", &id, "--title", "second title"]);
    let t1 = up[0]["tx"].as_u64().unwrap();
    assert!(t1 > t0);
    let c = sb.json(&["comments", "add", &id, "note"]);
    assert!(c["tx"].as_u64().unwrap() > t1);
    let closed = sb.json(&["close", &id, "--reason", "done"]);
    let t3 = closed[0]["tx"].as_u64().unwrap();
    let at = |tx: u64| sb.json(&["show", &id, "--at", &tx.to_string()])[0].clone();
    assert_eq!(at(t0)["title"], "first title");
    assert_eq!(at(t1)["title"], "second title");
    assert_eq!(at(t3)["status"], "closed");
    assert_eq!(at(t1)["status"], "open");
    // A dry run writes nothing, so it has no tx.
    assert!(sb.json(&["create", "x", "--dry-run"])["tx"].is_null());
}

#[test]
fn version_where_and_info_describe_the_ledger_without_creating_it() {
    let sb = Sandbox::new("about");
    let v = sb.json(&["version"]);
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        v["commit"].is_null(),
        "not embedded, so null rather than guessed"
    );
    assert_eq!(
        sb.ok(&["version", "--short"]).trim(),
        env!("CARGO_PKG_VERSION")
    );
    // where reads configuration only: no store and no project id appear.
    let w = sb.json(&["where"]);
    assert_eq!(w["mode"], "local");
    assert!(w["database_path"].as_str().unwrap().ends_with("seeds.db"));
    assert!(!sb.work().join(".seeds/project-id").exists());
    assert!(!sb.work().join(".seeds/seeds.db").exists());
    sb.json(&["create", "a"]);
    let i = sb.json(&["info"]);
    assert_eq!(i["issue_count"], 1);
    assert_eq!(i["mode"], "local");
    assert!(i["tx"].as_u64().unwrap() > 0);
    assert!(i["db_size"].as_u64().unwrap() > 0);
}

#[test]
fn completions_cover_every_verb_and_need_no_ledger() {
    let sb = Sandbox::new("completions");
    let bash = sb.ok(&["completions", "bash"]);
    for verb in ["create", "ready", "label", "comments", "completions"] {
        assert!(bash.contains(verb), "bash completions lack {verb}");
    }
    assert!(!sb.work().join(".seeds").exists(), "no ledger is created");
    let dir = sb.work().join("out");
    std::fs::create_dir_all(&dir).unwrap();
    sb.ok(&["completions", "zsh", "-o", dir.to_str().unwrap()]);
    assert!(std::fs::read_to_string(dir.join("_sd"))
        .unwrap()
        .contains("#compdef sd"));
    assert_eq!(code(&sb.run(&["completions", "tcsh"])), 2);
}

#[test]
fn init_makes_a_project_and_never_changes_its_id_or_prefix() {
    let sb = Sandbox::new("init");
    let first = sb.json(&["init", "--prefix", "ab"]);
    assert_eq!(first["prefix"], "ab");
    let id = first["project_id"].as_str().unwrap().to_string();
    let dir = sb.work().join(".seeds");
    assert!(dir.join("config.toml").exists() && dir.join(".gitignore").exists());
    // New seeds use the prefix and write to the project init named.
    let seed = sb.json(&["create", "x"]);
    assert!(seed["id"].as_str().unwrap().starts_with("ab-"));
    // A second init is refused and writes nothing.
    assert_eq!(code(&sb.run(&["init"])), 4);
    // --force restores a missing file but keeps the id...
    std::fs::remove_file(dir.join(".gitignore")).unwrap();
    let again = sb.json(&["init", "--force"]);
    assert_eq!(again["project_id"], id.as_str());
    assert_eq!(again["created"], serde_json::json!([".gitignore"]));
    // ...and refuses to change the prefix: that would move to another ledger.
    assert_eq!(code(&sb.run(&["init", "--force", "--prefix", "zz"])), 5);
    assert_eq!(
        std::fs::read_to_string(dir.join("project-id"))
            .unwrap()
            .trim(),
        id
    );
    assert_eq!(
        sb.json(&["show", seed["id"].as_str().unwrap()])[0]["title"],
        "x"
    );
}

#[test]
fn owner_is_set_changed_cleared_and_survives_a_pendant_round_trip() {
    let sb = Sandbox::new("owner");
    let a = sb
        .ok(&["create", "a", "--owner", "ada@example.org", "--silent"])
        .trim()
        .to_string();
    assert_eq!(sb.json(&["show", &a])[0]["owner"], "ada@example.org");
    // br's update output carries owner: it is a real field now, not a null key.
    let up = sb.json(&["update", &a, "--owner", "bo@example.org"]);
    assert_eq!(up[0]["owner"], "bo@example.org");
    // The owner travels in the pendant: export here, import into a fresh store.
    let dir = sb.root.join("pendant");
    sb.ok(&["export", "--to", dir.to_str().unwrap()]);
    let other = Sandbox::new("owner-import");
    other.ok(&["import", dir.to_str().unwrap()]);
    assert_eq!(other.json(&["show", &a])[0]["owner"], "bo@example.org");
    // "" clears it.
    sb.ok(&["update", &a, "--owner", ""]);
    assert_eq!(sb.json(&["show", &a])[0]["owner"], Value::Null);
}

#[test]
fn q_captures_status_aliases_stats_and_mapped_verbs_point_elsewhere() {
    let sb = Sandbox::new("q-mapped");
    // q: title words joined, prints only the id.
    let id = sb
        .ok(&["q", "quick", "one", "-p", "1", "-l", "a,b"])
        .trim()
        .to_string();
    let shown = sb.json(&["show", &id]);
    assert_eq!(shown[0]["title"], "quick one");
    assert_eq!(shown[0]["priority"], 1);
    let q = sb.json(&["q", "second"]);
    assert!(q["id"].is_string() && q["tx"].as_u64().unwrap() > 0);
    // status is stats.
    assert_eq!(sb.json(&["status"]), sb.json(&["stats"]));
    // A mapped verb exits 21 with the pointer on stderr, whatever its args,
    // and touches nothing.
    for (verb, needle) in [
        ("query", "quipu"),
        ("upgrade", "caboodle"),
        ("gate", "shuttle"),
    ] {
        let o = sb.run(&[verb, "anything", "--flag"]);
        assert_eq!(code(&o), 21, "{verb}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(needle),
            "{verb}"
        );
    }
    let j: Value = serde_json::from_slice(&sb.run(&["audit", "--json"]).stdout).unwrap();
    assert_eq!(j["error"]["code"], "ELSEWHERE");
    assert_eq!(sb.json(&["count"])["count"], 2);
}

#[test]
fn a_mapped_verb_honours_a_trailing_json_flag() {
    let sb = Sandbox::new("mapped-json");
    let o = sb.run(&["query", "list", "--json"]);
    assert_eq!(code(&o), 21);
    let j: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(j["error"]["code"], "ELSEWHERE");
}

#[test]
fn history_names_its_meaning_in_text_and_json() {
    // wu's triage ruling: br's `history` manages backup files; sd's must say
    // that it means something else.
    let sb = Sandbox::new("history");
    let id = sb.ok(&["create", "x", "--silent"]).trim().to_string();
    sb.ok(&["update", &id, "--priority", "0"]);
    let text = sb.ok(&["history", &id]);
    assert!(text.contains("transaction history") && text.contains("not br's local backup files"));
    let j = sb.json(&["history", &id]);
    assert_eq!(j["versions"].as_array().unwrap().len(), 2);
    assert!(j["versions"][1]["changes"][0]
        .as_str()
        .unwrap()
        .starts_with("priority:"));
    assert_eq!(code(&sb.run(&["history", &id, "--at", "1"])), 2);
}

#[test]
fn config_lists_and_gets_resolved_values_and_never_prints_a_token() {
    let sb = Sandbox::new("config");
    sb.ok(&["init", "--prefix", "cf"]);
    let list = sb.json(&["config", "list"]);
    assert_eq!(list["project.prefix"], "cf");
    assert_eq!(list["quipu.token"], "(unset)");
    assert_eq!(sb.json(&["config", "get", "project.prefix"])["value"], "cf");
    assert_eq!(code(&sb.run(&["config", "get", "no.such.key"])), 2);
    let path = sb.json(&["config", "path"]);
    assert_eq!(path["project"]["exists"], true);
    // A token in the environment and in a token file: shown as set, never printed.
    let secret = "s3cr3t-token-value-7788";
    std::fs::write(sb.root.join("home/tok"), secret).unwrap();
    std::fs::create_dir_all(sb.root.join("home/.config/seeds")).unwrap();
    std::fs::write(
        sb.root.join("home/.config/seeds/config.toml"),
        "[quipu]\ntoken_file = \"~/tok\"\n",
    )
    .unwrap();
    let from_file = sb.ok(&["config", "list", "--json"]);
    assert!(from_file.contains("(set: token_file)") && !from_file.contains(secret));
    let o = sb
        .cmd(&sb.work(), &["config", "list"])
        .env("SEEDS_QUIPU_TOKEN", secret)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("(set: SEEDS_QUIPU_TOKEN)") && !text.contains(secret));
    // Writing is not built, and says which file to edit.
    let o = sb.run(&["config", "set", "project.prefix", "zz"]);
    assert_eq!(code(&o), 20);
    assert!(String::from_utf8_lossy(&o.stderr).contains("config.toml"));
}

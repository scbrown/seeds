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
            // A bare --due date is 09:00 LOCAL; pin the zone so results do
            // not depend on the host.
            .env("TZ", "America/New_York")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in [
            "SEEDS_QUIPU_STORE",
            "SEEDS_QUIPU_URL",
            "SEEDS_GRAPH",
            "SEEDS_PREFIX",
            "SEEDS_AGENT_NAME",
            "SEEDS_HARNESS",
            "SEEDS_MODEL",
            "SEEDS_SESSION",
            "BR_AGENT_NAME",
            "BR_HARNESS",
            "BR_MODEL",
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

// aegis-bqgdr3 / w3k75d.15 C1: `--defer` on update is stored as given, and a
// value that is not an xsd:date or xsd:dateTime is refused, never coerced.
#[test]
fn a_defer_that_is_not_an_xsd_date_or_date_time_is_refused_by_name() {
    let sb = Sandbox::new("defer-lexical");
    let id = sb.ok(&["create", "x", "--silent"]).trim().to_string();
    let o = sb.run(&["update", &id, "--defer", "2026-10-03 14:00:00Z"]);
    assert_eq!(code(&o), 2, "{}", String::from_utf8_lossy(&o.stderr));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("defer_until \"2026-10-03 14:00:00Z\"") && err.contains("nothing was written"),
        "{err}"
    );
    let v = sb.json(&["show", &id]);
    assert!(v[0]["defer_until"].is_null(), "nothing landed: {v}");
    // The two forms that are valid land exactly as written.
    for d in ["2026-10-03", "2026-10-03T14:00:00.5+02:00"] {
        sb.ok(&["update", &id, "--defer", d]);
        assert_eq!(sb.json(&["show", &id])[0]["defer_until"], d);
    }
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
    // chmln/sd also prints "sd <ver>"; these tell seeds apart (aegis-1i5h1j).
    assert_eq!(v["tool"], "seeds");
    let ver = format!("sd {} (seeds)", env!("CARGO_PKG_VERSION"));
    assert_eq!(sb.ok(&["--version"]).trim(), ver);
    assert_eq!(sb.ok(&["version"]).trim(), ver);
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

#[test]
fn capabilities_are_derived_and_classify_every_verb() {
    let sb = Sandbox::new("capabilities");
    let c = sb.json(&["capabilities"]);
    let cmds = c["commands"].as_array().unwrap();
    let op = |name: &str| {
        cmds.iter()
            .find(|x| x["name"] == name)
            .map(|x| x["operation"].as_str().unwrap().to_string())
    };
    // Every leaf verb is classified; none is unknown.
    assert!(cmds.iter().all(|x| x["operation"] != "unknown"), "{c}");
    assert_eq!(op("create").as_deref(), Some("write"));
    assert_eq!(op("comments add").as_deref(), Some("write"));
    assert_eq!(op("list").as_deref(), Some("read"));
    assert_eq!(op("graph").as_deref(), Some("read"));
    assert_eq!(op("query").as_deref(), Some("elsewhere"));
    let codes: Vec<i64> = c["exit_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_i64().unwrap())
        .collect();
    assert_eq!(codes, [0, 1, 2, 3, 4, 5, 6, 7, 8, 20, 21]);
    assert_eq!(
        sb.json(&["capabilities", "--for", "comments add"])["operation"],
        "write"
    );
    assert_eq!(code(&sb.run(&["capabilities", "--for", "no such verb"])), 2);
    // Visible, so a reader (and parity-diff.py) can find br's spelling.
    assert!(sb
        .ok(&["capabilities", "--help"])
        .contains("[alias: --for]"));
    // Reads nothing from a ledger, creates nothing.
    assert!(!sb.work().join(".seeds").exists());
}

#[test]
fn doctor_passes_a_fresh_ledger_with_exit_0() {
    let sb = Sandbox::new("doctor");
    sb.ok(&["create", "a"]);
    let v = sb.json(&["doctor"]);
    assert_eq!(v["ok"], true, "{v}");
    for c in v["checks"].as_array().unwrap() {
        assert_ne!(c["status"], "error", "{c}");
    }
}

#[test]
fn doctor_quick_skips_only_the_ledger_validation_and_triage_names_a_fix() {
    // aegis-w3k75d.13: br's --quick and --robot-triage.
    let sb = Sandbox::new("doctor-quick");
    sb.ok(&["create", "a"]);
    sb.ok(&["init", "--force"]); // the .gitignore an init writes
    let full = sb.json(&["doctor"]);
    let quick = sb.json(&["doctor", "--quick"]);
    let names = |v: &serde_json::Value| -> Vec<String> {
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&full), names(&quick), "the same checks are listed");
    for (f, q) in full["checks"]
        .as_array()
        .unwrap()
        .iter()
        .zip(quick["checks"].as_array().unwrap())
    {
        if q["name"] == "ledger.valid" {
            assert_eq!(f["status"], "ok", "control: full validation ran\n{f}");
            assert_eq!(q["status"], "skipped", "{q}");
        } else {
            assert_eq!(f, q, "every other check is the same");
        }
    }
    assert_eq!(quick["ok"], true);

    // Healthy: an empty triage that recommends nothing but doctor itself.
    let t: serde_json::Value = serde_json::from_str(&sb.ok(&["doctor", "--robot-triage"])).unwrap();
    assert_eq!(t["schema_version"], "sd.doctor.triage.v1", "{t}");
    assert_eq!(t["findings"], serde_json::json!([]), "{t}");
    assert_eq!(t["actions_planned"], serde_json::json!([]), "{t}");
    assert_eq!(t["recommended_command"], "sd doctor", "{t}");

    // A missing .gitignore: one warning, with the command that restores it,
    // and that command does.
    std::fs::remove_file(sb.work().join(".seeds/.gitignore")).unwrap();
    let t: serde_json::Value = serde_json::from_str(&sb.ok(&["doctor", "--robot-triage"])).unwrap();
    assert_eq!(t["quick_ref"]["warn"], 1, "{t}");
    assert_eq!(t["findings"][0]["name"], ".gitignore", "{t}");
    assert_eq!(t["recommended_command"], "sd init --force", "{t}");
    sb.ok(&["init", "--force"]);
    assert!(sb.work().join(".seeds/.gitignore").exists());
}

#[test]
fn info_schema_whats_new_and_thanks() {
    // aegis-w3k75d.13: br's info --schema, --whats-new and --thanks.
    use sha2::{Digest, Sha256};
    let sb = Sandbox::new("info-flags");

    // --whats-new and --thanks are about this build: no store, nothing created.
    let w = sb.json(&["info", "--whats-new"]);
    assert_eq!(w["version"], env!("CARGO_PKG_VERSION"), "{w}");
    let release = w["release"].as_str().unwrap();
    assert!(
        release.starts_with('[')
            && include_str!("../CHANGELOG.md").contains(&format!("## {release}")),
        "the latest section of the shipped changelog: {w}"
    );
    assert!(!w["changes"].as_str().unwrap().is_empty(), "{w}");
    let t = sb.json(&["info", "--thanks"]);
    assert_eq!(t["thanks"].as_array().unwrap().len(), 3, "{t}");
    assert!(!sb.work().join(".seeds").exists(), "nothing was created");
    assert_eq!(code(&sb.run(&["info", "--whats-new", "--thanks"])), 2);

    // --schema: the digest of the shapes sd validates against, and their classes.
    sb.ok(&["create", "a"]);
    let plain = sb.json(&["info"]);
    assert!(plain.get("schema").is_none(), "only on request: {plain}");
    let i = sb.json(&["info", "--schema"]);
    let want = format!(
        "{:x}",
        Sha256::digest(include_str!("../shapes/seeds.shapes.ttl").as_bytes())
    );
    assert_eq!(i["schema"]["shapes_sha256"], want.as_str(), "{i}");
    assert!(
        i["schema"]["target_classes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "https://schema.org/Action"),
        "{i}"
    );
    assert_eq!(i["issue_count"], 1, "the plain info fields stay: {i}");
}

/// A one-shot HTTP server answering `body` with `status`; returns its URL.
fn fake_release_server(status: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/releases/latest", listener.local_addr().unwrap());
    let (status, body) = (status.to_string(), body.to_string());
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

#[test]
fn version_check_exits_0_current_1_update_7_cannot_tell() {
    // aegis-w3k75d.13: br's version --check contract (0 up to date, 1 update
    // available), plus 7 when sd cannot tell, so "offline" never reads as either.
    let sb = Sandbox::new("version-check");
    let mine = env!("CARGO_PKG_VERSION");
    let run = |url: &str| {
        let o = sb
            .cmd(&sb.work(), &["version", "--check", "--json"])
            .env("SEEDS_RELEASES_URL", url)
            .output()
            .unwrap();
        (code(&o), String::from_utf8_lossy(&o.stdout).to_string())
    };
    let (c, out) = run(&fake_release_server(
        "200 OK",
        &format!(r#"{{"tag_name":"seeds-ai-v{mine}"}}"#),
    ));
    assert_eq!(c, 0, "{out}");
    let (c, out) = run(&fake_release_server(
        "200 OK",
        r#"{"tag_name":"seeds-ai-v999.0.0"}"#,
    ));
    assert_eq!(c, 1, "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (v["update_available"].clone(), v["latest_version"].clone()),
        (true.into(), "999.0.0".into())
    );
    let (c, _) = run(&fake_release_server("200 OK", r#"{"tag_name":"nightly"}"#));
    assert_eq!(c, 7, "an unreadable tag is not 'up to date'");
    let (c, _) = run(&fake_release_server(
        "404 Not Found",
        r#"{"message":"Not Found"}"#,
    ));
    assert_eq!(c, 7);
    let (c, _) = run("http://127.0.0.1:9/releases/latest");
    assert_eq!(c, 7, "offline");
    assert!(!sb.work().join(".seeds").exists(), "needs no store");
}

#[test]
fn orphans_fix_closes_only_on_an_explicit_yes() {
    // aegis-w3k75d.13: br's --fix. EOF and anything but yes skip.
    use std::io::Write;
    let sb = Sandbox::new("orphans-fix");
    let a = sb.ok(&["create", "one", "--silent"]).trim().to_string();
    let b = sb.ok(&["create", "two", "--silent"]).trim().to_string();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(sb.work())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.org")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.org")
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&[
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        &format!("work on {a} and {b}"),
    ]);
    let fix = |input: &str| -> serde_json::Value {
        let mut c = sb
            .cmd(&sb.work(), &["orphans", "--fix", "--json"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    };
    let status = |id: &str| sb.json(&["show", id])[0]["status"].clone();

    // No input at all: both skipped, nothing closed.
    let v = fix("");
    assert!(
        v.as_array().unwrap().iter().all(|o| o["fix"] == "skipped"),
        "{v}"
    );
    assert_eq!((status(&a), status(&b)), ("open".into(), "open".into()));

    // yes to the first, no to the second (orphans list in id order).
    let v = fix("y\nn\n");
    let first = v[0]["issue_id"].as_str().unwrap().to_string();
    let other = if first == a { &b } else { &a };
    assert_eq!(
        (v[0]["fix"].clone(), v[1]["fix"].clone()),
        ("closed".into(), "skipped".into()),
        "{v}"
    );
    let shown = sb.json(&["show", &first]);
    assert_eq!(shown[0]["status"], "closed");
    assert_eq!(
        shown[0]["close_reason"],
        "Implemented (detected by orphans scan)"
    );
    assert_eq!(status(other), "open");

    // Plain orphans stays read-only and carries no fix field.
    let v = sb.json(&["orphans"]);
    assert!(v[0].get("fix").is_none(), "{v}");
}

#[test]
fn create_file_imports_br_markdown_in_one_go() {
    // aegis-w3k75d.13: br's create --file.
    let sb = Sandbox::new("create-file");
    let f = sb.root.join("bulk.md");
    std::fs::write(&f, "## One\n### Priority\n1\n\n## Two\n").unwrap();
    let path = f.to_str().unwrap();
    let dry = sb.json(&["create", "--file", path, "--dry-run"]);
    assert_eq!(dry.as_array().unwrap().len(), 2, "{dry}");
    assert_eq!(
        code(&sb.run(&["show", dry[0]["id"].as_str().unwrap()])),
        3,
        "dry run wrote nothing"
    );
    let made = sb.json(&["create", "-f", path]);
    assert_eq!(made[0]["priority"], 1, "{made}");
    assert_eq!(sb.json(&["count"])["count"], 2);

    // A section seeds cannot store refuses the whole file.
    std::fs::write(&f, "## Three\n\n## Four\n### Nonsense\nx\n").unwrap();
    assert_eq!(code(&sb.run(&["create", "--file", path])), 2);
    assert_eq!(sb.json(&["count"])["count"], 2, "nothing written");
    assert_eq!(
        code(&sb.run(&["create", "--file", path, "--title", "x"])),
        2
    );
}

#[test]
fn design_acceptance_and_external_ref_round_trip() {
    // aegis-w3k75d.13 step 2: br's design, acceptance_criteria, external_ref.
    let sb = Sandbox::new("text-fields");
    let made = sb.json(&[
        "create",
        "x",
        "--acceptance",
        "- [ ] one",
        "--external-ref",
        "gh-7",
    ]);
    let id = made["id"].as_str().unwrap().to_string();
    assert_eq!(made["acceptance_criteria"], "- [ ] one", "{made}");
    assert_eq!(made["external_ref"], "gh-7", "{made}");
    assert!(made["design"].is_null(), "{made}");

    sb.ok(&[
        "update",
        &id,
        "--design",
        "G1",
        "--acceptance-criteria",
        "- [ ] one",
    ]);
    let shown = sb.json(&["show", &id]);
    assert_eq!(shown[0]["design"], "G1", "{shown}");
    assert_eq!(shown[0]["acceptance_criteria"], "- [ ] one");

    // Replacing non-empty text with different text needs --force, as notes do.
    assert_eq!(
        code(&sb.run(&["update", &id, "--design", "G2"])),
        5,
        "refused"
    );
    assert_eq!(
        sb.json(&["show", &id])[0]["design"],
        "G1",
        "nothing written"
    );
    sb.ok(&["update", &id, "--design", "G2", "--force"]);
    assert_eq!(sb.json(&["show", &id])[0]["design"], "G2");

    // external_ref is a plain value: replaced freely, "" clears it.
    sb.ok(&["update", &id, "--external-ref", "gh-8"]);
    let csv = sb.ok(&["list", "--format", "csv", "--fields", "id,external_ref"]);
    assert!(csv.contains(&format!("{id},gh-8")), "{csv}");
    sb.ok(&["update", &id, "--external-ref", ""]);
    assert!(sb.json(&["show", &id])[0]["external_ref"].is_null());

    let text = sb.ok(&["show", &id]);
    assert!(text.contains("  design:\n    G2"), "{text}");
    assert!(
        text.contains("  acceptance criteria:\n    - [ ] one"),
        "{text}"
    );
}

#[test]
fn due_estimate_and_overdue_round_trip() {
    // aegis-w3k75d.13 step 2: br's due_at, estimated_minutes and --overdue.
    let sb = Sandbox::new("due-estimate");
    let made = sb.json(&["create", "late", "--due", "2001-01-01", "-e", "90"]);
    let late = made["id"].as_str().unwrap().to_string();
    // A bare date is 09:00 local (br's rule): EST in January, so 14:00Z.
    assert_eq!(made["due_at"], "2001-01-01T14:00:00Z", "{made}");
    assert_eq!(made["estimated_minutes"], 90, "{made}");
    let soon = sb.json(&["create", "soon", "--due", "+1d"]);
    let soon = soon["id"].as_str().unwrap().to_string();
    let none = sb.ok(&["create", "undated", "--silent"]).trim().to_string();
    let q = sb.json(&["q", "quick", "-e", "15"]);
    let q = q["id"].as_str().unwrap().to_string();
    assert_eq!(sb.json(&["show", &q])[0]["estimated_minutes"], 15);

    let ids = |v: &serde_json::Value| -> Vec<String> {
        v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_string())
            .collect()
    };
    // Only the past-due seed is overdue; the future and undated ones are not.
    assert_eq!(
        ids(&sb.json(&["list", "--overdue"])),
        std::slice::from_ref(&late)
    );
    assert_eq!(
        ids(&sb.json(&["search", "late", "--overdue"])),
        std::slice::from_ref(&late)
    );
    // A closed seed is never overdue, even with --all (br: terminal is excluded).
    sb.ok(&["close", &late]);
    assert!(ids(&sb.json(&["list", "--overdue", "--all"])).is_empty());

    // update sets, clears and refuses out-of-range values like br.
    sb.ok(&[
        "update",
        &soon,
        "--due",
        "2001-01-02T03:04:05Z",
        "--estimate",
        "0",
    ]);
    let s = sb.json(&["show", &soon]);
    assert_eq!(s[0]["due_at"], "2001-01-02T03:04:05Z");
    assert_eq!(s[0]["estimated_minutes"], 0);
    assert_eq!(
        ids(&sb.json(&["list", "--overdue"])),
        std::slice::from_ref(&soon)
    );
    sb.ok(&["update", &soon, "--due", ""]);
    assert!(sb.json(&["show", &soon])[0]["due_at"].is_null());
    for bad in [
        vec!["update", none.as_str(), "--estimate", "-1"],
        vec!["update", none.as_str(), "--estimate", "525961"],
        vec!["update", none.as_str(), "--due", "someday"],
        vec!["create", "x", "--due", "nope"],
    ] {
        assert_eq!(code(&sb.run(&bad)), 2, "{bad:?}");
    }
    assert!(sb.json(&["show", &none])[0]["estimated_minutes"].is_null());

    let csv = sb.ok(&["list", "--all", "--format", "csv", "--fields", "id,due_at"]);
    assert!(
        csv.contains(&format!("{late},2001-01-01T14:00:00Z")),
        "{csv}"
    );
    let text = sb.ok(&["show", &late]);
    assert!(
        text.contains("  due 2001-01-01T14:00:00Z") && text.contains("  estimate 90m"),
        "{text}"
    );
}

#[test]
fn a_bare_due_date_is_nine_local_as_an_instant() {
    // Spec (aegis-w3k75d.13, wu's review of seeds#75): a date-only --due, or
    // `tomorrow`, is 09:00 in the local zone, stored as an RFC 3339 UTC
    // instant; other forms pass through. The zone is America/New_York here.
    let sb = Sandbox::new("due-local");
    let due = |args: &[&str]| sb.json(args)["due_at"].as_str().unwrap().to_string();
    // EDT (UTC-4) in October, EST (UTC-5) in December.
    assert_eq!(
        due(&["create", "a", "--due", "2026-10-01"]),
        "2026-10-01T13:00:00Z"
    );
    assert_eq!(
        due(&["create", "b", "--due", "2026-12-01"]),
        "2026-12-01T14:00:00Z"
    );
    // An instant is kept as given.
    assert_eq!(
        due(&["create", "c", "--due", "2026-10-01T00:00:00Z"]),
        "2026-10-01T00:00:00Z"
    );
    // `tomorrow` is an instant at 09:00 local too, never a bare date.
    let t = due(&["create", "d", "--due", "tomorrow"]);
    assert!(t.ends_with(":00:00Z") && t.len() == 20, "{t}");
    // The same date under another zone moves with it.
    let o = sb
        .cmd(
            &sb.work(),
            &["create", "e", "--due", "2026-10-01", "--json"],
        )
        .env("TZ", "UTC")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["due_at"], "2026-10-01T09:00:00Z");
    // An impossible date is refused; nothing is written.
    let before = sb.json(&["count"])["count"].clone();
    assert_eq!(code(&sb.run(&["create", "f", "--due", "2026-02-30"])), 2);
    assert_eq!(sb.json(&["count"])["count"], before);
}

#[test]
fn acceptance_checklist_flags_edit_in_place() {
    // aegis-w3k75d.13: br's --check/--uncheck/--add-acceptance on update.
    let sb = Sandbox::new("checklist");
    let id = sb
        .ok(&[
            "create",
            "x",
            "--acceptance",
            "- [ ] one\n- [ ] two",
            "--silent",
        ])
        .trim()
        .to_string();
    let ac = || sb.json(&["show", &id])[0]["acceptance_criteria"].clone();

    // No --force needed: these edit the field in place.
    sb.ok(&[
        "update",
        &id,
        "--check-acceptance",
        "2",
        "--add-acceptance",
        "three",
    ]);
    assert_eq!(ac(), "- [ ] one\n- [x] two\n- [ ] three\n");
    sb.ok(&[
        "update",
        &id,
        "--check-acceptance",
        "ONE",
        "--uncheck-acceptance",
        "two",
    ]);
    assert_eq!(ac(), "- [x] one\n- [ ] two\n- [ ] three\n");

    // A bad selector refuses the whole update (exit 2); nothing is written.
    let before = ac();
    for bad in [
        vec!["update", id.as_str(), "--check-acceptance", "9"],
        vec!["update", id.as_str(), "--check-acceptance", "t"],
        vec![
            "update",
            id.as_str(),
            "--check-acceptance",
            "1",
            "--acceptance",
            "- [ ] new",
        ],
    ] {
        assert_eq!(code(&sb.run(&bad)), 2, "{bad:?}");
    }
    assert_eq!(ac(), before);
}

#[test]
fn orphans_reads_the_git_log_of_the_current_repo() {
    let sb = Sandbox::new("orphans");
    let id = sb.ok(&["create", "x", "--silent"]).trim().to_string();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(sb.work())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.org")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.org")
            .output()
            .unwrap()
    };
    assert_eq!(code(&sb.run(&["orphans"])), 2, "not a git repo yet");
    git(&["init", "-q"]);
    assert_eq!(
        sb.json(&["orphans"]),
        serde_json::json!([]),
        "no commits yet: empty, not an error"
    );
    git(&[
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        &format!("work on {id}"),
    ]);
    let v = sb.json(&["orphans"]);
    assert_eq!(v[0]["issue_id"], id.as_str());
    assert_eq!(
        v[0]["latest_commit_message"],
        format!("work on {id}").as_str()
    );
}

#[test]
fn count_by_shorthands_equal_by_and_conflicts_are_refused() {
    let sb = Sandbox::new("count-by");
    sb.ok(&["create", "a", "-p", "1"]);
    sb.ok(&["create", "b", "-p", "2"]);
    assert_eq!(
        sb.json(&["count", "--by-priority"]),
        sb.json(&["count", "--by", "priority"])
    );
    assert_eq!(sb.json(&["count", "--priority", "1"])["count"], 1);
    assert_eq!(code(&sb.run(&["count", "--by-status", "--by-type"])), 2);
    assert_eq!(code(&sb.run(&["count", "--by", "status", "--by-type"])), 2);
}

#[test]
fn update_changes_type_labels_parent_and_description_like_br() {
    let sb = Sandbox::new("update-flags");
    let a = sb
        .ok(&["create", "a", "-l", "x,y", "--silent"])
        .trim()
        .to_string();
    let p = sb.ok(&["create", "parent", "--silent"]).trim().to_string();
    // -t (sd-non.3), --set-labels replaces all, --parent reparents.
    let up = sb.json(&[
        "update",
        &a,
        "-t",
        "bug",
        "--set-labels",
        "z",
        "--parent",
        &p,
    ]);
    assert_eq!(up[0]["issue_type"], "bug");
    assert_eq!(up[0]["labels"], serde_json::json!(["z"]));
    assert_eq!(up[0]["parent"], p.as_str());
    // br's --labels is its alias for --set-labels: it also replaces all.
    let up = sb.json(&["update", &a, "--labels", "w,v"]);
    assert_eq!(up[0]["labels"], serde_json::json!(["v", "w"]));
    assert!(sb.ok(&["update", "--help"]).contains("[alias: --labels]"));
    // A seed cannot become its own ancestor.
    assert_eq!(code(&sb.run(&["update", &p, "--parent", &a])), 5);
    // "" detaches; --body is --description; --description-file reads a file.
    assert!(sb.json(&["update", &a, "--parent", ""])[0]["parent"].is_null());
    assert_eq!(
        sb.json(&["update", &a, "--body", "b1"])[0]["description"],
        "b1"
    );
    // Visible in --help, so a reader (and parity-diff.py) can find it.
    for verb in ["create", "update", "q"] {
        assert!(
            sb.ok(&[verb, "--help"]).contains("[alias: --body]"),
            "{verb}"
        );
    }
    let f = sb.root.join("desc.md");
    std::fs::write(&f, "## Acceptance Criteria\n- x\n").unwrap();
    // Replacing the existing "b1" is br's overwrite guard: --force says so.
    assert_eq!(
        code(&sb.run(&["update", &a, "--description-file", f.to_str().unwrap()])),
        5
    );
    let d = sb.json(&[
        "update",
        &a,
        "--description-file",
        f.to_str().unwrap(),
        "--force",
    ]);
    assert_eq!(d[0]["description"], "## Acceptance Criteria\n- x\n");
}

#[test]
fn update_refuses_replacing_text_without_force_like_br() {
    // aegis-w3k75d.13 (wu's spec): non-empty -> different refuses (5) and
    // writes nothing; --force passes; empty -> value and the same value pass;
    // notes behave the same. Clearing existing text is a replacement too.
    let sb = Sandbox::new("update-force");
    let a = sb.ok(&["create", "a", "--silent"]).trim().to_string();
    for field in ["--description", "--notes"] {
        let key = field.trim_start_matches("--");
        let get = || sb.json(&["show", &a])[0][key].clone();
        sb.ok(&["update", &a, field, "first"]); // empty -> value
        sb.ok(&["update", &a, field, "first"]); // same value: idempotent
        let o = sb.run(&["update", &a, field, "second"]);
        assert_eq!(code(&o), 5, "{field}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(key) && err.contains("--force"), "{err}");
        assert_eq!(get(), "first", "{field}: nothing was written");
        assert_eq!(code(&sb.run(&["update", &a, field, ""])), 5, "clearing");
        sb.ok(&["update", &a, field, "second", "--force"]);
        assert_eq!(get(), "second");
    }
}

#[test]
fn create_takes_an_initial_status_or_a_defer_date() {
    let sb = Sandbox::new("create-status");
    let s = sb.json(&["create", "working", "--status", "in_progress"]);
    assert_eq!(s["status"], "in_progress");
    let d = sb.json(&["create", "later", "--defer", "2099-01-01"]);
    assert_eq!(
        (d["status"].as_str(), d["defer_until"].as_str()),
        (Some("deferred"), Some("2099-01-01"))
    );
    assert_eq!(code(&sb.run(&["create", "x", "--status", "closed"])), 2);
    assert_eq!(
        code(&sb.run(&["create", "x", "--status", "open", "--defer", "+1d"])),
        2
    );
}

#[test]
fn format_json_is_json_text_is_text_and_toon_is_refused() {
    let sb = Sandbox::new("format");
    let id = sb.ok(&["create", "a", "--silent"]).trim().to_string();
    let as_json: Value = serde_json::from_str(&sb.ok(&["show", &id, "--format", "json"])).unwrap();
    assert_eq!(as_json[0]["id"], id.as_str());
    assert!(sb.ok(&["show", &id, "--format", "text"]).contains(&id));
    let o = sb.run(&["show", &id, "--format", "toon"]);
    assert_eq!(code(&o), 2);
    assert!(String::from_utf8_lossy(&o.stderr).contains("toon"));
}

#[test]
fn ready_epic_is_parent_recursive_and_conflicts_with_parent() {
    let sb = Sandbox::new("ready-epic");
    let p = sb.ok(&["create", "epic", "--silent"]).trim().to_string();
    let c = sb
        .ok(&["create", "child", "--parent", &p, "--silent"])
        .trim()
        .to_string();
    let g = sb
        .ok(&["create", "grandchild", "--parent", &c, "--silent"])
        .trim()
        .to_string();
    let ids = |args: &[&str]| -> Vec<String> {
        sb.json(args)
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&["ready", "--epic", &p]), [c.clone(), g.clone()]);
    assert_eq!(ids(&["ready", "--parent", &p, "-r"]), [c.clone(), g]);
    assert_eq!(ids(&["ready", "--parent", &p]), [c]);
    assert_eq!(code(&sb.run(&["ready", "--epic", &p, "--parent", &p])), 2);
    assert_eq!(code(&sb.run(&["ready", "-r"])), 2);
    assert_eq!(code(&sb.run(&["ready", "--sort", "bogus"])), 2);
}

#[test]
fn quiet_prints_nothing_on_success_and_still_explains_a_failure() {
    let sb = Sandbox::new("quiet");
    let o = sb.run(&["-q", "create", "a"]);
    assert_eq!(code(&o), 0);
    assert!(o.stdout.is_empty() && o.stderr.is_empty(), "{o:?}");
    // The write happened: -q silences the report, not the work.
    assert_eq!(sb.json(&["ready"]).as_array().unwrap().len(), 1);
    let o = sb.run(&["show", "sd-nope", "--quiet"]);
    assert_ne!(code(&o), 0);
    assert!(!o.stderr.is_empty());
}

#[test]
fn format_csv_on_list_and_search_quotes_like_rfc4180_and_refuses_what_it_would_ignore() {
    let sb = Sandbox::new("csv");
    let id = sb
        .ok(&["create", "has, comma and \"quote\"", "-p", "1", "--silent"])
        .trim()
        .to_string();
    let out = sb.ok(&["list", "--format", "csv"]);
    let mut lines = out.lines();
    assert_eq!(
        lines.next().unwrap(),
        "id,title,status,priority,issue_type,assignee,created_at,updated_at"
    );
    let row = lines.next().unwrap();
    assert!(
        row.starts_with(&format!(
            "{id},\"has, comma and \"\"quote\"\"\",open,1,task,,"
        )),
        "{row}"
    );
    assert_eq!(
        sb.ok(&["list", "--format", "csv", "--fields", "id,priority"]),
        format!("id,priority\n{id},1\n")
    );
    assert_eq!(
        sb.ok(&["search", "comma", "--format", "csv", "--fields", "id"]),
        format!("id\n{id}\n")
    );
    for bad in [
        vec!["list", "--format", "csv", "--fields", "id,nonsense"],
        vec!["list", "--fields", "id"],
        vec!["list", "--format", "csv", "--json"],
        vec!["ready", "--format", "csv"],
    ] {
        assert_eq!(code(&sb.run(&bad)), 2, "{bad:?}");
    }
}

// br's --long/--pretty/--tree, measured on a br scratch store: --long adds the
// fields with dates, --pretty the same fields with connectors and no dates,
// --tree nests children and lifts one whose parent is filtered out. br lets
// --tree silently win over the others and --json ignore all three; sd refuses
// each combination instead of ignoring it.
#[test]
fn list_long_pretty_and_tree_layouts_and_refusals() {
    let sb = Sandbox::new("layouts");
    let p = sb
        .ok(&["create", "epic", "-p", "1", "--silent"])
        .trim()
        .to_string();
    let c = sb
        .ok(&["create", "child", "--parent", &p, "--silent"])
        .trim()
        .to_string();
    let g = sb
        .ok(&["create", "grand", "--parent", &c, "--silent"])
        .trim()
        .to_string();
    let tree = sb.ok(&["list", "--tree"]);
    let lines: Vec<&str> = tree.lines().collect();
    assert!(
        lines[0].contains(&p) && !lines[0].starts_with(['├', '└']),
        "{tree}"
    );
    assert!(
        lines[1].starts_with("└── ") && lines[1].contains(&c),
        "{tree}"
    );
    assert!(
        lines[2].starts_with("    └── ") && lines[2].contains(&g),
        "{tree}"
    );
    // The epic filtered out (P1): its child is a root again.
    let lifted = sb.ok(&["list", "--tree", "-p", "2"]);
    assert!(lifted.lines().next().unwrap().contains(&c), "{lifted}");
    assert!(
        !lifted.lines().next().unwrap().starts_with(['├', '└']),
        "{lifted}"
    );

    let long = sb.ok(&["list", "--long", "--id", &p]);
    assert!(
        long.contains("\n  Status: open\n  Priority: P1\n  Type: task\n  Created: "),
        "{long}"
    );
    let pretty = sb.ok(&["list", "--pretty", "--id", &p]);
    assert!(
        pretty.contains("\n├── Status: open\n├── Priority: P1\n└── Type: task"),
        "{pretty}"
    );
    assert!(!pretty.contains("Created"), "{pretty}");
    assert!(sb.ok(&["search", "grand", "--tree"]).contains(&g));

    for bad in [
        vec!["list", "--tree", "--long"],
        vec!["list", "--pretty", "--tree"],
        vec!["list", "--tree", "--json"],
        vec!["list", "--long", "--format", "csv"],
    ] {
        assert_eq!(code(&sb.run(&bad)), 2, "{bad:?}");
    }
}

#[test]
fn dep_list_type_filters_edges_and_comments_content_is_visible() {
    let sb = Sandbox::new("dep-type");
    let p = sb.ok(&["create", "epic", "--silent"]).trim().to_string();
    let b = sb.ok(&["create", "blocker", "--silent"]).trim().to_string();
    let c = sb
        .ok(&["create", "child", "--parent", &p, "--deps", &b, "--silent"])
        .trim()
        .to_string();
    let ids = |args: &[&str]| -> Vec<String> {
        sb.json(args)
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["depends_on_id"].as_str().unwrap().to_string())
            .collect()
    };
    // Control: unfiltered, the child has both edges.
    let mut all = ids(&["dep", "list", &c]);
    all.sort();
    let mut want = vec![p.clone(), b.clone()];
    want.sort();
    assert_eq!(all, want);
    assert_eq!(ids(&["dep", "list", &c, "--type", "blocks"]), [b]);
    assert_eq!(ids(&["dep", "list", &c, "-t", "parent-child"]), [p]);
    // br prints an error but exits 0 on an unknown type; sd exits 2.
    assert_eq!(code(&sb.run(&["dep", "list", &c, "--type", "bogus"])), 2);
    assert!(sb
        .ok(&["comments", "add", "--help"])
        .contains("[alias: --content]"));
    sb.ok(&["comments", "add", &c, "--content", "via content"]);
    assert!(sb.ok(&["comments", "list", &c]).contains("via content"));
}

// br's blocked --detailed, measured on a br scratch store: each blocker on its
// own line with title, priority and status.
#[test]
fn blocked_detailed_lists_each_blocker_with_title_priority_and_status() {
    let sb = Sandbox::new("blocked-detailed");
    let a = sb
        .ok(&["create", "blocked one", "--silent"])
        .trim()
        .to_string();
    let b = sb
        .ok(&["create", "the blocker", "-p", "1", "--silent"])
        .trim()
        .to_string();
    sb.ok(&["dep", "add", &a, &b]);
    // Control: the short form names the blocker only by id.
    let short = sb.ok(&["blocked"]);
    assert!(
        short.contains(&format!("blocked by 1 open: {b}")),
        "{short}"
    );
    assert!(!short.contains("the blocker"), "{short}");
    let detailed = sb.ok(&["blocked", "--detailed"]);
    assert!(
        detailed.contains(&format!(
            "  blocked by:\n    • {b}: the blocker [P1] [open]"
        )),
        "{detailed}"
    );
    assert_eq!(code(&sb.run(&["blocked", "--detailed", "--json"])), 2);
}

#[test]
fn delete_from_file_reads_ids_skipping_blanks_and_comments() {
    let sb = Sandbox::new("delete-from-file");
    let a = sb.ok(&["create", "a", "--silent"]).trim().to_string();
    let b = sb.ok(&["create", "b", "--silent"]).trim().to_string();
    let keep = sb.ok(&["create", "keep", "--silent"]).trim().to_string();
    let f = sb.root.join("ids.txt");
    std::fs::write(&f, format!("# to delete\n{a}\n\n{b}  # trailing note\n")).unwrap();
    sb.ok(&["delete", "--from-file", f.to_str().unwrap()]);
    let left: Vec<String> = sb
        .json(&["ready"])
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(left, [keep], "a and b deleted, keep untouched");
    // A file with no ids refuses instead of deleting nothing silently.
    let empty = sb.root.join("empty.txt");
    std::fs::write(&empty, "# nothing\n\n").unwrap();
    assert_eq!(
        code(&sb.run(&["delete", "--from-file", empty.to_str().unwrap()])),
        2
    );
    // No ids and no file at all is a usage error, as before.
    assert_eq!(code(&sb.run(&["delete"])), 2);
}

// br's close --suggest-next, measured on a br scratch store with this board:
// closing the gate frees the seed that waited only on it, not the one that
// also waits on something else; --json becomes {closed, unblocked}; more than
// one id is refused.
#[test]
fn close_suggest_next_lists_only_seeds_the_close_fully_unblocked() {
    let sb = Sandbox::new("suggest-next");
    let id = |t: &str| sb.ok(&["create", t, "--silent"]).trim().to_string();
    let (gate, other) = (id("gate"), id("other"));
    let y = sb
        .ok(&["create", "waits on gate", "--deps", &gate, "--silent"])
        .trim()
        .to_string();
    let z = sb
        .ok(&[
            "create",
            "waits on both",
            "--deps",
            &format!("{gate},{other}"),
            "--silent",
        ])
        .trim()
        .to_string();
    let v = sb.json(&["close", &gate, "--suggest-next"]);
    assert_eq!(v["closed"][0]["id"], gate.as_str());
    let freed: Vec<&str> = v["unblocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(freed, [y.as_str()], "z still waits on other");
    assert!(!freed.contains(&z.as_str()));
    let text = sb.ok(&["close", &other, "--suggest-next"]);
    assert!(text.contains("unblocked 1:") && text.contains(&z), "{text}");
    let a = id("a");
    let b = id("b");
    assert_eq!(code(&sb.run(&["close", &a, &b, "--suggest-next"])), 2);
    // Without the flag the --json shape is unchanged: a bare array.
    assert!(sb.json(&["close", &a]).is_array());
}

#[test]
fn config_list_project_and_user_show_only_what_that_file_sets() {
    let sb = Sandbox::new("config-scope");
    std::fs::create_dir_all(sb.work().join(".seeds")).unwrap();
    std::fs::write(
        sb.work().join(".seeds/config.toml"),
        "[project]\nprefix = \"zz\"\n",
    )
    .unwrap();
    // Control: the resolved list shows both the file's key and defaults.
    let all = sb.ok(&["config", "list"]);
    assert!(
        all.contains("project.prefix = zz") && all.contains("quipu.url"),
        "{all}"
    );
    assert_eq!(
        sb.ok(&["config", "list", "--project"]).trim(),
        "project.prefix = zz"
    );
    // No user file yet: it sets nothing (not an error).
    assert!(sb
        .ok(&["config", "list", "--user"])
        .contains("sets nothing"));
    std::fs::create_dir_all(sb.root.join("home/.config/seeds")).unwrap();
    std::fs::write(
        sb.root.join("home/.config/seeds/config.toml"),
        "[quipu]\ntrusted_hosts = [\"h.example\"]\n",
    )
    .unwrap();
    let user = sb.json(&["config", "list", "--user"]);
    assert_eq!(
        user,
        serde_json::json!({"quipu.trusted_hosts": ["h.example"]})
    );
    assert_eq!(code(&sb.run(&["config", "list", "--project", "--user"])), 2);
}

#[test]
fn create_slug_embeds_a_normalized_slug_in_the_id() {
    // aegis-w3k75d.13: br's create --slug, from br's --help and observed
    // outputs only (aegis-fur6v8): <prefix>-<slug>-<hash>.
    let sb = Sandbox::new("slug");
    let id = sb
        .ok(&["create", "x", "--slug", "Survey My Thing!", "--silent"])
        .trim()
        .to_string();
    let (head, hash) = id.rsplit_once('-').unwrap();
    assert!(head.ends_with("-survey-my-thing"), "{id}");
    assert_eq!(hash.len(), 3, "{id}");
    // The id round-trips: show finds the seed under it.
    assert_eq!(sb.json(&["show", &id])[0]["id"], id.as_str());

    // A slug that normalizes to nothing mints a plain id, as br does.
    let plain = sb.ok(&["create", "y", "--slug", "!!!", "--silent"]);
    assert_eq!(plain.trim().matches('-').count(), 1, "{plain}");

    // Under --parent the child numbering wins and the slug is ignored, as br.
    let kid = sb.ok(&["create", "z", "--parent", &id, "--slug", "kid", "--silent"]);
    assert_eq!(kid.trim(), format!("{id}.1"));

    // --step derives the id itself, so a slug is a usage error, nothing written.
    let before = ids(&sb.json(&["list", "--all"])).len();
    assert_eq!(before, 3, "control: the three seeds above must be listed");
    let o = sb.run(&[
        "create",
        "w",
        "--workflow-run",
        "r1",
        "--step",
        "s",
        "--slug",
        "nope",
    ]);
    assert_eq!(code(&o), 2);
    assert_eq!(ids(&sb.json(&["list", "--all"])).len(), before);
}

#[test]
fn agent_context_round_trips_and_guards_replacement_like_br() {
    // aegis-w3k75d.13: br's --agent-context on create and update, from br's
    // --help and observed outputs only (aegis-fur6v8).
    let sb = Sandbox::new("agent-context");
    let ac = |id: &str| sb.json(&["show", id])[0]["agent_context"].clone();
    let id = sb
        .ok(&[
            "create",
            "x",
            "--agent-context",
            r#"{"b":2, "a":1}"#,
            "--silent",
        ])
        .trim()
        .to_string();
    // Stored compact, key order kept.
    assert_eq!(ac(&id), r#"{"b":2,"a":1}"#);

    // "" on create leaves it unset.
    let bare = sb
        .ok(&["create", "y", "--agent-context", "", "--silent"])
        .trim()
        .to_string();
    assert!(ac(&bare).is_null());
    // Filling an empty field needs no --force.
    sb.ok(&["update", &bare, "--agent-context", r#"{"z":0}"#]);
    assert_eq!(ac(&bare), r#"{"z":0}"#);

    // Replacing or clearing a non-empty one is refused without --force.
    for v in [r#"{"k":"v"}"#, ""] {
        let o = sb.run(&["update", &id, "--agent-context", v]);
        assert_eq!(code(&o), 5, "{v:?}");
        assert_eq!(ac(&id), r#"{"b":2,"a":1}"#, "{v:?}: nothing written");
    }
    // The same value is a no-op, not a refusal.
    sb.ok(&["update", &id, "--agent-context", r#"{"b":2,"a":1}"#]);
    sb.ok(&["update", &id, "--agent-context", r#"{"k":"v"}"#, "--force"]);
    assert_eq!(ac(&id), r#"{"k":"v"}"#);
    sb.ok(&["update", &id, "--agent-context", "", "--force"]);
    assert!(ac(&id).is_null());

    // Invalid JSON is a usage error and nothing is created.
    let before = ids(&sb.json(&["list", "--all"])).len();
    assert_eq!(before, 2, "control: both seeds above are listed");
    let o = sb.run(&["create", "z", "--agent-context", "{bad"]);
    assert_eq!(code(&o), 2);
    assert_eq!(ids(&sb.json(&["list", "--all"])).len(), before);
}

#[test]
fn ephemeral_seeds_are_read_everywhere_never_ready_never_shared() {
    // aegis-w3k75d.13: br's create --ephemeral, from br's --help and observed
    // outputs only (aegis-fur6v8); lead ruling: a sibling ephemeral graph.
    let sb = Sandbox::new("ephemeral");
    let n = sb
        .ok(&["create", "shared zebra", "--silent"])
        .trim()
        .to_string();
    let e = sb
        .ok(&["create", "ephemeral zebra", "--ephemeral", "--silent"])
        .trim()
        .to_string();
    let ids_of = |v: &Value| ids(v);

    // Every read path sees it: show, list, search, count.
    assert_eq!(sb.json(&["show", &e])[0]["ephemeral"], true);
    assert_eq!(sb.json(&["show", &n])[0]["ephemeral"], false);
    let listed = ids_of(&sb.json(&["list"]));
    assert!(listed.contains(&e) && listed.contains(&n), "{listed:?}");
    let found = ids_of(&sb.json(&["search", "zebra"]));
    assert!(found.contains(&e) && found.contains(&n), "{found:?}");
    assert_eq!(sb.json(&["count"])["count"], 2);

    // Never ready; the shared seed is (control).
    let ready = ids_of(&sb.json(&["ready"]));
    assert!(ready.contains(&n), "control: {ready:?}");
    assert!(!ready.contains(&e), "{ready:?}");

    // Writes to it stay in its graph.
    sb.ok(&["update", &e, "--title", "ephemeral zebra 2"]);
    sb.ok(&["comments", "add", &e, "a note"]);
    sb.ok(&["close", &e, "--reason", "done"]);
    let shown = sb.json(&["show", &e]);
    assert_eq!(shown[0]["ephemeral"], true);
    assert_eq!(shown[0]["status"], "closed");

    // A shared seed may NOT depend on an ephemeral one, by any edge: usage
    // error, nothing written.
    let before = sb.json(&["show", &n]);
    for args in [
        vec!["dep", "add", n.as_str(), e.as_str()],
        vec!["dep", "add", n.as_str(), e.as_str(), "--type", "related"],
    ] {
        let o = sb.run(&args);
        assert_eq!(code(&o), 2, "{args:?}");
    }
    assert_eq!(
        sb.json(&["show", &n])[0]["dependencies"],
        before[0]["dependencies"]
    );
    let count = || ids_of(&sb.json(&["list", "--all"])).len();
    let total = count();
    assert_eq!(total, 2, "control: both seeds are listed");
    for args in [
        vec!["create", "x", "--deps", e.as_str()],
        vec!["create", "x", "--parent", e.as_str()],
    ] {
        let o = sb.run(&args);
        assert_eq!(code(&o), 2, "{args:?}");
    }
    assert_eq!(count(), total, "nothing created");

    // (After the refusals: N -> E checked above, so E -> N is no cycle.)
    // An ephemeral seed may depend on a shared one, and be its child.
    sb.ok(&["dep", "add", &e, &n]);
    let kid = sb
        .ok(&[
            "create",
            "eph kid",
            "--ephemeral",
            "--parent",
            &n,
            "--silent",
        ])
        .trim()
        .to_string();
    assert_eq!(sb.json(&["show", &kid])[0]["ephemeral"], true);

    // The pendant carries the shared seed and not the ephemeral ones, and a
    // fresh store importing it gets only the shared seed.
    let dir = sb.root.join("pendant");
    sb.ok(&["export", "--to", dir.to_str().unwrap()]);
    let nt = std::fs::read_to_string(dir.join("export.nt")).unwrap();
    assert!(
        nt.contains(&format!("/item/{n}>")),
        "control: shared seed exported"
    );
    assert!(!nt.contains(&format!("/item/{e}")), "ephemeral seed leaked");
    assert!(
        !nt.contains(&format!("/item/{kid}")),
        "ephemeral child leaked"
    );
    let other = Sandbox::new("ephemeral-import");
    other.ok(&["import", dir.to_str().unwrap()]);
    assert_eq!(ids_of(&other.json(&["list", "--all"])), vec![n.clone()]);
    // Importing back does not touch the local ephemerals.
    sb.ok(&["import", dir.to_str().unwrap()]);
    assert_eq!(sb.json(&["show", &e])[0]["ephemeral"], true);
}

#[test]
fn cutover_roundtrip_sync_dry_run_and_conflict_are_observable() {
    let sb = Sandbox::new("cutover");
    let input = serde_json::json!({"id":"br-one","title":"one","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","future":{"n":1}});
    std::fs::write(sb.work().join("board.jsonl"), format!("{input}\n")).unwrap();
    let common = [
        "--store",
        "local.db",
        "--graph",
        "https://seeds.local/project/cutover",
    ];
    let call = |args: &[&str]| {
        let mut a = common.to_vec();
        a.extend_from_slice(args);
        sb.run(&a)
    };
    let ok = |args: &[&str]| {
        let o = call(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice::<Value>(&o.stdout).unwrap()
    };
    let dry = ok(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
        "--dry-run",
    ]);
    assert_eq!(dry["created"], serde_json::json!(["br-one"]));
    assert!(!sb.work().join("local.db").exists());
    assert!(!sb.work().join("cursor.json").exists());
    let verify = ok(&["cutover", "verify", "--file", "board.jsonl"]);
    assert_eq!(verify["losses"], 0);
    ok(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ]);
    let second = ok(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ]);
    assert_eq!(second["wrote"], false);
    assert_eq!(second["store_differences"], serde_json::json!([]));
    ok(&["cutover", "export", "--file", "out.jsonl"]);
    assert_eq!(
        std::fs::read_to_string(sb.work().join("out.jsonl")).unwrap(),
        format!("{input}\n")
    );
    let changed = call(&["update", "br-one", "--title", "from seed"]);
    assert!(changed.status.success());
    ok(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ]);
    let peer: Value =
        serde_json::from_str(&std::fs::read_to_string(sb.work().join("board.jsonl")).unwrap())
            .unwrap();
    assert_eq!(peer["title"], "from seed");
    assert_eq!(peer["future"], input["future"]);
    let mut peer = peer;
    peer["priority"] = serde_json::json!(0);
    std::fs::write(sb.work().join("board.jsonl"), format!("{peer}\n")).unwrap();
    assert!(call(&["update", "br-one", "--priority", "1"])
        .status
        .success());
    let before = std::fs::read(sb.work().join("cursor.json")).unwrap();
    let conflict = call(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ]);
    assert_eq!(code(&conflict), 4);
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("priority"));
    assert_eq!(
        std::fs::read(sb.work().join("cursor.json")).unwrap(),
        before
    );
}

#[test]
fn cutover_repeated_peer_edits_replace_only_the_owned_json_shadow() {
    let sb = Sandbox::new("cutover-shadow-replacement");
    let common = [
        "--store",
        "local.db",
        "--graph",
        "https://example.org/cutover",
        "--actor",
        "peer-editor",
    ];
    let ok = |args: &[&str]| {
        let mut a = common.to_vec();
        a.extend_from_slice(args);
        sb.ok(&a)
    };
    let sync = [
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ];
    let foreign = serde_json::json!([[
        "https://example.org/future",
        seeds::model::Obj::Str("preserve".into())
    ]]);
    let mut row = serde_json::json!({"id":"br-one","title":"before","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","future":{"n":1},"_seeds":{"format":"seeds-facts-v1","revision":1,"facts":foreign}});
    std::fs::write(sb.work().join("board.jsonl"), format!("{row}\n")).unwrap();
    ok(&sync);
    for title in [
        "first peer change",
        "second peer change",
        "third peer change",
    ] {
        row["title"] = serde_json::json!(title);
        std::fs::write(sb.work().join("board.jsonl"), format!("{row}\n")).unwrap();
        ok(&sync);
        // Fresh process reads the committed graph, not the planned candidate.
        ok(&["cutover", "export", "--file", "actual.jsonl"]);
        let actual: Value =
            serde_json::from_str(&std::fs::read_to_string(sb.work().join("actual.jsonl")).unwrap())
                .unwrap();
        let peer: Value =
            serde_json::from_str(&std::fs::read_to_string(sb.work().join("board.jsonl")).unwrap())
                .unwrap();
        assert_eq!(actual, peer);
        assert_eq!(actual["title"], title);
        assert_eq!(actual["future"], serde_json::json!({"n":1}));
        assert_eq!(actual["_seeds"]["facts"], foreign);
        row = actual;
        let second: Value = serde_json::from_str(&ok(&sync)).unwrap();
        assert_eq!(second["wrote"], false);
        assert_eq!(second["store_difference_count"], 0);
    }
}

#[test]
fn cutover_recovers_a_store_commit_before_peer_publication() {
    let sb = Sandbox::new("cutover-recovery");
    let common = [
        "--store",
        "local.db",
        "--graph",
        "https://seeds.local/project/recovery",
    ];
    let ok = |args: &[&str]| {
        let mut a = common.to_vec();
        a.extend_from_slice(args);
        sb.ok(&a)
    };
    let item = serde_json::json!({"id":"br-one","title":"old","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"});
    std::fs::write(sb.work().join("board.jsonl"), format!("{item}\n")).unwrap();
    ok(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ]);
    let base: Value =
        serde_json::from_slice(&std::fs::read(sb.work().join("cursor.json")).unwrap()).unwrap();
    ok(&["update", "br-one", "--title", "after crash"]);
    ok(&["cutover", "export", "--file", "desired.jsonl"]);
    let desired: Value =
        serde_json::from_str(&std::fs::read_to_string(sb.work().join("desired.jsonl")).unwrap())
            .unwrap();
    let journal = serde_json::json!({"binding":base["binding"],"local":base["records"],"peer":base["records"],"desired":{"br-one":desired}});
    std::fs::write(sb.work().join("cursor.json.pending"), journal.to_string()).unwrap();
    ok(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ]);
    assert!(!sb.work().join("cursor.json.pending").exists());
    let peer: Value =
        serde_json::from_str(&std::fs::read_to_string(sb.work().join("board.jsonl")).unwrap())
            .unwrap();
    assert_eq!(peer["title"], "after crash");
    let second: Value = serde_json::from_str(&ok(&[
        "cutover",
        "sync",
        "--file",
        "board.jsonl",
        "--base",
        "cursor.json",
    ]))
    .unwrap();
    assert_eq!(second["wrote"], false);
    let mut args = common.to_vec();
    args.extend_from_slice(&["cutover", "export", "--file", "local.db"]);
    assert_eq!(code(&sb.run(&args)), 2);
}

#[test]
fn cutover_refuses_sidecar_aliases_and_missing_dependency_in_dry_run() {
    let sb = Sandbox::new("cutover-invalid");
    let common = [
        "--store",
        "local.db",
        "--graph",
        "https://seeds.local/project/invalid",
    ];
    let call = |args: &[&str]| {
        let mut a = common.to_vec();
        a.extend_from_slice(args);
        sb.run(&a)
    };
    let alias = call(&[
        "cutover",
        "sync",
        "--file",
        "cursor.json.pending",
        "--base",
        "cursor.json",
    ]);
    assert_eq!(code(&alias), 2);
    assert!(!sb.work().join("local.db").exists());
    let item = serde_json::json!({"id":"br-one","title":"one","status":"open","priority":2,"issue_type":"task","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","dependencies":[{"depends_on_id":"br-missing","type":"blocks"}]});
    std::fs::write(sb.work().join("board.jsonl"), format!("{item}\n")).unwrap();
    let dry = call(&["cutover", "import", "--file", "board.jsonl", "--dry-run"]);
    assert!(!dry.status.success());
    assert!(String::from_utf8_lossy(&dry.stderr).contains("br-missing"));
    assert!(!sb.work().join("local.db").exists());
}

#[test]
fn cutover_comment_ids_survive_processes_and_dry_run_never_reserves() {
    let sb = Sandbox::new("cutover-comment-map");
    let common = [
        "--store",
        "local.db",
        "--graph",
        "https://seeds.local/project/comments",
    ];
    let call = |args: &[&str]| {
        let mut a = common.to_vec();
        a.extend_from_slice(args);
        sb.run(&a)
    };
    let ok = |args: &[&str]| {
        let out = call(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let first = ok(&["create", "first", "--silent"]).trim().to_string();
    ok(&["comments", "add", &first, "first comment"]);
    ok(&["cutover", "export", "--file", "dry.jsonl", "--dry-run"]);
    let maps = || {
        std::fs::read_dir(sb.work())
            .unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".comments.json")
                    .then_some(p)
            })
            .collect::<Vec<_>>()
    };
    assert!(maps().is_empty());
    assert!(!sb.work().join("dry.jsonl").exists());
    ok(&["cutover", "export", "--file", "first.jsonl"]);
    let read = |name: &str| {
        seeds::beads::parse(&std::fs::read_to_string(sb.work().join(name)).unwrap()).unwrap()
    };
    let before = read("first.jsonl");
    let second = ok(&["create", "second", "--silent"]).trim().to_string();
    ok(&["comments", "add", &second, "second comment"]);
    ok(&["cutover", "export", "--file", "second.jsonl"]);
    let after = read("second.jsonl");
    assert_eq!(before[&first], after[&first]);
    assert_ne!(
        after[&first]["comments"][0]["id"],
        after[&second]["comments"][0]["id"]
    );
    let paths = maps();
    assert_eq!(paths.len(), 1);
    std::fs::write(&paths[0], "invalid").unwrap();
    assert!(!call(&["cutover", "export", "--file", "refused.jsonl"])
        .status
        .success());
    assert!(!sb.work().join("refused.jsonl").exists());
}

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
    assert!(
        !sb.work().join(".seeds").exists(),
        "no local store was created"
    );

    // Reachable, but the server backend is not built: a distinct code.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let o = sb
        .cmd(&sb.work(), &["ready"])
        .env("SEEDS_QUIPU_URL", &url)
        .output()
        .unwrap();
    assert_eq!(code(&o), 20);
    assert!(!sb.work().join(".seeds").exists());
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

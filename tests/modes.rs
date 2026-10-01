//! The storage modes end to end: a repo-local pendant (mode 1), a remote quipu
//! server (mode 2), and sync between them (mode 3).
//!
//! The remote tests need a `quipu-server` binary and run only when
//! `SEEDS_TEST_QUIPU_SERVER` names one; CI's remote job installs it. Without
//! it they print that they were skipped, so a skip is visible, not silent.
#![cfg(feature = "native")]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::Value;

struct Env {
    root: PathBuf,
    servers: Vec<Child>,
}

impl Env {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("seeds-modes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.config")).unwrap();
        Self {
            root,
            servers: Vec::new(),
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let d = self.root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A project directory whose config keeps the ledger in `.seeds/pendant`.
    fn pendant_project(&self, name: &str) -> PathBuf {
        let d = self.dir(name);
        std::fs::create_dir_all(d.join(".seeds")).unwrap();
        std::fs::write(
            d.join(".seeds/config.toml"),
            "[pendant]\ndir = \".seeds/pendant\"\n",
        )
        .unwrap();
        d
    }

    fn sd(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
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
            "SEEDS_PENDANT_DIR",
            "SEEDS_SYNC_REMOTE",
            "SEEDS_QUIPU_TOKEN",
            "SEEDS_AGENT_NAME",
            "SEEDS_HARNESS",
            "SEEDS_MODEL",
            "BR_AGENT_NAME",
            "BR_HARNESS",
            "BR_MODEL",
        ] {
            c.env_remove(k);
        }
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
        let o = self.sd(cwd, args, env);
        assert!(
            o.status.success(),
            "sd {args:?} exited {:?}: {}",
            o.status.code(),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }

    fn ready(&self, cwd: &Path, env: &[(&str, &str)]) -> Vec<String> {
        let v: Value = serde_json::from_str(&self.ok(cwd, &["ready", "--json"], env)).unwrap();
        let mut ids: Vec<String> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    }

    /// Start a quipu server on a free port, if the test may.
    ///
    /// Ready means an HTTP 200 from `/health`, not a TCP connect: a loopback
    /// connect to a port nobody listens on yet can pick that same port as its
    /// source and connect to ITSELF, so "connected" happened while our server
    /// was still starting, and the test's first request was refused. A
    /// self-connected socket only echoes the request back, never a status line.
    /// And the probe port was released before the server bound it, so if our
    /// child exits (the port was taken), try again on a fresh one.
    fn start_server(&mut self) -> Option<String> {
        let bin = std::env::var("SEEDS_TEST_QUIPU_SERVER").ok()?;
        for attempt in 0..5 {
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let dir = self.dir(&format!("server{}-{attempt}", self.servers.len()));
            let mut child = Command::new(&bin)
                .args(["--db", dir.join("q.db").to_str().unwrap()])
                .args(["--bind", &format!("127.0.0.1:{port}")])
                .current_dir(&dir)
                .env("HOME", &dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut up = false;
            for _ in 0..100 {
                if !matches!(child.try_wait(), Ok(None)) {
                    break; // ours exited: the port was taken
                }
                if health_ok(port) && matches!(child.try_wait(), Ok(None)) {
                    up = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if up {
                self.servers.push(child);
                return Some(format!("http://127.0.0.1:{port}"));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        panic!("quipu-server did not start");
    }
}

/// Whether `GET /health` on the port returns an HTTP 200 status line.
fn health_ok(port: u16) -> bool {
    use std::io::{Read, Write};
    let Ok(mut c) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = c.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    if c.write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 64];
    let n = c.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    head.starts_with("HTTP/1.") && head.contains(" 200")
}

impl Drop for Env {
    fn drop(&mut self) {
        for mut c in self.servers.drain(..) {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

fn board(env: &Env, dir: &Path) -> (String, String) {
    let a = env
        .ok(
            dir,
            &["create", "Write the parser", "-p", "1", "--silent"],
            &[],
        )
        .trim()
        .to_string();
    let g = env
        .ok(dir, &["create", "Design the grammar", "--silent"], &[])
        .trim()
        .to_string();
    env.ok(dir, &["dep", "add", &a, &g], &[]);
    (a, g)
}

// ---------------------------------------------------------------- mode 1

#[test]
fn a_repo_local_pendant_is_written_on_every_change_and_a_copy_is_the_board() {
    let env = Env::new("pendant");
    let repo = env.pendant_project("repo");
    let (a, g) = board(&env, &repo);
    let pdir = repo.join(".seeds/pendant");
    for f in [
        "export.nt",
        "shapes.ttl",
        "manifest.json",
        "manifest.ttl",
        ".gitattributes",
    ] {
        assert!(pdir.join(f).is_file(), "{f}");
    }
    let gitignore = std::fs::read_to_string(repo.join(".seeds/.gitignore")).unwrap();
    assert!(gitignore.contains("seeds.db"));

    // "Clone": only what git would carry (config + pendant), no working store.
    let clone = env.dir("clone");
    std::fs::create_dir_all(clone.join(".seeds")).unwrap();
    std::fs::copy(
        repo.join(".seeds/config.toml"),
        clone.join(".seeds/config.toml"),
    )
    .unwrap();
    copy_dir(&pdir, &clone.join(".seeds/pendant"));
    let before: Vec<Vec<u8>> = ["export.nt", "manifest.json"]
        .iter()
        .map(|f| std::fs::read(clone.join(".seeds/pendant").join(f)).unwrap())
        .collect();
    assert_eq!(env.ready(&clone, &[]), vec![g.clone()]);
    let after: Vec<Vec<u8>> = ["export.nt", "manifest.json"]
        .iter()
        .map(|f| std::fs::read(clone.join(".seeds/pendant").join(f)).unwrap())
        .collect();
    assert_eq!(
        before, after,
        "reading a fresh clone leaves the pendant untouched"
    );

    // A change in the clone moves only the lines that changed.
    let old = std::fs::read_to_string(clone.join(".seeds/pendant/export.nt")).unwrap();
    env.ok(
        &clone,
        &["close", &g, "--reason", "done", "--outcome", "abandoned"],
        &[],
    );
    let new = std::fs::read_to_string(clone.join(".seeds/pendant/export.nt")).unwrap();
    let removed = old.lines().filter(|l| !new.contains(l)).count();
    let added = new.lines().filter(|l| !old.contains(l)).count();
    assert!(
        removed <= 4 && added <= 6,
        "small diff: -{removed} +{added}"
    );

    // "Pull" into the original: its next command loads the change.
    copy_dir(&clone.join(".seeds/pendant"), &pdir);
    let o = env.sd(&repo, &["ready", "--json"], &[]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("loaded the pendant"));
    assert_eq!(env.ready(&repo, &[]), vec![a]);
    // The close outcome travels in the pendant, not only the status.
    let shown = env.ok(&repo, &["show", &g, "--json"], &[]);
    assert!(
        shown.contains("\"outcome\": \"abandoned\"") || shown.contains("\"outcome\":\"abandoned\""),
        "{shown}"
    );
}

#[test]
fn when_store_and_pendant_both_changed_sd_refuses_and_says_how_to_choose() {
    let env = Env::new("both-changed");
    let repo = env.pendant_project("repo");
    let (a, _) = board(&env, &repo);
    let pdir = repo.join(".seeds/pendant");
    let snapshot = env.dir("elsewhere");
    copy_dir(&pdir, &snapshot);
    // Change the working store WITHOUT exporting (as a crash between the
    // commit and the export would, or a run that bypassed the project config).
    let db = repo.join(".seeds/seeds.db");
    let elsewhere = env.dir("no-config");
    let id = std::fs::read_to_string(repo.join(".seeds/project-id")).unwrap();
    let graph = format!("https://seeds.local/project/sd-{}", id.trim());
    env.ok(
        &elsewhere,
        &[
            "update",
            &a,
            "--add-label",
            "store-side",
            "--store",
            db.to_str().unwrap(),
            "--graph",
            &graph,
        ],
        &[],
    );
    // ...and change the pendant too.
    let other = env.pendant_project("other");
    copy_dir(&snapshot, &other.join(".seeds/pendant"));
    env.ok(&other, &["update", &a, "--add-label", "pendant-side"], &[]);
    copy_dir(&other.join(".seeds/pendant"), &pdir);
    let o = env.sd(&repo, &["ready"], &[]);
    assert_eq!(o.status.code(), Some(4));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("both the working store") && err.contains("--prefer"),
        "{err}"
    );
    // Choosing explicitly resolves it.
    env.ok(
        &repo,
        &["import", pdir.to_str().unwrap(), "--prefer", "pendant"],
        &[],
    );
    let v: Value = serde_json::from_str(&env.ok(&repo, &["show", &a, "--json"], &[])).unwrap();
    assert!(v[0]["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l == "pendant-side"));
}

#[test]
fn the_git_merge_driver_merges_fields_and_marks_real_conflicts() {
    let env = Env::new("driver");
    let base = env.pendant_project("base");
    let (a, g) = board(&env, &base);
    let base_nt = base.join(".seeds/pendant/export.nt");
    let fork = |name: &str| {
        let d = env.pendant_project(name);
        copy_dir(&base.join(".seeds/pendant"), &d.join(".seeds/pendant"));
        d
    };
    let ours = fork("ours");
    let theirs = fork("theirs");
    env.ok(&ours, &["update", &a, "--add-label", "parser"], &[]);
    env.ok(&theirs, &["close", &g, "--reason", "done"], &[]);
    let merged = env.dir("merge").join("export.nt");
    std::fs::copy(ours.join(".seeds/pendant/export.nt"), &merged).unwrap();
    let args = |m: &Path, t: &Path| {
        vec![
            "merge-driver".to_string(),
            base_nt.to_str().unwrap().to_string(),
            m.to_str().unwrap().to_string(),
            t.join(".seeds/pendant/export.nt")
                .to_str()
                .unwrap()
                .to_string(),
        ]
    };
    let a1 = args(&merged, &theirs);
    let a1: Vec<&str> = a1.iter().map(String::as_str).collect();
    env.ok(&env.root, &a1, &[]);
    let text = std::fs::read_to_string(&merged).unwrap();
    assert!(
        text.contains("\"parser\"") && text.contains("\"closed\""),
        "both sides kept"
    );

    // A real conflict: both set the parser's status.
    env.ok(&ours, &["update", &a, "--status", "deferred"], &[]);
    env.ok(&theirs, &["update", &a, "--status", "blocked"], &[]);
    std::fs::copy(ours.join(".seeds/pendant/export.nt"), &merged).unwrap();
    let o = env.sd(&env.root, &a1, &[]);
    assert_eq!(o.status.code(), Some(4));
    let text = std::fs::read_to_string(&merged).unwrap();
    assert!(
        text.starts_with("<<<<<<< sd merge-driver"),
        "marked, not resolved"
    );
    assert!(text.contains("status"), "{text}");
}

// ---------------------------------------------------------------- modes 2 and 3

#[test]
fn remote_mode_reads_and_writes_a_quipu_server() {
    let mut env = Env::new("remote");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
    let a = env
        .ok(&work, &["create", "remote parser", "--silent"], &remote)
        .trim()
        .to_string();
    let g = env
        .ok(&work, &["create", "remote grammar", "--silent"], &remote)
        .trim()
        .to_string();
    env.ok(&work, &["dep", "add", &a, &g], &remote);
    assert_eq!(env.ready(&work, &remote), vec![g.clone()]);
    env.ok(&work, &["comments", "add", &a, "over http"], &remote);
    env.ok(&work, &["close", &g, "--reason", "done"], &remote);
    assert_eq!(env.ready(&work, &remote), vec![a.clone()]);
    // Only the project id (which names the remote graph); no local store.
    assert!(
        !work.join(".seeds/seeds.db").exists(),
        "remote mode keeps no local store"
    );

    // Simultaneous claims through the server's compare-and-set: one winner.
    let procs: Vec<_> = (0..4)
        .map(|i| {
            let mut c = Command::new(env!("CARGO_BIN_EXE_sd"));
            c.args(["update", &a, "--claim", "--actor", &format!("racer{i}")])
                .current_dir(&work)
                .env("HOME", env.root.join("home"))
                .env("XDG_CONFIG_HOME", env.root.join("home/.config"))
                .env("SEEDS_QUIPU_URL", &url)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            c.spawn().unwrap()
        })
        .collect();
    let codes: Vec<i32> = procs
        .into_iter()
        .map(|p| p.wait_with_output().unwrap().status.code().unwrap_or(-1))
        .collect();
    assert_eq!(codes.iter().filter(|c| **c == 0).count(), 1, "{codes:?}");
    assert!(codes.iter().all(|c| *c == 0 || *c == 4), "{codes:?}");

    // Simultaneous creates of ONE workflow step through the server: every
    // caller names the same seed and exactly one exists. Process start-up is
    // slow enough that these rarely collide inside the server's compare-and-set
    // (measured: this passes with the race recovery disabled), so the proof of
    // the race path itself is core.rs's RacedBackend test; this is the
    // end-to-end check.
    let procs: Vec<_> = (0..6)
        .map(|_| {
            let mut c = Command::new(env!("CARGO_BIN_EXE_sd"));
            c.args([
                "create",
                "raced step",
                "--workflow-run",
                "r1",
                "--step",
                "s",
                "--silent",
            ])
            .current_dir(&work)
            .env("HOME", env.root.join("home"))
            .env("XDG_CONFIG_HOME", env.root.join("home/.config"))
            .env("SEEDS_QUIPU_URL", &url)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
            c.spawn().unwrap()
        })
        .collect();
    let mut ids = std::collections::BTreeSet::new();
    for p in procs {
        let o = p.wait_with_output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        ids.insert(String::from_utf8_lossy(&o.stdout).trim().to_string());
    }
    assert_eq!(ids.len(), 1, "{ids:?}");
    let listed = env.ok(&work, &["list", "--json"], &remote);
    assert_eq!(listed.matches("raced step").count(), 1, "{listed}");
}

#[test]
fn transcript_repo_local_export_import_into_a_remote_ready_agrees() {
    let mut env = Env::new("transcript");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    board(&env, &repo);
    let out = env.dir("out");
    env.ok(&repo, &["export", "--to", out.to_str().unwrap()], &[]);
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
    let work = env.dir("work");
    env.ok(&work, &["import", out.to_str().unwrap()], &remote);
    assert_eq!(env.ready(&repo, &[]), env.ready(&work, &remote));
    // Exporting the remote reproduces the same ledger bytes.
    let back = env.dir("back");
    env.ok(&work, &["export", "--to", back.to_str().unwrap()], &remote);
    assert_eq!(
        std::fs::read(out.join("export.nt")).unwrap(),
        std::fs::read(back.join("export.nt")).unwrap()
    );
}

#[test]
fn sync_merges_a_repo_local_ledger_with_a_remote_both_ways() {
    let mut env = Env::new("sync");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    let (a, g) = board(&env, &repo);
    let sync_env = [("SEEDS_SYNC_REMOTE", url.as_str())];
    env.ok(&repo, &["sync"], &sync_env);
    // Read the remote as the same project (its graph comes from repo's id).
    let id = std::fs::read_to_string(repo.join(".seeds/project-id")).unwrap();
    let graph = format!("https://seeds.local/project/sd-{}", id.trim());
    let remote = [
        ("SEEDS_QUIPU_URL", url.as_str()),
        ("SEEDS_GRAPH", graph.as_str()),
    ];
    let work = env.dir("work");
    assert_eq!(env.ready(&work, &remote), vec![g.clone()]);

    // Both sides change different fields; sync merges them.
    env.ok(&work, &["close", &g, "--reason", "done remotely"], &remote);
    env.ok(&repo, &["update", &a, "--add-label", "local"], &[]);
    env.ok(&repo, &["sync"], &sync_env);
    assert_eq!(env.ready(&repo, &[]), vec![a.clone()]);
    assert_eq!(env.ready(&work, &remote), vec![a.clone()]);
    let v: Value = serde_json::from_str(&env.ok(&work, &["show", &a, "--json"], &remote)).unwrap();
    assert!(v[0]["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l == "local"));

    // Both change the same field: reported, nothing written.
    env.ok(&work, &["update", &a, "--priority", "0"], &remote);
    env.ok(&repo, &["update", &a, "--priority", "3"], &[]);
    let o = env.sd(&repo, &["sync"], &sync_env);
    assert_eq!(o.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&o.stderr).contains("priority"));
}

// ---------------------------------------------------------------- review fixes (wu, seeds#2)

fn count_remote(env: &Env, cwd: &Path, url: &str) -> usize {
    let v: Value = serde_json::from_str(&env.ok(
        cwd,
        &["count", "--include-closed", "--json"],
        &[("SEEDS_QUIPU_URL", url)],
    ))
    .unwrap();
    v["count"].as_u64().unwrap() as usize
}

#[test]
fn syncing_to_a_second_empty_remote_does_not_delete_the_local_ledger() {
    // wu's blocker-1 repro, exactly: sync to A, then to an empty B.
    let mut env = Env::new("two-remotes");
    let (Some(a_url), Some(b_url)) = (env.start_server(), env.start_server()) else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    board(&env, &repo);
    let count_local = |env: &Env| -> u64 {
        let v: Value =
            serde_json::from_str(&env.ok(&repo, &["count", "--include-closed", "--json"], &[]))
                .unwrap();
        v["count"].as_u64().unwrap()
    };
    env.ok(&repo, &["sync", "--remote", &a_url], &[]);
    assert_eq!(count_remote(&env, &repo, &a_url), 2);
    let out = env.ok(&repo, &["sync", "--remote", &b_url, "--json"], &[]);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["local"]["removed"].as_array().unwrap().len(), 0, "{out}");
    assert_eq!(count_local(&env), 2, "the local ledger survives");
    assert_eq!(count_remote(&env, &repo, &b_url), 2, "B received it");

    // A remote that was reset (its seeds gone) is refused, naming the count.
    let empty = env.dir("empty-pendant");
    std::fs::write(empty.join("export.nt"), "").unwrap();
    env.ok(
        &repo,
        &["import", empty.to_str().unwrap(), "--replace"],
        &[("SEEDS_QUIPU_URL", a_url.as_str())],
    );
    assert_eq!(count_remote(&env, &repo, &a_url), 0);
    let o = env.sd(&repo, &["sync", "--remote", &a_url], &[]);
    assert_eq!(o.status.code(), Some(5));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("remove 2 seed(s) from the local store"),
        "{err}"
    );
    assert_eq!(count_local(&env), 2, "nothing was written");
    // Asked for explicitly, the removal goes through.
    env.ok(
        &repo,
        &["sync", "--remote", &a_url, "--allow-remote-deletes"],
        &[],
    );
    assert_eq!(count_local(&env), 0);
}

#[test]
fn a_corrupt_sync_base_is_an_error_not_an_empty_base() {
    let mut env = Env::new("corrupt-base");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    board(&env, &repo);
    env.ok(&repo, &["sync", "--remote", &url], &[]);
    let dir = repo.join(".seeds/seeds.db.sync");
    let base = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "nt"))
        .expect("a base keyed per remote");
    std::fs::write(&base, "<<<<<<< not a ledger\n").unwrap();
    let o = env.sd(&repo, &["sync", "--remote", &url], &[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unreadable"));
}

#[test]
fn a_second_sync_with_nothing_new_writes_nothing_on_either_side() {
    let mut env = Env::new("sync-noop");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    board(&env, &repo);
    let first: Value =
        serde_json::from_str(&env.ok(&repo, &["sync", "--remote", &url, "--json"], &[])).unwrap();
    assert_eq!(
        first["remote"]["wrote"], true,
        "the first sync writes the remote"
    );
    let second: Value =
        serde_json::from_str(&env.ok(&repo, &["sync", "--remote", &url, "--json"], &[])).unwrap();
    assert_eq!(second["remote"]["wrote"], false);
    assert_eq!(second["local"]["wrote"], false);
}

#[test]
fn two_repos_with_no_prefix_keep_separate_ledgers_on_one_server() {
    let mut env = Env::new("two-repos");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
    let one = env.dir("one");
    let two = env.dir("two");
    env.ok(&one, &["create", "only in one", "--silent"], &remote);
    env.ok(&two, &["create", "only in two", "--silent"], &remote);
    assert_eq!(env.ready(&one, &remote).len(), 1);
    assert_eq!(env.ready(&two, &remote).len(), 1);
    assert!(one.join(".seeds/project-id").is_file());
}

#[test]
fn remote_writes_record_who_made_them() {
    let mut env = Env::new("provenance");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let id = env
        .ok(
            &work,
            &["create", "attributed", "--silent", "--actor", "alice"],
            &[("SEEDS_QUIPU_URL", url.as_str())],
        )
        .trim()
        .to_string();
    let q = format!(
        "SELECT ?actor ?source WHERE {{ GRAPH ?g {{ ?w <https://seeds.local/ontology/wrote> \
         <https://seeds.local/item/{id}> ; <https://seeds.local/ontology/actor> ?actor ; \
         <https://seeds.local/ontology/source> ?source }} }}"
    );
    let body = serde_json::json!({ "query": q }).to_string();
    let text = ureq::post(&format!("{url}/query"))
        .set("Content-Type", "application/json")
        .send_string(&body)
        .unwrap()
        .into_string()
        .unwrap();
    assert!(
        text.contains("alice") && text.contains("seeds:create"),
        "{text}"
    );
}

// A proxy in front of the server that can lose the response to /update.
#[derive(Clone, Copy, PartialEq)]
enum Fault {
    /// Forward /update, then drop the connection without answering.
    LoseResponse,
    /// Answer /update with 502 without forwarding it.
    BadGateway,
}

fn faulty_proxy(upstream: &str, fault: Fault) -> String {
    use std::io::{Read, Write};
    let upstream = upstream.trim_start_matches("http://").to_string();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut client) = stream else { continue };
            let upstream = upstream.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let (head_end, len) = loop {
                    let n = client.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (i + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    let n = client.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let is_update = head.starts_with("POST /update");
                if is_update && fault == Fault::BadGateway {
                    let _ = client.write_all(
                        b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    return;
                }
                let head: String = head
                    .split("\r\n")
                    .filter(|l| !l.to_ascii_lowercase().starts_with("connection:"))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                let head = head.trim_end().to_string() + "\r\nConnection: close\r\n\r\n";
                let Ok(mut up) = std::net::TcpStream::connect(&upstream) else {
                    return;
                };
                let _ = up.write_all(head.as_bytes());
                let _ = up.write_all(&buf[head_end..]);
                let mut resp = Vec::new();
                let _ = up.read_to_end(&mut resp);
                if is_update && fault == Fault::LoseResponse {
                    return; // the write landed; the answer never arrives
                }
                let _ = client.write_all(&resp);
            });
        }
    });
    format!("http://{addr}")
}

#[test]
fn a_lost_response_is_read_back_and_never_duplicates_a_create() {
    let mut env = Env::new("lost-response");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    // The write lands, the response is lost: the read-back sees it landed.
    let lossy = faulty_proxy(&url, Fault::LoseResponse);
    let o = env.sd(
        &work,
        &["create", "exactly once", "--silent"],
        &[("SEEDS_QUIPU_URL", lossy.as_str())],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("landed"));
    assert_eq!(count_remote(&env, &work, &url), 1);

    // The gateway fails and nothing landed: INDETERMINATE (exit 8), not a
    // retry invitation, and nothing was written.
    let failing = faulty_proxy(&url, Fault::BadGateway);
    let o = env.sd(
        &work,
        &["create", "maybe", "--silent"],
        &[("SEEDS_QUIPU_URL", failing.as_str())],
    );
    assert_eq!(
        o.status.code(),
        Some(8),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("UNKNOWN") && err.contains("do not simply retry"),
        "{err}"
    );
    assert_eq!(count_remote(&env, &work, &url), 1);
}

#[test]
fn a_delete_syncs_both_ways_without_allow_remote_deletes() {
    // aegis-w3k75d.8 constraint 3: a delete is a tombstone, i.e. an ordinary
    // change, so sync carries it with no --allow-remote-deletes.
    let mut env = Env::new("sync-delete");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    let (a, g) = board(&env, &repo);
    let sync_env = [("SEEDS_SYNC_REMOTE", url.as_str())];
    env.ok(&repo, &["sync"], &sync_env);
    let id = std::fs::read_to_string(repo.join(".seeds/project-id")).unwrap();
    let graph = format!("https://seeds.local/project/sd-{}", id.trim());
    let remote = [
        ("SEEDS_QUIPU_URL", url.as_str()),
        ("SEEDS_GRAPH", graph.as_str()),
    ];
    let work = env.dir("work");
    // Local delete -> sync -> the remote sees a tombstone, not a removal.
    env.ok(&repo, &["delete", &g, "--force", "--reason", "dup"], &[]);
    let o = env.sd(&repo, &["sync"], &sync_env);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&env.ok(&work, &["show", &g, "--json"], &remote)).unwrap();
    assert_eq!(v[0]["status"], "tombstone");
    assert!(!env.ready(&work, &remote).contains(&g));
    // Remote delete -> sync -> the local store sees it too.
    env.ok(&work, &["delete", &a, "--force"], &remote);
    let o = env.sd(&repo, &["sync"], &sync_env);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_str(&env.ok(&repo, &["show", &a, "--json"], &[])).unwrap();
    assert_eq!(v[0]["status"], "tombstone");
    assert!(env.ready(&repo, &[]).is_empty());
}

#[test]
fn history_over_a_quipu_server_finds_each_version_without_a_head_tx() {
    // A quipu server reports no head transaction, and its /update returns no tx
    // (so remote writes print tx null; aegis-xajsgn). history gallops over --at
    // reads instead: every version it reports must be exactly what --at reads
    // at that tx, in order, one per write.
    let mut env = Env::new("remote-history");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
    let json = |args: &[&str]| -> Value {
        let mut a = args.to_vec();
        a.push("--json");
        serde_json::from_str(&env.ok(&work, &a, &remote)).unwrap()
    };
    let id = json(&["create", "first"])["id"]
        .as_str()
        .unwrap()
        .to_string();
    json(&["create", "noise one"]);
    json(&["update", &id, "--title", "second"]);
    json(&["create", "noise two"]);
    json(&["close", &id, "--reason", "done"]);
    let h = json(&["history", &id]);
    let versions = h["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 3, "{h}");
    let txs: Vec<u64> = versions.iter().map(|v| v["tx"].as_u64().unwrap()).collect();
    assert!(txs.windows(2).all(|w| w[0] < w[1]), "{txs:?}");
    for v in versions {
        let tx = v["tx"].as_u64().unwrap().to_string();
        let at = json(&["show", &id, "--at", &tx]);
        assert_eq!(at[0]["title"], v["title"]);
        assert_eq!(at[0]["status"], v["status"]);
        assert_eq!(at[0]["revision"], v["revision"]);
    }
    assert_eq!(versions[1]["title"], "second");
    assert_eq!(versions[2]["status"], "closed");
    assert!(h["meaning"]
        .as_str()
        .unwrap()
        .contains("transaction history"));
}

#[test]
fn a_remote_create_says_created_not_exists() {
    // wu, aegis-w3k75d.13: the create text branched on tx == 0 to mean "a keyed
    // create found the seed", and a remote store reports tx 0 for EVERY write,
    // so every remote create printed "exists".
    let mut env = Env::new("remote-created");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
    let text = env.ok(&work, &["create", "brand new"], &remote);
    assert!(text.starts_with("created "), "{text}");
    let j: Value =
        serde_json::from_str(&env.ok(&work, &["create", "another", "--json"], &remote)).unwrap();
    assert!(j["tx"].is_null(), "a remote write has no tx to report: {j}");
    // A keyed create that finds its seed still says so.
    let keyed = ["create", "step", "--workflow-run", "r1", "--step", "build"];
    assert!(env.ok(&work, &keyed, &remote).starts_with("created "));
    assert!(env.ok(&work, &keyed, &remote).starts_with("exists "));
}

// wu's conditions on attribution (aegis-w3k75d.13): the claims are identical in
// local and remote modes, flags beat SEEDS_* beats br's BR_* environment, a
// claim is recorded as `declared`, and a read verb refuses the flags.
#[test]
fn attribution_claims_read_back_identically_in_local_and_remote_modes() {
    let mut env = Env::new("claims");
    let run = |env: &Env, dir: &Path, extra: &[(&str, &str)]| -> Value {
        let e = |more: &[(&'static str, &'static str)]| -> Vec<(&str, &str)> {
            extra.iter().chain(more.iter()).copied().collect()
        };
        let id = env
            .ok(
                dir,
                &[
                    "create",
                    "x",
                    "--silent",
                    "--agent-name",
                    "gennaro",
                    "--harness",
                    "claude-code",
                ],
                &e(&[]),
            )
            .trim()
            .to_string();
        env.ok(dir, &["update", &id, "-p", "1"], &e(&[]));
        env.ok(
            dir,
            &["update", &id, "-p", "2"],
            &e(&[("BR_MODEL", "br-model"), ("SEEDS_MODEL", "seeds-model")]),
        );
        let h: Value =
            serde_json::from_str(&env.ok(dir, &["history", &id, "--json"], &e(&[]))).unwrap();
        Value::Array(
            h["versions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["claimed"].clone())
                .collect(),
        )
    };
    let want = serde_json::json!([
        {"agent_name": "gennaro", "harness": "claude-code", "model": null, "source_kind": "declared"},
        null,
        {"agent_name": null, "harness": null, "model": "seeds-model", "source_kind": "declared"},
    ]);
    let local = env.dir("local");
    assert_eq!(run(&env, &local, &[]), want);
    let o = env.sd(&local, &["list", "--model", "m"], &[]);
    assert_eq!(o.status.code(), Some(2), "a read verb refuses --model");

    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED (remote half): set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary");
        return;
    };
    let work = env.dir("remote-work");
    assert_eq!(run(&env, &work, &[("SEEDS_QUIPU_URL", url.as_str())]), want);
}

// Forward compatibility in remote mode (aegis-w3k75d.13): a fact whose
// predicate this sd does not model survives this sd's update of the seed.
// Released 0.0.2 erased it (measured: 1 row -> 0 after `update -p 1`).
#[test]
fn a_remote_update_keeps_facts_this_sd_does_not_model() {
    let mut env = Env::new("remote-forward-compat");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
    let id = env
        .ok(&work, &["create", "x", "--silent"], &remote)
        .trim()
        .to_string();
    let project = std::fs::read_to_string(work.join(".seeds/project-id")).unwrap();
    // The project graph as the SERVER names it (prefix-id), not re-derived.
    let graphs = Command::new("curl")
        .args(["-s", "-m", "10", &format!("{url}/graphs")])
        .output()
        .unwrap();
    let graphs: Value = serde_json::from_slice(&graphs.stdout).unwrap();
    let graph = graphs["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["iri"].as_str())
        .find(|g| g.ends_with(project.trim()))
        .expect("the project graph is registered")
        .to_string();
    let item = seeds::vocab::item_iri(&id);
    let pred = "https://seeds.local/ontology/fieldFromTheFuture";
    let post = |path: &str, ctype: &str, body: String| {
        Command::new("curl")
            .args([
                "-s",
                "-m",
                "10",
                "-X",
                "POST",
                "-H",
                &format!("content-type: {ctype}"),
            ])
            .arg("--data-binary")
            .arg(body)
            .arg(format!("{url}{path}"))
            .output()
            .unwrap()
    };
    let o = post(
        "/update",
        "application/sparql-update",
        format!("INSERT DATA {{ GRAPH <{graph}> {{ <{item}> <{pred}> \"kept\" }} }}"),
    );
    assert!(o.status.success());
    let rows = |p: &str| -> usize {
        let o = post(
            "/query",
            "application/json",
            serde_json::json!({
                "query": format!("SELECT ?v WHERE {{ GRAPH <{graph}> {{ <{item}> <{p}> ?v }} }}")
            })
            .to_string(),
        );
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        v["rows"].as_array().map_or(0, Vec::len)
    };
    let label = "http://www.w3.org/2000/01/rdf-schema#label";
    assert_eq!(
        (rows(pred), rows(label)),
        (1, 1),
        "control: both facts present"
    );
    env.ok(&work, &["update", &id, "-p", "1", "--title", "y"], &remote);
    assert_eq!(rows(pred), 1, "the update kept the fact it does not model");
    assert_eq!(rows(label), 1, "and replaced the one it does (title)");
    let shown: Value =
        serde_json::from_str(&env.ok(&work, &["show", &id, "--json"], &remote)).unwrap();
    assert_eq!(shown[0]["title"], "y");
}

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
            "SEEDS_SESSION",
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
                // Healthy AND ours: another process may already hold the
                // port, in which case /health answers for IT while our child
                // is still starting (and about to exit on the bind error).
                if health_ok(port)
                    && matches!(child.try_wait(), Ok(None))
                    && match listener_owned_by(child.id(), port) {
                        Some(ours) => ours,
                        // Linux always has /proc: an unreadable one is a
                        // failed check, not a pass.
                        None => !cfg!(target_os = "linux"),
                    }
                {
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

/// The body of `GET <path>` on the port (test helper, no TLS, no auth).
fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(c, "GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    out.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
}

/// Whether the TCP listener on loopback `port` belongs to process `pid`, from
/// `/proc` (Linux). `None` where `/proc` cannot say, so other platforms keep
/// the health-and-alive check alone.
fn listener_owned_by(pid: u32, port: u16) -> Option<bool> {
    let tcp = std::fs::read_to_string("/proc/net/tcp").ok()?;
    let want = format!(":{port:04X}");
    let inode = tcp.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() > 9 && f[1].ends_with(&want) && f[3] == "0A").then(|| f[9].to_string())
    })?;
    let socket = format!("socket:[{inode}]");
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    Some(
        fds.flatten()
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t.to_string_lossy() == socket)),
    )
}

/// `POST <path>` on the port with `body`; the response body (test helper, no
/// TLS, no auth).
fn http_post(port: u16, path: &str, content_type: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        c,
        "POST {path} HTTP/1.0\r\nHost: localhost\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    out.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
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
    // status, dateModified, version and the derived actionStatus change; a
    // close adds endTime, result and outcome.
    assert!(
        removed <= 5 && added <= 7,
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
    // Typed values over /update: the server stores the canonical form of a
    // duration (PT90M as PT1H30M), which must still read as the estimate,
    // and a due instant keeps its exact lexical form.
    env.ok(
        &work,
        &[
            "update",
            &a,
            "--estimate",
            "90",
            "--due",
            "2026-09-06T18:47:22.616951891Z",
        ],
        &remote,
    );
    let shown: Value =
        serde_json::from_str(&env.ok(&work, &["show", &a, "--json"], &remote)).unwrap();
    let shown = if shown.is_array() { &shown[0] } else { &shown };
    assert_eq!(shown["estimated_minutes"], 90, "{shown}");
    assert_eq!(shown["due_at"], "2026-09-06T18:47:22.616951891Z", "{shown}");
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
    let outs: Vec<(i32, String)> = procs
        .into_iter()
        .map(|p| {
            let o = p.wait_with_output().unwrap();
            (
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        })
        .collect();
    let codes: Vec<i32> = outs.iter().map(|(c, _)| *c).collect();
    assert_eq!(codes.iter().filter(|c| **c == 0).count(), 1, "{codes:?}");
    assert!(codes.iter().all(|c| *c == 0 || *c == 4), "{codes:?}");
    // Every loser is told WHO holds it (aegis-w3k75d.15), whether it lost in the
    // server's guard or saw the claim in its own read first.
    let winner = env.ok(&work, &["show", &a, "--json"], &remote);
    let winner: Value = serde_json::from_str(&winner).unwrap();
    let holder = winner[0]["assignee"]
        .as_str()
        .or(winner["assignee"].as_str())
        .unwrap()
        .to_string();
    for (code, err) in &outs {
        if *code == 4 {
            assert!(
                err.contains(&format!("claimed by {holder}")),
                "loser not told the holder: {err}"
            );
        }
    }
    // On a server that records the caller's actor (quipu >= #413), the winning
    // claim's transaction carries it instead of the constant "sparql-update".
    let port: u16 = url.rsplit(':').next().unwrap().parse().unwrap();
    let txs = http_get(port, "/transactions");
    if txs.contains("\"seeds\"") {
        assert!(
            txs.contains(&format!("\"actor\":\"{holder}\"")),
            "{holder} not on a tx: {txs}"
        );
    }

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
    /// Answer /update with 413 without forwarding it, as a server whose
    /// body limit is below seeds' own cap would.
    TooLarge,
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
                if is_update && fault == Fault::TooLarge {
                    let _ = client.write_all(
                        b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
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
    // A server that reports its transactions (quipu >= #411) gives the real tx;
    // an older one reports none, and sd says null rather than 0.
    assert!(
        j["tx"].is_null() || j["tx"].as_u64().is_some_and(|t| t > 0),
        "a remote write reports its tx or null: {j}"
    );
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
        // br's `close --session`: a claim like the others, flag beats env.
        env.ok(
            dir,
            &["close", &id, "--session", "sess-flag"],
            &e(&[("SEEDS_SESSION", "sess-env")]),
        );
        env.ok(dir, &["reopen", &id], &e(&[("SEEDS_SESSION", "sess-env")]));
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
        {"agent_name": "gennaro", "harness": "claude-code", "model": null, "session": null, "source_kind": "declared"},
        null,
        {"agent_name": null, "harness": null, "model": "seeds-model", "session": null, "source_kind": "declared"},
        {"agent_name": null, "harness": null, "model": null, "session": "sess-flag", "source_kind": "declared"},
        {"agent_name": null, "harness": null, "model": null, "session": "sess-env", "source_kind": "declared"},
    ]);
    let local = env.dir("local");
    assert_eq!(run(&env, &local, &[]), want);
    let o = env.sd(&local, &["list", "--model", "m"], &[]);
    assert_eq!(o.status.code(), Some(2), "a read verb refuses --model");
    let o = env.sd(&local, &["list", "--session", "s"], &[]);
    assert_eq!(o.status.code(), Some(2), "a read verb refuses --session");

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

// ---------------------------------------------------------------- sync previews (aegis-w3k75d.13)

#[test]
fn sync_dry_run_and_status_report_without_writing_anything() {
    let mut env = Env::new("sync-preview");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    let (a, g) = board(&env, &repo);
    let sync_env = [("SEEDS_SYNC_REMOTE", url.as_str())];
    let base_dir = repo.join(".seeds/seeds.db.sync");
    let status = |env: &Env| -> Value {
        serde_json::from_str(&env.ok(&repo, &["sync", "--status", "--json"], &sync_env)).unwrap()
    };

    // Never synced: local is ahead, and neither preview writes the remote or a base.
    let s = status(&env);
    assert_eq!(s["sync"]["state"], "local-ahead", "{s}");
    assert_eq!(s["sync"]["synced_before"], false, "{s}");
    let d: Value =
        serde_json::from_str(&env.ok(&repo, &["sync", "--dry-run", "--json"], &sync_env)).unwrap();
    assert_eq!(d["dry_run"], true);
    assert_eq!(d["remote"]["created"].as_array().unwrap().len(), 2, "{d}");
    assert_eq!(d["remote"]["wrote"], false, "{d}");
    assert_eq!(
        count_remote(&env, &repo, &url),
        0,
        "dry run wrote the remote"
    );
    assert!(
        !base_dir.exists() || std::fs::read_dir(&base_dir).unwrap().next().is_none(),
        "dry run wrote a sync base"
    );

    // Control: a real sync writes, and then both sides agree.
    env.ok(&repo, &["sync"], &sync_env);
    assert_eq!(count_remote(&env, &repo, &url), 2);
    assert_eq!(status(&env)["sync"]["state"], "in-sync");

    // The remote moves: remote-ahead; a dry run names the local update and
    // leaves the local store as it was.
    let id = std::fs::read_to_string(repo.join(".seeds/project-id")).unwrap();
    let graph = format!("https://seeds.local/project/sd-{}", id.trim());
    let remote = [
        ("SEEDS_QUIPU_URL", url.as_str()),
        ("SEEDS_GRAPH", graph.as_str()),
    ];
    let work = env.dir("work");
    env.ok(&work, &["close", &g, "--reason", "done remotely"], &remote);
    assert_eq!(status(&env)["sync"]["state"], "remote-ahead");
    let d: Value =
        serde_json::from_str(&env.ok(&repo, &["sync", "--dry-run", "--json"], &sync_env)).unwrap();
    assert_eq!(d["local"]["updated"], serde_json::json!([g.clone()]), "{d}");
    let v: Value = serde_json::from_str(&env.ok(&repo, &["show", &g, "--json"], &[])).unwrap();
    assert_eq!(v[0]["status"], "open", "dry run wrote the local store");

    // Both sides change one field differently: status reports it and exits 0;
    // a dry run fails exactly as the sync would (exit 4), writing nothing.
    env.ok(&work, &["update", &a, "--priority", "0"], &remote);
    env.ok(&repo, &["update", &a, "--priority", "3"], &[]);
    let s = status(&env);
    assert_eq!(s["sync"]["state"], "conflicted", "{s}");
    assert_eq!(s["sync"]["would_refuse"], true, "{s}");
    assert_eq!(s["sync"]["conflicts"][0]["id"], a.as_str(), "{s}");
    let o = env.sd(&repo, &["sync", "--dry-run"], &sync_env);
    assert_eq!(o.status.code(), Some(4));
    let o = env.sd(&repo, &["sync"], &sync_env);
    assert_eq!(
        o.status.code(),
        Some(4),
        "control: the sync refuses the same way"
    );

    // The two previews are alternatives.
    let o = env.sd(&repo, &["sync", "--dry-run", "--status"], &sync_env);
    assert_eq!(o.status.code(), Some(2));
}

// ---------------------------------------------------------------- update overwrite guard, remote (aegis-w3k75d.13)

#[test]
fn update_refuses_replacing_text_without_force_on_a_remote_ledger_too() {
    // wu's amendment: the guard is in the engine path every backend shares,
    // so a quipu server refuses exactly as a local store does.
    let mut env = Env::new("update-force-remote");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let remote = [
        ("SEEDS_QUIPU_URL", url.as_str()),
        ("SEEDS_GRAPH", "https://seeds.local/project/update-force"),
    ];
    let a = env
        .ok(&work, &["create", "a", "--silent"], &remote)
        .trim()
        .to_string();
    let get = |env: &Env, key: &str| -> Value {
        let v: Value =
            serde_json::from_str(&env.ok(&work, &["show", &a, "--json"], &remote)).unwrap();
        v[0][key].clone()
    };
    for (field, key) in [("--description", "description"), ("--notes", "notes")] {
        env.ok(&work, &["update", &a, field, "first"], &remote);
        env.ok(&work, &["update", &a, field, "first"], &remote);
        let o = env.sd(&work, &["update", &a, field, "second"], &remote);
        assert_eq!(o.status.code(), Some(5), "{field}");
        assert_eq!(get(&env, key), "first", "{field}: nothing was written");
        env.ok(&work, &["update", &a, field, "second", "--force"], &remote);
        assert_eq!(get(&env, key), "second");
    }
}

// ---------------------------------------------------------------- typed literals over a remote (wu's review of seeds#73)

#[test]
fn typed_literals_a_newer_sd_wrote_cross_a_quipu_server_with_their_types() {
    // Local store -> sd sync -> quipu server -> sd sync -> a second local
    // store. Exercises the remote WRITE (SPARQL text) and READ (SPARQL JSON)
    // of a boolean, an xsd:date and a language-tagged literal.
    use quipu::store::{Datum, Store};
    use quipu::types::{Op, Value as Q};
    let mut env = Env::new("typed-remote");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let graph = "https://seeds.local/project/typed-remote";
    let future = "https://seeds.local/ontology/fieldFromTheFuture";
    let local = [("SEEDS_GRAPH", graph), ("SEEDS_SYNC_REMOTE", url.as_str())];
    let one = env.dir("one");
    let id = env
        .ok(&one, &["create", "typed", "--silent"], &local)
        .trim()
        .to_string();
    let iri = seeds::vocab::item_iri(&id);
    let want = [
        Q::Bool(true),
        Q::Typed {
            lexical: "2026-10-01".into(),
            datatype: "http://www.w3.org/2001/XMLSchema#date".into(),
        },
        Q::Lang {
            lexical: "bonjour".into(),
            lang: "fr".into(),
        },
    ];
    {
        let mut st = Store::open(one.join(".seeds/seeds.db").to_str().unwrap()).unwrap();
        let g = st.graph_create(graph).unwrap();
        let e = st.intern(&iri).unwrap();
        let a = st.intern(future).unwrap();
        let datums: Vec<Datum> = want
            .iter()
            .map(|v| Datum {
                entity: e,
                attribute: a,
                value: v.clone(),
                valid_from: "2026-10-01T00:00:00Z".into(),
                valid_to: None,
                op: Op::Assert,
            })
            .collect();
        st.transact_to_graph(
            &datums,
            "2026-10-01T00:00:00Z",
            Some("newer-sd"),
            Some("test"),
            g,
        )
        .unwrap();
    }
    env.ok(&one, &["sync"], &local);
    let two = env.dir("two");
    env.ok(&two, &["sync"], &local);
    let got = |dir: &Path| -> Vec<String> {
        let st = Store::open_read_only(dir.join(".seeds/seeds.db").to_str().unwrap()).unwrap();
        let g = st.graph_create(graph).unwrap();
        let (Some(e), Some(a)) = (st.lookup(&iri).unwrap(), st.lookup(future).unwrap()) else {
            return vec![];
        };
        let mut v: Vec<String> = st
            .entity_facts_in_graph(e, g)
            .unwrap()
            .into_iter()
            .filter(|f| f.attribute == a)
            .map(|f| format!("{:?}", f.value))
            .collect();
        v.sort();
        v
    };
    let mut expect: Vec<String> = want.iter().map(|v| format!("{v:?}")).collect();
    expect.sort();
    assert_eq!(got(&one), expect, "control: planted in the first store");
    assert_eq!(
        got(&two),
        expect,
        "arrived through the server with their types"
    );
}

#[test]
fn remote_mode_keeps_ephemeral_seeds_in_their_own_graph() {
    // aegis-w3k75d.13, lead ruling (a), through a real quipu-server: the
    // ephemeral graph is created on first use, read with the project graph,
    // never ready, never exported, and a shared seed cannot depend on it.
    let mut env = Env::new("remote-ephemeral");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
    let s = env
        .ok(&work, &["create", "shared", "--silent"], &remote)
        .trim()
        .to_string();
    let e = env
        .ok(
            &work,
            &["create", "eph", "--ephemeral", "--silent"],
            &remote,
        )
        .trim()
        .to_string();
    let show = |id: &str| -> Value {
        serde_json::from_str(&env.ok(&work, &["show", id, "--json"], &remote)).unwrap()
    };
    assert_eq!(show(&e)[0]["ephemeral"], true);
    assert_eq!(show(&s)[0]["ephemeral"], false);
    assert_eq!(env.ready(&work, &remote), vec![s.clone()]);

    // Writes to it stay in its graph, comments included.
    env.ok(&work, &["update", &e, "--title", "eph 2"], &remote);
    env.ok(&work, &["comments", "add", &e, "over http"], &remote);
    assert_eq!(show(&e)[0]["title"], "eph 2");
    assert_eq!(show(&e)[0]["ephemeral"], true);

    // Shared -> ephemeral is refused; ephemeral -> shared is fine.
    let o = env.sd(&work, &["dep", "add", &s, &e], &remote);
    assert_eq!(
        o.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    env.ok(&work, &["dep", "add", &e, &s], &remote);

    // The export carries the shared seed (control), not the ephemeral one.
    let dir = env.root.join("pendant");
    env.ok(&work, &["export", "--to", dir.to_str().unwrap()], &remote);
    let nt = std::fs::read_to_string(dir.join("export.nt")).unwrap();
    assert!(
        nt.contains(&format!("/item/{s}>")),
        "control: shared seed exported"
    );
    assert!(!nt.contains(&format!("/item/{e}")), "ephemeral seed leaked");
}

// ---------------------------------------------- a push over the request limit (aegis-w3k75d.15)

#[test]
fn a_sync_over_the_request_limit_lands_in_batches_on_a_real_server() {
    let mut env = Env::new("batched-sync");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    let (a, g) = board(&env, &repo);
    let mut prev = g.clone();
    for i in 0..10 {
        let id = env
            .ok(&repo, &["q", &format!("step {i}")], &[])
            .trim()
            .to_string();
        env.ok(&repo, &["dep", "add", &id, &prev], &[]);
        env.ok(&repo, &["comments", "add", &id, &format!("note {i}")], &[]);
        prev = id;
    }
    let sync_env = [
        ("SEEDS_SYNC_REMOTE", url.as_str()),
        ("SEEDS_MAX_WRITE_BYTES", "60000"),
        ("SEEDS_MAX_WRITE_CLAUSES", "8"),
    ];
    let o = env.sd(&repo, &["sync"], &sync_env);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(0), "{err}");
    let batches = err.matches("remote batch").count();
    assert!(batches >= 3, "pushed in batches: {err}");
    let id = std::fs::read_to_string(repo.join(".seeds/project-id")).unwrap();
    let graph = format!("https://seeds.local/project/sd-{}", id.trim());
    let remote = [
        ("SEEDS_QUIPU_URL", url.as_str()),
        ("SEEDS_GRAPH", graph.as_str()),
    ];
    let work = env.dir("work");
    // Every seed, every edge: the ready set is the head of the chain.
    assert_eq!(env.ready(&work, &remote), env.ready(&repo, &[]));
    assert_eq!(env.ready(&work, &remote), vec![g.clone()]);
    // Comments crossed with their seeds: the last step's note, on both sides.
    for (cwd, e) in [(&repo, &[][..]), (&work, &remote[..])] {
        let v: Value = serde_json::from_str(&env.ok(cwd, &["show", &prev, "--json"], e)).unwrap();
        let texts: Vec<&str> = v[0]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["note 9"]);
    }
    assert!(env.ready(&work, &remote).iter().all(|id| *id != a));
    // A second sync has nothing to push.
    let o = env.sd(&repo, &["sync", "--json"], &sync_env);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["remote"]["wrote"], false, "{v}");
}

#[test]
fn a_write_over_the_request_limit_is_refused_unsent() {
    let mut env = Env::new("too-large");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let o = env.sd(
        &work,
        &["create", "far too big for this limit", "--silent"],
        &[
            ("SEEDS_QUIPU_URL", url.as_str()),
            ("SEEDS_MAX_WRITE_BYTES", "200"),
        ],
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(5), "{err}");
    assert!(
        err.contains("write too large") && err.contains("nothing was sent"),
        "{err}"
    );
    assert_eq!(count_remote(&env, &work, &url), 0);
    let o = env.sd(
        &work,
        &["create", "x"],
        &[
            ("SEEDS_QUIPU_URL", url.as_str()),
            ("SEEDS_MAX_WRITE_BYTES", "lots"),
        ],
    );
    assert_eq!(o.status.code(), Some(6), "a bad limit is a config error");
    // A create nests two guard clauses (absent from both graphs).
    let o = env.sd(
        &work,
        &["create", "two guards", "--silent"],
        &[
            ("SEEDS_QUIPU_URL", url.as_str()),
            ("SEEDS_MAX_WRITE_CLAUSES", "1"),
        ],
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(5), "{err}");
    assert!(
        err.contains("guard clauses") && err.contains("nothing was sent"),
        "{err}"
    );
    assert_eq!(count_remote(&env, &work, &url), 0);
}

#[test]
fn a_413_is_a_definite_refusal_not_an_unknown_outcome() {
    let mut env = Env::new("http-413");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let work = env.dir("work");
    let refusing = faulty_proxy(&url, Fault::TooLarge);
    let o = env.sd(
        &work,
        &["create", "refused for size", "--silent"],
        &[("SEEDS_QUIPU_URL", refusing.as_str())],
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(5), "{err}");
    assert!(err.contains("413") && !err.contains("UNKNOWN"), "{err}");
    assert_eq!(count_remote(&env, &work, &url), 0);
}

// ---------------------------------------------------------------- canonical times (wu, aegis-bqgdr3)

/// Percent-encode a form value.
fn form(v: &str) -> String {
    let mut out = String::new();
    for b in v.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// seeds writes the XSD canonical form of every time and duration, and a
/// quipu server's /update stores typed literals in canonical form too. This
/// pins the two together: raw lexicals go to the server, and what it stores
/// must equal what seeds would have written. The corpus is the edge cases
/// below plus, when `SEEDS_TEST_TIME_CORPUS` names a file of
/// `dateTime|date|duration<TAB>lexical` lines, every one of those.
#[test]
fn seeds_canonical_times_equal_a_quipu_servers() {
    let mut env = Env::new("canonical-times");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let port: u16 = url.rsplit(':').next().unwrap().parse().unwrap();
    let mut corpus: Vec<(String, String)> = [
        ("dateTime", "2026-08-30T13:35:59.014436677Z"),
        ("dateTime", "2026-08-30T13:35:59.014436670Z"),
        ("dateTime", "2026-08-30T13:35:59.100000000Z"),
        ("dateTime", "2026-10-03T14:00:00.500Z"),
        ("dateTime", "2026-10-03T14:00:00.000Z"),
        ("dateTime", "2026-10-03T14:00:00.0Z"),
        ("dateTime", "2026-10-03T14:00:00Z"),
        ("dateTime", "2026-10-03T14:00:00+00:00"),
        ("dateTime", "2026-10-03T14:00:00-00:00"),
        ("dateTime", "2026-10-03T14:00:00+02:00"),
        ("dateTime", "2026-10-03T14:00:00-05:30"),
        ("dateTime", "2026-10-03T14:00:00+14:00"),
        ("dateTime", "2026-10-03T14:00:00"),
        ("dateTime", "2026-10-03T14:00:00.250"),
        ("dateTime", "2026-10-03T24:00:00Z"),
        ("dateTime", "2026-12-31T24:00:00Z"),
        ("dateTime", "2024-02-29T23:59:59.999999999Z"),
        ("date", "2026-10-03"),
        ("date", "2024-02-29"),
        ("duration", "PT0M"),
        ("duration", "PT1M"),
        ("duration", "PT30M"),
        ("duration", "PT59M"),
        ("duration", "PT60M"),
        ("duration", "PT90M"),
        ("duration", "PT1440M"),
        ("duration", "PT1441M"),
        ("duration", "PT100000M"),
    ]
    .iter()
    .map(|(t, v)| (t.to_string(), v.to_string()))
    .collect();
    let extra = std::env::var("SEEDS_TEST_TIME_CORPUS").ok();
    if let Some(path) = &extra {
        for l in std::fs::read_to_string(path).unwrap().lines() {
            if let Some((t, v)) = l.split_once('\t') {
                corpus.push((t.to_string(), v.to_string()));
            }
        }
    }
    let ours = |t: &str, v: &str| -> String {
        match t {
            "dateTime" => seeds::model::canonical_time(v, false),
            "date" => seeds::model::canonical_time(v, true),
            _ => seeds::model::canonical_duration(v),
        }
        .unwrap_or_else(|| panic!("seeds refuses {t} {v:?}"))
    };
    let mut checked = 0;
    let mut differ = Vec::new();
    for (n, chunk) in corpus.chunks(2000).enumerate() {
        let g = format!("urn:seeds-test:canonical:{n}");
        http_post(
            port,
            "/graph/create",
            "application/json",
            &format!("{{\"graph\":\"{g}\"}}"),
        );
        let triples: String = chunk
            .iter()
            .enumerate()
            .map(|(i, (t, v))| {
                format!("<urn:s:{i}> <urn:p> \"{v}\"^^<http://www.w3.org/2001/XMLSchema#{t}> . ")
            })
            .collect();
        let report = http_post(
            port,
            "/update",
            "application/x-www-form-urlencoded",
            &format!(
                "update={}",
                form(&format!("INSERT DATA {{ GRAPH <{g}> {{ {triples}}} }}"))
            ),
        );
        assert!(
            report.contains("\"asserted\""),
            "the update landed: {report}"
        );
        let q =
            format!("{{\"query\":\"SELECT ?s ?o WHERE {{ GRAPH <{g}> {{ ?s <urn:p> ?o }} }}\"}}");
        let body: Value = serde_json::from_str(&http_post(port, "/query", "application/json", &q))
            .expect("query JSON");
        let rows = body["rows"]
            .as_array()
            .or_else(|| body["results"]["bindings"].as_array())
            .expect("rows");
        let text = |v: &Value| -> String {
            v.get("value")
                .and_then(Value::as_str)
                .or_else(|| v.as_str())
                .unwrap()
                .to_string()
        };
        let stored: std::collections::BTreeMap<String, String> = rows
            .iter()
            .map(|r| (text(&r["s"]), text(&r["o"])))
            .collect();
        for (i, (t, v)) in chunk.iter().enumerate() {
            let server = stored.get(&format!("urn:s:{i}")).expect("stored");
            checked += 1;
            if *server != ours(t, v) {
                differ.push(format!(
                    "{t} {v:?}: server {server:?}, seeds {:?}",
                    ours(t, v)
                ));
            }
        }
    }
    eprintln!("canonical forms checked against the server: {checked}");
    assert!(
        differ.is_empty(),
        "{} differ:\n{}",
        differ.len(),
        differ.join("\n")
    );
    // Positive control: the corpus does exercise canonicalisation.
    assert_eq!(ours("duration", "PT90M"), "PT1H30M");
    assert_eq!(
        ours("dateTime", "2026-10-03T14:00:00.500Z"),
        "2026-10-03T14:00:00.5Z"
    );
}

/// A time with trailing fractional zeros, written locally and synced through
/// a real server: local and remote hold the same value in every field, and a
/// second sync writes nothing on either side.
#[test]
fn a_canonical_time_round_trips_through_a_server_with_no_drift() {
    let mut env = Env::new("canonical-sync");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    let id = env
        .ok(
            &repo,
            &["create", "zeros", "--silent", "--estimate", "90"],
            &[],
        )
        .trim()
        .to_string();
    env.ok(
        &repo,
        &[
            "update",
            &id,
            "--due",
            "2026-09-06T18:47:22.616951890Z",
            "--defer",
            "2026-10-03T14:00:00.500+00:00",
        ],
        &[],
    );
    let local: Value = serde_json::from_str(&env.ok(&repo, &["show", &id, "--json"], &[])).unwrap();
    assert_eq!(
        local[0]["due_at"], "2026-09-06T18:47:22.61695189Z",
        "{local}"
    );
    assert_eq!(local[0]["defer_until"], "2026-10-03T14:00:00.5Z", "{local}");
    assert_eq!(local[0]["estimated_minutes"], 90);

    let first: Value =
        serde_json::from_str(&env.ok(&repo, &["sync", "--remote", &url, "--json"], &[])).unwrap();
    assert_eq!(first["remote"]["wrote"], true, "{first}");
    let pid = std::fs::read_to_string(repo.join(".seeds/project-id")).unwrap();
    let graph = format!("https://seeds.local/project/sd-{}", pid.trim());
    let remote_env = [
        ("SEEDS_QUIPU_URL", url.as_str()),
        ("SEEDS_GRAPH", graph.as_str()),
    ];
    let work = env.dir("work");
    let remote: Value =
        serde_json::from_str(&env.ok(&work, &["show", &id, "--json"], &remote_env)).unwrap();
    let local: Value = serde_json::from_str(&env.ok(&repo, &["show", &id, "--json"], &[])).unwrap();
    for (k, v) in local[0].as_object().unwrap() {
        assert_eq!(&remote[0][k], v, "{k} differs between local and remote");
    }
    let second: Value =
        serde_json::from_str(&env.ok(&repo, &["sync", "--remote", &url, "--json"], &[])).unwrap();
    assert_eq!(second["remote"]["wrote"], false, "{second}");
    assert_eq!(second["local"]["wrote"], false, "{second}");
    let after: Value = serde_json::from_str(&env.ok(&repo, &["show", &id, "--json"], &[])).unwrap();
    assert_eq!(after, local, "the local seed did not change");
}

/// Comments that cross in batches WITHOUT their seed are validated with the
/// parent read from the TARGET store, so the comment shape's
/// `schema:parentItem sh:class schema:Action` holds on the switched model
/// under real SHACL. Both seed-less paths, at 4 guard clauses a request:
///
/// 1. a NEW seed with 12 comments: the comments past the limit follow the
///    seed in later batches (seeds#95);
/// 2. 12 comments added to a seed the remote already holds.
#[test]
fn comments_past_the_clause_limit_cross_in_batches_with_their_parent_from_the_remote() {
    let mut env = Env::new("batched-comments");
    let Some(url) = env.start_server() else {
        eprintln!("SKIPPED: set SEEDS_TEST_QUIPU_SERVER to a quipu-server binary to run this");
        return;
    };
    let repo = env.pendant_project("repo");
    let sync_env = [
        ("SEEDS_SYNC_REMOTE", url.as_str()),
        ("SEEDS_MAX_WRITE_CLAUSES", "4"),
    ];
    let synced = |what: &str| {
        let o = env.sd(&repo, &["sync"], &sync_env);
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        assert_eq!(o.status.code(), Some(0), "{what}: {err}");
        assert!(
            err.matches("remote batch").count() >= 3,
            "{what}: 12 comments at 4 clauses a request cross in batches: {err}"
        );
    };
    let fresh = env
        .ok(&repo, &["create", "new with comments", "--silent"], &[])
        .trim()
        .to_string();
    for i in 0..12 {
        env.ok(
            &repo,
            &["comments", "add", &fresh, &format!("note {i}")],
            &[],
        );
    }
    let held = env
        .ok(&repo, &["create", "comments later", "--silent"], &[])
        .trim()
        .to_string();
    synced("a new seed past the limit");
    for i in 0..12 {
        env.ok(
            &repo,
            &["comments", "add", &held, &format!("note {i}")],
            &[],
        );
    }
    synced("comments on a seed the remote holds");

    let pid = std::fs::read_to_string(repo.join(".seeds/project-id")).unwrap();
    let graph = format!("https://seeds.local/project/sd-{}", pid.trim());
    let remote = [
        ("SEEDS_QUIPU_URL", url.as_str()),
        ("SEEDS_GRAPH", graph.as_str()),
    ];
    let work = env.dir("work");
    let texts = |seed: &str, cwd: &Path, e: &[(&str, &str)]| -> Vec<String> {
        let v: Value = serde_json::from_str(&env.ok(cwd, &["show", seed, "--json"], e)).unwrap();
        v[0]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap().to_string())
            .collect()
    };
    let want: Vec<String> = (0..12).map(|i| format!("note {i}")).collect();
    let port: u16 = url.rsplit(':').next().unwrap().parse().unwrap();
    for seed in [&fresh, &held] {
        assert_eq!(texts(seed, &repo, &[]), want);
        assert_eq!(
            texts(seed, &work, &remote),
            want,
            "every comment on the remote"
        );
        // On the switched model: the remote holds them as schema:Comment.
        let q = format!(
            "{{\"query\":\"SELECT ?c WHERE {{ GRAPH <{graph}> {{ ?c a <https://schema.org/Comment> ; \
             <https://schema.org/parentItem> <{}> }} }}\"}}",
            seeds::vocab::item_iri(seed)
        );
        let body = http_post(port, "/query", "application/json", &q);
        assert_eq!(body.matches("/comment/").count(), 12, "{seed}: {body}");
    }
    // Nothing left to push.
    let o = env.sd(&repo, &["sync", "--json"], &sync_env);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["remote"]["wrote"], false, "{v}");
}

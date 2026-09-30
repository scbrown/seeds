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
    server: Option<Child>,
}

impl Env {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("seeds-modes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.config")).unwrap();
        Self { root, server: None }
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
    fn start_server(&mut self) -> Option<String> {
        let bin = std::env::var("SEEDS_TEST_QUIPU_SERVER").ok()?;
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir = self.dir("server");
        let child = Command::new(bin)
            .args(["--db", dir.join("q.db").to_str().unwrap()])
            .args(["--bind", &format!("127.0.0.1:{port}")])
            .current_dir(&dir)
            .env("HOME", &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        self.server = Some(child);
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Some(format!("http://127.0.0.1:{port}"));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("quipu-server did not start");
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(mut c) = self.server.take() {
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
    env.ok(&clone, &["close", &g, "--reason", "done"], &[]);
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
    env.ok(
        &elsewhere,
        &[
            "update",
            &a,
            "--add-label",
            "store-side",
            "--store",
            db.to_str().unwrap(),
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
    assert!(
        !work.join(".seeds").exists(),
        "remote mode writes nothing locally"
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
    let remote = [("SEEDS_QUIPU_URL", url.as_str())];
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

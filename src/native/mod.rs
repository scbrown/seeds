//! The native `sd` CLI: arguments, configuration, the store file, the clock.
//!
//! Everything in this module may touch the filesystem, the environment and the
//! system clock; nothing outside it may (enforced by `clippy.toml`'s
//! disallowed lists, which this module opts out of). It turns a parsed
//! [`cli::Cli`] into calls on [`crate::engine`] and prints the result.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod cli;
pub mod config;
pub mod remote;
pub mod store;

use serde_json::Value as Json;

use crate::backend::{Backend, Ctx};
use crate::engine::{self, Filter};
use crate::error::{ErrorKind, Result, SdError};
use crate::output;
use crate::pendant;
use crate::quipu_backend::QuipuBackend;
use crate::sync;
use cli::{Cli, Command, CommentsCommand, ConfigCommand, DepCommand, EpicCommand, LabelCommand};
use config::{Location, Resolved};

/// What a run produced: the exit code and the two streams.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Process exit code.
    pub code: i32,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

/// Run a parsed command against the configuration in the environment.
pub fn run(cli: &Cli) -> Outcome {
    if let Command::MergeDriver(a) = &cli.command {
        // Needs no store and no configuration: git runs it mid-merge.
        return match merge_driver(a) {
            Ok(o) => o,
            Err(e) => error_outcome(cli.json, &e, Vec::new()),
        };
    }
    if let Some(pointer) = mapped_pointer(&cli.command) {
        let e = SdError::new(ErrorKind::Elsewhere, pointer);
        return error_outcome(cli.wants_json(), &e, Vec::new());
    }
    if let Command::Completions(a) = &cli.command {
        // Needs no store and no configuration.
        return match completions(a) {
            Ok(o) => o,
            Err(e) => error_outcome(cli.json, &e, Vec::new()),
        };
    }
    if let Command::Capabilities { command_path } = &cli.command {
        return match capabilities(command_path.as_deref()) {
            Ok(v) => ok(
                cli.json,
                v.clone(),
                serde_json::to_string_pretty(&v).unwrap_or_default(),
                vec![],
            ),
            Err(e) => error_outcome(cli.json, &e, Vec::new()),
        };
    }
    if let Command::Schema { target } = &cli.command {
        // Needs no store and no configuration. The output is JSON either way.
        let value = if target == "all" {
            Some(crate::schema::all(env!("CARGO_PKG_VERSION")))
        } else {
            crate::schema::target(target)
        };
        return match value {
            Some(v) => ok(
                cli.json,
                v.clone(),
                serde_json::to_string_pretty(&v).unwrap_or_default(),
                vec![],
            ),
            None => {
                let names: Vec<&str> = crate::schema::TARGETS.iter().map(|(n, _)| *n).collect();
                let e = SdError::usage(format!(
                    "unknown schema {target:?}; one of all, {}",
                    names.join(", ")
                ));
                error_outcome(cli.json, &e, Vec::new())
            }
        };
    }
    if let Command::Version(a) = &cli.command {
        // Needs no store and no configuration.
        return version(cli.json, a.short);
    }
    if let Command::Init(a) = &cli.command {
        return match init(cli, a) {
            Ok(o) => o,
            Err(e) => error_outcome(cli.json, &e, Vec::new()),
        };
    }
    let result = config::Inputs::from_env(cli.store.clone(), cli.quipu.clone(), cli.graph.clone())
        .and_then(|i| config::resolve(&i).map(|cfg| (i, cfg)))
        .and_then(|(inputs, cfg)| match &cli.command {
            Command::Config { command } => config_outcome(cli.json, &inputs, &cfg, command),
            // Reads only the configuration: never creates a project id or a store.
            Command::Where => Ok(where_outcome(cli.json, &cfg)),
            _ => run_with(cli, &cfg),
        });
    match result {
        Ok(o) => o,
        Err(e) => error_outcome(cli.json, &e, Vec::new()),
    }
}

/// `sd version`: br's keys. seeds does not embed its commit, branch or
/// compiler, so those are null rather than guessed.
fn version(json: bool, short: bool) -> Outcome {
    let v = env!("CARGO_PKG_VERSION");
    let mut features = vec!["native"];
    if cfg!(feature = "shacl") {
        features.push("shacl");
    }
    let value = serde_json::json!({
        "version": v,
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "commit": null, "branch": null, "rust_version": null,
        "target": format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        "features": features,
    });
    let text = if short {
        v.to_string()
    } else {
        format!("sd {v}")
    };
    ok(json, value, text, vec![])
}

fn location_parts(cfg: &Resolved) -> (Option<String>, Option<String>, &'static str) {
    match &cfg.location {
        Location::Store(p) => (Some(p.display().to_string()), None, "local"),
        Location::Url(u) => (None, Some(u.clone()), "remote"),
    }
}

/// `sd where`: where the ledger lives, from the configuration alone.
fn where_outcome(json: bool, cfg: &Resolved) -> Outcome {
    let (store, url, mode) = location_parts(cfg);
    let dir = cfg
        .project_id_file
        .parent()
        .map(|p| p.display().to_string());
    let pendant = cfg.pendant.as_ref().map(|p| p.display().to_string());
    let value = serde_json::json!({
        "path": dir, "prefix": cfg.prefix, "database_path": store, "jsonl_path": null,
        "quipu_url": url, "mode": mode, "graph": cfg.graph, "pendant_path": pendant,
        "location_source": cfg.location_source,
    });
    let mut lines = vec![match (&store, &url) {
        (Some(s), _) => format!("local store {s}"),
        (_, Some(u)) => format!("quipu server {u}"),
        _ => unreachable!("a location is a store or a url"),
    }];
    lines.push(format!("  from {}", cfg.location_source));
    lines.push(format!("  prefix {}", cfg.prefix));
    if let Some(g) = &cfg.graph {
        lines.push(format!("  graph {g}"));
    }
    if let Some(p) = &pendant {
        lines.push(format!("  pendant {p}"));
    }
    ok(json, value, lines.join("\n"), vec![])
}

/// The resolved configuration as flat `section.key` pairs. A token is shown as
/// set or unset, never its value.
fn config_pairs(cfg: &Resolved) -> Vec<(&'static str, Json)> {
    let (store, url, _) = location_parts(cfg);
    let path = |p: &Option<std::path::PathBuf>| {
        p.as_ref()
            .map_or(Json::Null, |p| Json::String(p.display().to_string()))
    };
    vec![
        ("quipu.store", serde_json::json!(store)),
        ("quipu.url", serde_json::json!(url)),
        (
            "quipu.location_source",
            serde_json::json!(cfg.location_source),
        ),
        (
            "quipu.token",
            serde_json::json!(match (&cfg.token, &cfg.token_file) {
                (Some(_), _) => "(set: SEEDS_QUIPU_TOKEN)",
                (None, Some(_)) => "(set: token_file)",
                (None, None) => "(unset)",
            }),
        ),
        ("quipu.token_file", path(&cfg.token_file)),
        ("quipu.trusted_hosts", serde_json::json!(cfg.trusted_hosts)),
        (
            "quipu.allow_plain_http_hosts",
            serde_json::json!(cfg.allow_plain_http_hosts),
        ),
        ("project.prefix", serde_json::json!(cfg.prefix)),
        ("project.graph", serde_json::json!(cfg.graph)),
        (
            "project.id_file",
            serde_json::json!(cfg.project_id_file.display().to_string()),
        ),
        ("pendant.dir", path(&cfg.pendant)),
        ("sync.remote", serde_json::json!(cfg.sync_remote)),
    ]
}

/// `sd config`: read-only over the resolved layers. Writing is not built: a
/// prefix or graph change would re-point the project at another ledger (see
/// `sd init`), so the file is edited deliberately, by hand.
fn config_outcome(
    json: bool,
    inputs: &config::Inputs,
    cfg: &Resolved,
    command: &ConfigCommand,
) -> Result<Outcome> {
    let text_of = |v: &Json| match v {
        Json::String(s) => s.clone(),
        Json::Null => "(none)".into(),
        other => other.to_string(),
    };
    match command {
        ConfigCommand::List => {
            let pairs = config_pairs(cfg);
            let value = Json::Object(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            );
            let text = pairs
                .iter()
                .map(|(k, v)| format!("{k} = {}", text_of(v)))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ok(json, value, text, vec![]))
        }
        ConfigCommand::Get { key } => {
            let pairs = config_pairs(cfg);
            let Some((_, v)) = pairs.iter().find(|(k, _)| k == key) else {
                return Err(SdError::usage(format!(
                    "unknown key {key:?}; sd config list shows every key"
                )));
            };
            Ok(ok(
                json,
                serde_json::json!({"key": key, "value": v}),
                text_of(v),
                vec![],
            ))
        }
        ConfigCommand::Path => {
            let (project, user) = config::config_paths(inputs);
            let row = |p: &Option<std::path::PathBuf>| {
                serde_json::json!({"path": p.as_ref().map(|p| p.display().to_string()),
                                   "exists": p.as_ref().is_some_and(|p| p.exists())})
            };
            let value = serde_json::json!({"project": row(&project), "user": row(&user)});
            let line = |name: &str, p: &Option<std::path::PathBuf>| match p {
                Some(p) => format!(
                    "{name} config: {} ({})",
                    p.display(),
                    if p.exists() { "exists" } else { "absent" }
                ),
                None => format!("{name} config: (none: not inside a project)"),
            };
            Ok(ok(
                json,
                value,
                format!("{}\n{}", line("Project", &project), line("User", &user)),
                vec![],
            ))
        }
        ConfigCommand::Set { .. } | ConfigCommand::Delete { .. } | ConfigCommand::Edit => {
            let (project, _) = config::config_paths(inputs);
            Err(SdError::new(
                ErrorKind::NotBuilt,
                format!(
                    "sd config does not write yet: edit {} by hand. A prefix or graph change \
                     moves the project to another ledger, which is why this is not a one-liner",
                    project.map_or_else(
                        || ".seeds/config.toml".to_string(),
                        |p| p.display().to_string()
                    )
                ),
            ))
        }
    }
}

/// `sd doctor`: read-only checks. Every check is reported; the exit code is 1
/// when any check is an error (so a script or CI can gate on it), 0 otherwise.
fn doctor(json: bool, cfg: &Resolved, b: &mut dyn Backend) -> Result<Outcome> {
    let mut checks: Vec<(String, &'static str, String)> = Vec::new();
    let mut add = |name: &str, status: &'static str, msg: String| {
        checks.push((name.to_string(), status, msg));
    };
    add(
        "config.resolves",
        "ok",
        format!("from {}", cfg.location_source),
    );
    match std::fs::read_to_string(&cfg.project_id_file) {
        Ok(id) if !id.trim().is_empty() && id.trim().chars().all(|c| c.is_ascii_alphanumeric()) => {
            add("project.id", "ok", id.trim().to_string())
        }
        Ok(_) => add(
            "project.id",
            "error",
            format!(
                "{} does not hold a project id",
                cfg.project_id_file.display()
            ),
        ),
        Err(_) => add(
            "project.id",
            "warn",
            format!(
                "no {} yet (created by the first write)",
                cfg.project_id_file.display()
            ),
        ),
    }
    if let Location::Store(p) = &cfg.location {
        let gi = p.parent().map(|d| d.join(".gitignore"));
        match gi {
            Some(g) if g.exists() || !p.exists() => {
                add(".gitignore", "ok", g.display().to_string())
            }
            Some(g) => add(
                ".gitignore",
                "warn",
                format!(
                    "{} is missing: the local store could be committed",
                    g.display()
                ),
            ),
            None => {}
        }
    }
    let snap = match b.snapshot(None) {
        Ok(s) => {
            add(
                "store.opens",
                "ok",
                format!(
                    "{} seeds, {} comments at tx {}",
                    s.seeds.len(),
                    s.comments.len(),
                    s.tx
                ),
            );
            Some(s)
        }
        Err(e) => {
            add("store.opens", "error", e.message.clone());
            None
        }
    };
    if let Some(snap) = &snap {
        // The same validation `sd import` applies to a pendant: shapes,
        // single-valued fields, dangling blocks edges.
        match pendant_of(b).and_then(|p| {
            let nt = p.export_nt().unwrap_or_default().to_string();
            Ok(crate::validate::validate_ledger(
                &nt,
                &pendant::parse_ntriples(&nt)?,
            ))
        }) {
            Ok(problems) if problems.is_empty() => {
                add("ledger.valid", "ok", "shapes and fields conform".into())
            }
            Ok(problems) => add("ledger.valid", "error", problems.join("; ")),
            Err(e) => add("ledger.valid", "error", e.message.clone()),
        }
        let dangling: Vec<String> = snap
            .seeds
            .values()
            .flat_map(|s| {
                s.dependencies()
                    .into_iter()
                    .map(move |(t, ty)| (s.id.clone(), t, ty))
            })
            .filter(|(_, t, _)| !snap.seeds.contains_key(t))
            .map(|(s, t, ty)| format!("{s} -{ty}-> {t}"))
            .collect();
        if dangling.is_empty() {
            add(
                "deps.targets_exist",
                "ok",
                "every dependency points at a seed".into(),
            );
        } else {
            add("deps.targets_exist", "error", dangling.join(", "));
        }
        // A cycle in `blocks` makes every seed on it permanently unready.
        let mut cycle: Option<Vec<String>> = None;
        let (mut done, mut stack): (std::collections::BTreeSet<String>, Vec<String>) =
            (std::collections::BTreeSet::new(), Vec::new());
        fn visit(
            id: &str,
            snap: &crate::model::Snapshot,
            done: &mut std::collections::BTreeSet<String>,
            stack: &mut Vec<String>,
            cycle: &mut Option<Vec<String>>,
        ) {
            if cycle.is_some() || done.contains(id) {
                return;
            }
            if let Some(i) = stack.iter().position(|s| s == id) {
                let mut c = stack[i..].to_vec();
                c.push(id.to_string());
                *cycle = Some(c);
                return;
            }
            stack.push(id.to_string());
            if let Some(s) = snap.seeds.get(id) {
                for t in &s.blocked_on {
                    visit(t, snap, done, stack, cycle);
                }
            }
            stack.pop();
            done.insert(id.to_string());
        }
        for id in snap.seeds.keys() {
            visit(id, snap, &mut done, &mut stack, &mut cycle);
        }
        match cycle {
            None => add("deps.no_cycles", "ok", "no blocks cycle".into()),
            Some(c) => add(
                "deps.no_cycles",
                "error",
                format!("blocks cycle: {}", c.join(" -> ")),
            ),
        }
    }
    let failed = checks.iter().any(|(_, s, _)| *s == "error");
    let value = serde_json::json!({
        "ok": !failed,
        "checks": checks.iter().map(|(n, s, m)| serde_json::json!({"name": n, "status": s, "message": m})).collect::<Vec<_>>(),
    });
    let text = checks
        .iter()
        .map(|(n, s, m)| format!("{:5} {n}: {m}", s))
        .collect::<Vec<_>>()
        .join("\n");
    let mut o = ok(json, value, text, vec![]);
    if failed {
        o.code = 1;
    }
    Ok(o)
}

/// `sd info`: `where`, plus what the ledger holds.
fn info_outcome(json: bool, cfg: &Resolved, b: &dyn Backend) -> Result<Outcome> {
    let snap = b.snapshot(None)?;
    let (store, url, mode) = location_parts(cfg);
    let size = store
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    let value = serde_json::json!({
        "database_path": store, "beads_dir": cfg.project_id_file.parent().map(|p| p.display().to_string()),
        "mode": mode, "quipu_url": url, "graph": cfg.graph,
        "issue_count": snap.seeds.len(), "comment_count": snap.comments.len(), "tx": snap.tx,
        "config": {"issue_prefix": cfg.prefix}, "db_size": size, "jsonl_path": null,
    });
    let text = format!(
        "{} seeds, {} comments at tx {} ({mode}: {})",
        snap.seeds.len(),
        snap.comments.len(),
        snap.tx,
        store.or(url).unwrap_or_default()
    );
    Ok(ok(json, value, text, vec![]))
}

/// Every leaf command path (e.g. `["comments", "add"]`) with its clap definition.
fn leaf_commands(
    cmd: &clap::Command,
    prefix: Vec<String>,
    out: &mut Vec<(Vec<String>, clap::Command)>,
) {
    for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
        let mut path = prefix.clone();
        path.push(sub.get_name().to_string());
        if sub.has_subcommands() {
            leaf_commands(sub, path, out);
        } else {
            out.push((path, sub.clone()));
        }
    }
}

/// `sd capabilities`: derived from sd's own definitions rather than written
/// out, so it cannot drift. read/write is `Command::writes`, the same flag that
/// decides whether a verb takes the store's write lock.
fn capabilities(command_path: Option<&str>) -> Result<Json> {
    use clap::{CommandFactory, Parser};
    let root = Cli::command();
    let mut leaves = Vec::new();
    leaf_commands(&root, Vec::new(), &mut leaves);
    let describe = |path: &[String], sub: &clap::Command| {
        // Parse the verb with a placeholder for each required positional to
        // learn its write class and whether it is a pointer elsewhere.
        let placeholder = |a: &clap::Arg| {
            a.get_possible_values()
                .first()
                .map_or_else(|| "x".to_string(), |v| v.get_name().to_string())
        };
        let attempt = |all: bool| {
            let mut argv: Vec<String> = vec!["sd".into()];
            argv.extend(path.iter().cloned());
            for a in sub.get_positionals().filter(|a| all || a.is_required_set()) {
                let n = a.get_num_args().map_or(1, |r| r.min_values().max(1));
                argv.extend(std::iter::repeat_n(placeholder(a), n));
            }
            Cli::try_parse_from(&argv).ok()
        };
        let parsed = attempt(false).or_else(|| attempt(true));
        let operation = match &parsed {
            Some(c) if mapped_pointer(&c.command).is_some() => "elsewhere",
            Some(c) if c.command.writes() => "write",
            Some(_) => "read",
            None => "unknown",
        };
        serde_json::json!({
            "name": path.join(" "),
            "summary": sub.get_about().map(|s| s.to_string()),
            "aliases": sub.get_visible_aliases().collect::<Vec<_>>(),
            "operation": operation,
            "pins_with_at": operation == "read",
            "flags": sub.get_arguments()
                .filter(|a| !a.is_global_set() && a.get_long().is_some())
                .map(|a| format!("--{}", a.get_long().unwrap_or_default()))
                .collect::<Vec<_>>(),
        })
    };
    if let Some(p) = command_path {
        let want: Vec<String> = p.split_whitespace().map(str::to_string).collect();
        return leaves
            .iter()
            .find(|(path, _)| *path == want)
            .map(|(path, sub)| describe(path, sub))
            .ok_or_else(|| SdError::usage(format!("unknown command path {p:?}")));
    }
    Ok(serde_json::json!({
        "tool": "sd",
        "version": env!("CARGO_PKG_VERSION"),
        "contract_version": "sd.capabilities.v1",
        "commands": leaves.iter().map(|(p, s)| describe(p, s)).collect::<Vec<_>>(),
        "global_flags": root.get_arguments()
            .filter(|a| a.get_long().is_some())
            .map(|a| serde_json::json!({
                "flag": format!("--{}", a.get_long().unwrap_or_default()),
                "description": a.get_help().map(|h| h.to_string()),
            }))
            .collect::<Vec<_>>(),
        "output_formats": ["text", "json"],
        "exit_codes": std::iter::once(serde_json::json!({"code": 0, "name": "OK", "description": "success"}))
            .chain(ErrorKind::ALL.iter().map(|k| serde_json::json!({
                "code": k.exit_code(), "name": k.name(), "description": k.description()})))
            .collect::<Vec<_>>(),
        "env_vars": ["SEEDS_ACTOR", "SEEDS_QUIPU_STORE", "SEEDS_QUIPU_URL", "SEEDS_GRAPH",
                     "SEEDS_PREFIX", "SEEDS_SYNC_REMOTE", "SEEDS_QUIPU_TOKEN",
                     "SEEDS_QUIPU_TOKEN_FILE"],
        "safety": [
            "every write is one quipu transaction: all named seeds change, or none do",
            "a lost response is read back; an unconfirmed write is exit 8, never a blind retry",
            "an unreachable server is exit 7; sd never falls back to a local store",
            "a token is never sent to a project-chosen server unless the user trusts its host",
            "--at pins any read to a past transaction; writes always apply to now",
            "delete is a tombstone: history is kept, sync carries it without deleting data",
        ],
        "json_schemas": "sd schema",
    }))
}

/// Where a br verb that sd does not own lives. These exit 21 (ELSEWHERE) with
/// the pointer on stderr, so desire-path records every attempt (aegis-w3k75d.8).
fn mapped_pointer(c: &Command) -> Option<String> {
    let (verb, where_) = match c {
        Command::Query(_) => (
            "query",
            "saved queries are quipu stored queries: ask quipu (quipu ask / the quipu_ask tool)",
        ),
        Command::Upgrade(_) => (
            "upgrade",
            "sd is installed and upgraded by caboodle: caboodle update-release --tool seeds",
        ),
        Command::Gate(_) => ("gate", "workflow gates are shuttle's"),
        Command::Scheduler(_) => ("scheduler", "ranking ready work for a swarm is shuttle's"),
        Command::Audit(_) => (
            "audit",
            "provenance is quipu's: every sd write is a transaction recording its actor and \
             source; read past states with sd show --at <tx>",
        ),
        Command::RobotDocs(_) => (
            "robot-docs",
            "the automation docs are the seeds book and sd <verb> --help",
        ),
        _ => return None,
    };
    Some(format!("`sd {verb}` is not an sd verb: {where_}"))
}

/// `sd completions <shell> [-o dir]`: clap's generator over the real CLI
/// definition, so completions can never drift from the verbs.
fn completions(a: &cli::CompletionsArgs) -> Result<Outcome> {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    clap_complete::generate(a.shell, &mut cmd, "sd", &mut buf);
    let script = String::from_utf8(buf)
        .map_err(|e| SdError::failed(format!("completion script is not UTF-8: {e}")))?;
    match &a.output {
        None => Ok(ok(false, Json::Null, script, vec![])),
        Some(dir) => {
            let name = match a.shell {
                clap_complete::Shell::Bash => "sd.bash".to_string(),
                clap_complete::Shell::Zsh => "_sd".to_string(),
                clap_complete::Shell::Fish => "sd.fish".to_string(),
                clap_complete::Shell::PowerShell => "_sd.ps1".to_string(),
                clap_complete::Shell::Elvish => "sd.elv".to_string(),
                other => format!("sd.{other}"),
            };
            let path = std::path::Path::new(dir).join(name);
            std::fs::write(&path, script)
                .map_err(|e| SdError::failed(format!("cannot write {}: {e}", path.display())))?;
            Ok(ok(
                false,
                Json::Null,
                format!("wrote {}", path.display()),
                vec![],
            ))
        }
    }
}

/// `sd init`: in the current directory, never a parent project's.
fn init(cli: &Cli, a: &cli::InitArgs) -> Result<Outcome> {
    let cwd = std::env::current_dir()
        .map_err(|e| SdError::failed(format!("cannot read the current directory: {e}")))?;
    // The default prefix follows the usual layers (env, user config); a parent
    // project's file is deliberately not consulted.
    let default_prefix = std::env::var("SEEDS_PREFIX")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| config::DEFAULT_PREFIX.to_string());
    let r = config::init_project(&cwd, a.prefix.as_deref(), &default_prefix, a.force)?;
    let graph = crate::vocab::project_graph_iri(&format!("{}-{}", r.prefix, r.project_id));
    let value = serde_json::json!({
        "path": r.dir.display().to_string(), "prefix": r.prefix, "project_id": r.project_id,
        "graph": graph, "created": r.created, "kept": r.kept,
    });
    let text = format!(
        "initialized {} (prefix {}, project id {}){}\ncommit .seeds/config.toml and \
         .seeds/project-id: together they name this project's ledger",
        r.dir.display(),
        r.prefix,
        r.project_id,
        if r.kept.is_empty() {
            String::new()
        } else {
            format!("; kept {}", r.kept.join(", "))
        }
    );
    Ok(ok(cli.json, value, text, vec![]))
}

fn error_outcome(json: bool, e: &SdError, warnings: Vec<String>) -> Outcome {
    let mut stderr = warnings.join("\n");
    if !stderr.is_empty() {
        stderr.push('\n');
    }
    stderr.push_str(&format!("sd: {}", e.message));
    Outcome {
        code: e.exit_code(),
        stdout: if json {
            output::error_json(e.kind.name(), &e.message).to_string()
        } else {
            String::new()
        },
        stderr,
    }
}

fn now() -> String {
    quipu::time::now_iso()
}

fn actor(cli: &Cli) -> String {
    cli.actor
        .clone()
        .filter(|a| !a.trim().is_empty())
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))
        .unwrap_or_else(|| "unknown".into())
}

/// Run against an already-resolved configuration.
///
/// Four arrangements (see `docs/book/src/storage-modes.md`):
///
/// - **local** (the default): a quipu store file;
/// - **repo-local pendant** (`[pendant] dir`): a local working store plus a
///   pendant in the repository that every write is exported to and every
///   command first reconciles with;
/// - **remote** (`[quipu] url`): a quipu server over HTTP;
/// - **sync** (`[sync] remote`): a local store (with or without a pendant)
///   that `sd sync` merges with a remote.
pub fn run_with(cli: &Cli, cfg: &Resolved) -> Result<Outcome> {
    let ctx = Ctx {
        now: now(),
        actor: actor(cli),
        prefix: cfg.prefix.clone(),
    };
    if cli.command.writes() && cli.at.is_some() {
        return Err(SdError::usage(
            "--at pins a read; a write always applies to the current state",
        ));
    }
    // The graph: configured, or derived from the project id, which is created
    // on first write or first use of a server (never by a plain local read).
    let create = match &cfg.location {
        Location::Url(_) => true,
        Location::Store(_) => {
            cli.command.writes()
                || cfg.pendant.is_some()
                || matches!(cli.command, Command::Export(_))
        }
    };
    let (graph, note) = config::project_graph(cfg, create)?;
    let graph = match graph {
        Some(g) => g,
        None => {
            if let Location::Store(p) = &cfg.location {
                if p.exists() {
                    return Err(SdError::new(
                        ErrorKind::Config,
                        format!(
                            "{} exists but {} is missing, so seeds cannot tell which ledger in \
                             it is this project's; restore that file or set [project] graph",
                            p.display(),
                            cfg.project_id_file.display()
                        ),
                    ));
                }
            }
            // Nothing has been written from here: an empty graph answers.
            crate::vocab::project_graph_iri(&format!("{}-none", cfg.prefix))
        }
    };
    let cfg = &Resolved {
        graph: Some(graph),
        ..cfg.clone()
    };
    let o = match &cfg.location {
        Location::Url(url) => run_remote(cli, cfg, &ctx, url),
        Location::Store(path) => run_local(cli, cfg, &ctx, path),
    }?;
    Ok(with_notes(o, note.into_iter().collect()))
}

/// The bearer for writes to `url`, if one is configured AND may go there: a
/// token is never sent to a server URL that came from a project file unless
/// the user's own config lists its host in `trusted_hosts`.
fn token(cfg: &Resolved, url: &str, from_project: bool) -> Result<Option<String>> {
    if !cfg.token_allowed(url, from_project) {
        if cfg.token.is_some() || cfg.token_file.is_some() {
            eprintln!(
                "sd: not sending your quipu token to {url}: that URL comes from the project's \
                 config. Add its host to trusted_hosts in ~/.config/seeds/config.toml to allow it."
            );
        }
        return Ok(None);
    }
    if let Some(t) = &cfg.token {
        return Ok(Some(t.trim().to_string()));
    }
    match &cfg.token_file {
        Some(f) => std::fs::read_to_string(f)
            .map(|t| Some(t.trim().to_string()))
            .map_err(|e| {
                SdError::new(
                    ErrorKind::Config,
                    format!("cannot read the quipu token file {}: {e}", f.display()),
                )
            }),
        None => Ok(None),
    }
}

fn with_notes(mut o: Outcome, notes: Vec<String>) -> Outcome {
    if notes.is_empty() {
        return o;
    }
    let notes: Vec<String> = notes.into_iter().map(|n| format!("sd: {n}")).collect();
    o.stderr = if o.stderr.is_empty() {
        notes.join("\n")
    } else {
        format!("{}\n{}", notes.join("\n"), o.stderr)
    };
    o
}

fn run_remote(cli: &Cli, cfg: &Resolved, ctx: &Ctx, url: &str) -> Result<Outcome> {
    let mut remote = remote::RemoteBackend::connect(
        url,
        cfg.graph(),
        token(cfg, url, cfg.location_from_project)?,
        &cfg.allow_plain_http_hosts,
    )?;
    match &cli.command {
        Command::Export(a) => {
            let dir = export_dir(a.to.as_deref(), cfg)?;
            let p = pendant_of(&remote)?;
            let changed = store::write_pendant_dir(&dir, &p)?;
            Ok(export_outcome(cli.json, &dir, &p, changed))
        }
        Command::Import(a) => import_into(cli, ctx, &mut remote, a),
        Command::Sync(_) => Err(SdError::usage(
            "sd sync runs from a local store ([quipu] store) to a [sync] remote; this \
             configuration's primary store is already the remote",
        )),
        _ => dispatch(cli, cfg, ctx, &mut remote),
    }
}

/// A pendant of any backend: load its snapshot into a scratch in-memory store
/// and export that. `export.nt` depends only on the ledger, so this is
/// byte-identical to exporting the store itself.
fn pendant_of(b: &dyn Backend) -> Result<pendant::Pendant> {
    let snap = b.snapshot(None)?;
    // The scratch store's graph name does not appear in export.nt.
    let mut scratch = QuipuBackend::in_memory("https://seeds.local/project/scratch")?;
    let ctx = Ctx {
        now: now(),
        actor: "seeds".into(),
        prefix: String::new(),
    };
    sync::import(&mut scratch, &ctx, &snap, None, true)?;
    pendant::export(&scratch)
}

fn export_dir(to: Option<&str>, cfg: &Resolved) -> Result<std::path::PathBuf> {
    match (to, &cfg.pendant) {
        (Some(t), _) => Ok(std::path::PathBuf::from(t)),
        (None, Some(p)) => Ok(p.clone()),
        (None, None) => Err(SdError::usage(
            "no pendant directory configured ([pendant] dir); pass --to <dir>",
        )),
    }
}

fn export_outcome(
    json: bool,
    dir: &std::path::Path,
    p: &pendant::Pendant,
    changed: bool,
) -> Outcome {
    let seeds = p
        .export_nt()
        .map(|t| {
            t.lines()
                .filter(|l| l.contains("/ontology/identifier>"))
                .count()
        })
        .unwrap_or(0);
    let hash = p.export_hash().unwrap_or_default();
    let text = format!(
        "{} pendant at {} ({seeds} seeds, export.nt {hash})",
        if changed { "wrote" } else { "unchanged:" },
        dir.display()
    );
    ok(
        json,
        serde_json::json!({
            "status": "ok",
            "dir": dir.display().to_string(),
            "changed": changed,
            "seeds": seeds,
            "export_hash": hash,
            "files": p.files.keys().collect::<Vec<_>>(),
        }),
        text,
        vec![],
    )
}

fn import_into(cli: &Cli, ctx: &Ctx, b: &mut dyn Backend, a: &cli::ImportArgs) -> Result<Outcome> {
    let dir = std::path::Path::new(&a.dir);
    let p = store::read_pendant_dir(dir)?.ok_or_else(|| {
        SdError::new(
            ErrorKind::NotFound,
            format!(
                "no pendant at {} (no {})",
                dir.display(),
                pendant::EXPORT_NT
            ),
        )
    })?;
    let ledger = pendant::read(&p)?;
    let prefer = match a.prefer.as_deref() {
        Some("pendant") => Some(sync::Prefer::Incoming),
        Some("store") => Some(sync::Prefer::Existing),
        _ => None,
    };
    let r = sync::import(b, ctx, &ledger.snapshot, prefer, a.replace)?;
    let mut warnings = Vec::new();
    if let pendant::Seal::Broken(why) = &ledger.seal {
        warnings.push(format!(
            "the pendant's manifest does not match its data ({why}); its data validated and was used"
        ));
    }
    Ok(ok(
        cli.json,
        report_json(&r),
        report_text("imported", &r),
        warnings,
    ))
}

fn report_json(r: &sync::Report) -> serde_json::Value {
    serde_json::json!({
        "status": "ok",
        "created": r.created,
        "updated": r.updated,
        "removed": r.removed,
        "unchanged": r.unchanged,
        "comments_added": r.comments_added,
        "tx": r.tx,
        "wrote": r.wrote,
    })
}

fn report_text(verb: &str, r: &sync::Report) -> String {
    format!(
        "{verb}: {} created, {} updated, {} removed, {} unchanged, {} comments added{}",
        r.created.len(),
        r.updated.len(),
        r.removed.len(),
        r.unchanged,
        r.comments_added,
        if r.tx > 0 {
            format!(" (tx {})", r.tx)
        } else {
            String::new()
        }
    )
}

fn run_local(cli: &Cli, cfg: &Resolved, ctx: &Ctx, path: &std::path::Path) -> Result<Outcome> {
    match &cli.command {
        Command::Export(a) => {
            let dir = export_dir(a.to.as_deref(), cfg)?;
            let h = store::open_for_write(path, cfg.graph())?;
            let is_configured = cfg.pendant.as_deref() == Some(dir.as_path());
            let p = pendant::export(&h.backend)?;
            let changed = if is_configured {
                store::export_to_pendant(&h.backend, path, &dir)?
            } else {
                store::write_pendant_dir(&dir, &p)?
            };
            return Ok(export_outcome(cli.json, &dir, &p, changed));
        }
        Command::Import(a) => {
            let mut h = store::open_for_write(path, cfg.graph())?;
            let o = import_into(cli, ctx, &mut h.backend, a)?;
            if let Some(dir) = &cfg.pendant {
                store::export_to_pendant(&h.backend, path, dir)?;
            }
            return Ok(o);
        }
        Command::Sync(a) => return run_sync(cli, cfg, ctx, path, a),
        _ => {}
    }
    if let Some(dir) = &cfg.pendant {
        // Mode 1: reconcile with the pendant, run, export.
        let mut h = store::open_for_write(path, cfg.graph())?;
        let notes = store::hydrate(&mut h.backend, path, dir, ctx)?;
        let o = dispatch(cli, cfg, ctx, &mut h.backend)?;
        if cli.command.writes() {
            store::export_to_pendant(&h.backend, path, dir)?;
        }
        return Ok(with_notes(o, notes));
    }
    if cli.command.writes() {
        let mut h = store::open_for_write(path, cfg.graph())?;
        return dispatch(cli, cfg, ctx, &mut h.backend);
    }
    match store::open_for_read(path, cfg.graph())? {
        Some(mut b) => dispatch(cli, cfg, ctx, &mut b),
        None => {
            // Nothing written yet: answer from an empty graph, and say so,
            // so an empty answer is never mistaken for "nothing matched".
            let mut empty = QuipuBackend::in_memory(cfg.graph())?;
            let o = dispatch(cli, cfg, ctx, &mut empty)?;
            Ok(with_notes(
                o,
                vec![format!(
                    "no seeds store at {} yet (it is created on first write)",
                    path.display()
                )],
            ))
        }
    }
}

fn run_sync(
    cli: &Cli,
    cfg: &Resolved,
    ctx: &Ctx,
    path: &std::path::Path,
    a: &cli::SyncArgs,
) -> Result<Outcome> {
    let url = a
        .remote
        .clone()
        .or_else(|| cfg.sync_remote.clone())
        .ok_or_else(|| {
            SdError::usage(
                "sd sync needs a remote: set [sync] remote, SEEDS_SYNC_REMOTE, or --remote",
            )
        })?;
    let mut h = store::open_for_write(path, cfg.graph())?;
    let mut notes = Vec::new();
    if let Some(dir) = &cfg.pendant {
        notes.extend(store::hydrate(&mut h.backend, path, dir, ctx)?);
    }
    let from_project = a.remote.is_none() && cfg.sync_remote_from_project;
    let mut remote = remote::RemoteBackend::connect(
        &url,
        cfg.graph(),
        token(cfg, &url, from_project)?,
        &cfg.allow_plain_http_hosts,
    )?;
    let base_path = store::sync_base_path(path, &url, cfg.graph());
    let base = match std::fs::read_to_string(&base_path) {
        Ok(nt) => {
            let p = pendant::Pendant {
                files: [(pendant::EXPORT_NT.to_string(), nt)].into(),
            };
            pendant::read(&p)
                .map_err(|e| {
                    SdError::failed(format!(
                        "the sync base {} is unreadable ({}); nothing was written. Remove it \
                         only if you want the next sync to treat this remote as new.",
                        base_path.display(),
                        e.message
                    ))
                })?
                .snapshot
        }
        // Only a missing file means "never synced with this remote".
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => crate::model::Snapshot::default(),
        Err(e) => {
            return Err(SdError::failed(format!(
                "cannot read the sync base {}: {e}; nothing was written",
                base_path.display()
            )))
        }
    };
    let (_, local_r, remote_r) = sync::sync(
        &base,
        &mut h.backend,
        &mut remote,
        ctx,
        a.allow_remote_deletes,
    )?;
    // The new base is what both sides now hold.
    let merged = pendant::export(&h.backend)?;
    store::write_sync_base(
        &base_path,
        &url,
        cfg.graph(),
        merged.export_nt().unwrap_or_default(),
    )?;
    if let Some(dir) = &cfg.pendant {
        store::export_to_pendant(&h.backend, path, dir)?;
    }
    let text = format!(
        "{}\n{}",
        report_text("local", &local_r),
        report_text(&format!("remote {url}"), &remote_r)
    );
    let json = serde_json::json!({
        "status": "ok",
        "local": report_json(&local_r),
        "remote": report_json(&remote_r),
    });
    Ok(with_notes(ok(cli.json, json, text, vec![]), notes))
}

/// `sd merge-driver %O %A %B`: merge three versions of a pendant's
/// `export.nt` with [`sync::merge3`] and write the result over `%A`.
///
/// On a conflict it exits 1 (so git marks the file conflicted) and writes a
/// first line that does not parse, followed by the conflicts and our side, so
/// the file cannot be committed and loaded by accident: sd refuses it until a
/// person resolves it.
fn merge_driver(a: &cli::MergeDriverArgs) -> Result<Outcome> {
    let read = |p: &str| {
        std::fs::read_to_string(p).map_err(|e| SdError::failed(format!("cannot read {p}: {e}")))
    };
    let snap = |text: &str| -> Result<crate::model::Snapshot> {
        Ok(crate::model::Snapshot::from_subjects(
            &pendant::parse_ntriples(text)?,
        ))
    };
    let (base_t, ours_t, theirs_t) = (read(&a.base)?, read(&a.ours)?, read(&a.theirs)?);
    let m = sync::merge3_named(
        &snap(&base_t)?,
        &snap(&ours_t)?,
        &snap(&theirs_t)?,
        sync::Sides {
            a: "ours",
            b: "theirs",
        },
    );
    if !m.conflicts.is_empty() {
        let mut out = format!(
            "<<<<<<< sd merge-driver: {} conflicting seed(s). Resolve with sd on either branch and merge again, or edit this file to one valid ledger.\n",
            m.conflicts.len()
        );
        for c in &m.conflicts {
            out.push_str(&format!("# {}: {}\n", c.id, c.fields.join("; ")));
        }
        out.push_str(&ours_t);
        std::fs::write(&a.ours, out)
            .map_err(|e| SdError::failed(format!("cannot write {}: {e}", a.ours)))?;
        return Err(sync::conflict_error(
            "merge",
            &m.conflicts,
            "The file is marked conflicted.",
        ));
    }
    let mut scratch = QuipuBackend::in_memory("https://seeds.local/project/scratch")?;
    let ctx = Ctx {
        now: now(),
        actor: "seeds".into(),
        prefix: String::new(),
    };
    sync::import(&mut scratch, &ctx, &m.merged, None, true)?;
    let merged = pendant::export(&scratch)?;
    std::fs::write(&a.ours, merged.export_nt().unwrap_or_default())
        .map_err(|e| SdError::failed(format!("cannot write {}: {e}", a.ours)))?;
    Ok(Outcome::default())
}

/// " (tx N)" when the store reported a transaction (a quipu server does not).
/// `2026-09-30T14:00:00-04:00` -> `2026-09-30T18:00:00Z`, so a git date
/// compares correctly against the ledger's UTC instants.
fn to_utc(d: &str) -> Option<String> {
    let (base, sign, off) = if let Some(i) = d.rfind(['+', '-']).filter(|i| *i >= 19) {
        (&d[..19], if &d[i..=i] == "-" { 1 } else { -1 }, &d[i + 1..])
    } else {
        return d
            .ends_with('Z')
            .then(|| format!("{}Z", &d[..19.min(d.len())]));
    };
    let (h, m) = off.split_once(':')?;
    let shift = sign * (h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().ok()? * 60);
    let t = engine::parse_until(&format!("{base}Z"), &format!("{base}Z")).ok()?;
    let secs = if shift >= 0 {
        format!("+{}m", shift / 60)
    } else {
        String::new()
    };
    if shift >= 0 {
        engine::parse_until(&secs, &t).ok()
    } else {
        engine::parse_since(&format!("+{}m", -shift / 60), &t).ok()
    }
}

fn tx_note(tx: u64) -> String {
    if tx > 0 {
        format!(" (tx {tx})")
    } else {
        String::new()
    }
}

fn read_text(path: &str) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| SdError::usage(format!("cannot read {path}: {e}")))
}

fn split_csv(s: &Option<String>) -> Vec<String> {
    s.as_deref()
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn ok(json: bool, value: Json, text: String, warnings: Vec<String>) -> Outcome {
    Outcome {
        code: 0,
        stdout: if json { value.to_string() } else { text },
        stderr: warnings
            .into_iter()
            .map(|w| format!("sd: {w}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn dispatch(cli: &Cli, cfg: &Resolved, ctx: &Ctx, b: &mut dyn Backend) -> Result<Outcome> {
    let json = cli.json;
    let at = cli.at;
    match &cli.command {
        Command::Create(a) => {
            let title = match (&a.title, &a.title_flag) {
                (Some(_), Some(_)) => {
                    return Err(SdError::usage("give the title once: positional or --title"))
                }
                (Some(t), None) | (None, Some(t)) => t.clone(),
                (None, None) => return Err(SdError::usage("a seed needs a title")),
            };
            let description = match (&a.description, &a.description_file) {
                (Some(_), Some(_)) => {
                    return Err(SdError::usage(
                        "give the description once: --description or --description-file",
                    ))
                }
                (Some(d), None) => Some(d.clone()),
                (None, Some(f)) => Some(read_text(f)?),
                (None, None) => None,
            };
            let req = engine::CreateReq {
                title,
                description,
                issue_type: a.issue_type.clone(),
                priority: a.priority.clone(),
                assignee: a.assignee.clone(),
                owner: a.owner.clone(),
                labels: split_csv(&a.labels),
                parent: a.parent.clone(),
                deps: split_csv(&a.deps),
                workflow_run: a.workflow_run.clone(),
                step: a.step.clone(),
                visit: a.visit,
                dry_run: a.dry_run,
            };
            let (seed, tx) = engine::create(b, ctx, &req)?;
            let text = if a.silent {
                seed.id.clone()
            } else if a.dry_run {
                format!("would create {}", output::seed_line(&seed))
            } else if tx == 0 {
                format!(
                    "exists {} (this run and step already created it)",
                    output::seed_line(&seed)
                )
            } else {
                format!("created {}{}", output::seed_line(&seed), tx_note(tx))
            };
            // null when nothing was written: a dry run, or a keyed create whose
            // seed already existed (tx 0 is not a transaction to pin with --at).
            let tx = (!a.dry_run && tx != 0).then_some(tx);
            Ok(ok(json, output::with_tx(seed.to_json(), tx), text, vec![]))
        }
        Command::Q(a) => {
            let req = engine::CreateReq {
                title: a.title.join(" "),
                description: a.description.clone(),
                issue_type: a.issue_type.clone(),
                priority: a.priority.clone(),
                labels: a.labels.iter().flat_map(|l| split_csv(&Some(l.clone()))).collect(),
                parent: a.parent.clone(),
                ..engine::CreateReq::default()
            };
            let (seed, tx) = engine::create(b, ctx, &req)?;
            let value = serde_json::json!({"id": seed.id, "title": seed.title, "tx": tx});
            Ok(ok(json, value, seed.id.clone(), vec![]))
        }
        Command::Show(a) => {
            let views = engine::show(b, &a.ids, at)?;
            let text = views
                .iter()
                .map(output::view_text)
                .collect::<Vec<_>>()
                .join("\n\n");
            Ok(ok(json, output::show_json(&views), text, vec![]))
        }
        Command::List(a) => {
            let req = engine::ListReq {
                filter: Filter {
                    status: a.status.clone(),
                    issue_type: a.issue_type.clone(),
                    assignee: a.assignee.clone(),
                    unassigned: a.unassigned,
                    labels: a.label.clone(),
                    priority: a.priority.clone(),
                    parent: None,
                },
                all: a.all,
                limit: a.limit,
                sort: a.sort.clone(),
            };
            let p = engine::list(b, &req, at)?;
            let mut warnings = vec![];
            if p.has_more {
                warnings.push(format!(
                    "list: showing {} of {}; --limit 0 lists all",
                    p.issues.len(),
                    p.total
                ));
            }
            Ok(ok(
                json,
                output::list_json(&p),
                output::page_text(&p, "matching"),
                warnings,
            ))
        }
        Command::Ready(a) => {
            let assignee = match a.assignee.as_deref() {
                Some("") => Some(ctx.actor.clone()),
                other => other.map(str::to_string),
            };
            let req = engine::ReadyReq {
                filter: Filter {
                    status: None,
                    issue_type: a.issue_type.clone(),
                    assignee,
                    unassigned: a.unassigned,
                    labels: a.label.clone(),
                    priority: a.priority.clone(),
                    parent: a.parent.clone(),
                },
                limit: a.limit,
            };
            let p = engine::ready(b, ctx, &req, at)?;
            let mut warnings = vec![];
            if p.has_more {
                warnings.push(format!(
                    "ready: TRUNCATED to {} of {} ready seeds by --limit",
                    p.issues.len(),
                    p.total
                ));
            }
            Ok(ok(
                json,
                output::seeds_json(&p.issues),
                output::page_text(&p, "ready"),
                warnings,
            ))
        }
        Command::Search(a) => {
            let req = engine::SearchReq {
                query: a.query.clone(),
                filter: Filter {
                    status: a.status.clone(),
                    issue_type: a.issue_type.clone(),
                    assignee: a.assignee.clone(),
                    unassigned: a.unassigned,
                    labels: a.label.clone(),
                    priority: a.priority.clone(),
                    parent: None,
                },
                all: a.all,
                limit: a.limit,
                sort: a.sort.clone(),
            };
            let r = engine::search(b, &req, at)?;
            Ok(ok(
                json,
                output::search_json(&r),
                output::search_text(&r, &a.query),
                vec![],
            ))
        }
        Command::Stats(a) => {
            let req = engine::StatsReq {
                by_type: a.by_type,
                by_priority: a.by_priority,
                by_assignee: a.by_assignee,
                by_label: a.by_label,
            };
            let st = engine::stats(b, ctx, req, at)?;
            Ok(ok(
                json,
                output::stats_json(&st),
                output::stats_text(&st),
                vec![],
            ))
        }
        Command::Graph(a) => {
            let graphs = match &a.issue {
                Some(id) if !a.all => vec![engine::graph(b, id, a.dependencies, at)?],
                _ => engine::graph_all(b, at)?,
            };
            let node = |n: &engine::GraphNode| {
                serde_json::json!({"id": n.seed.id, "title": n.seed.title, "status": n.seed.status,
                                   "priority": n.seed.priority, "depth": n.depth})
            };
            let one = |g: &engine::Graph| {
                serde_json::json!({"nodes": g.nodes.iter().map(node).collect::<Vec<_>>(),
                                   "edges": g.edges.iter().map(|(f, t)| [f, t]).collect::<Vec<_>>()})
            };
            let value = if a.all {
                serde_json::json!({"total_components": graphs.len(),
                                   "total_nodes": graphs.iter().map(|g| g.nodes.len()).sum::<usize>(),
                                   "components": graphs.iter().map(|g| {
                    let mut o = one(g);
                    o["roots"] = serde_json::json!(g.roots);
                    o
                }).collect::<Vec<_>>()})
            } else {
                let g = &graphs[0];
                let mut o = one(g);
                o["root"] = serde_json::json!(g.roots[0]);
                o["count"] = serde_json::json!(g.nodes.len());
                o
            };
            if a.dot {
                let mut lines = vec!["digraph dependencies {".to_string(),
                    "    node [shape=box, style=\"rounded,filled\", fontname=\"sans-serif\"];".into()];
                for g in &graphs {
                    for n in &g.nodes {
                        lines.push(format!(
                            "    {:?} [label={:?}];",
                            n.seed.id,
                            format!("{}\n{} [P{}]", n.seed.id, n.seed.title, n.seed.priority)
                        ));
                    }
                    for (f, t) in &g.edges {
                        lines.push(format!("    {f:?} -> {t:?};"));
                    }
                }
                lines.push("}".into());
                return Ok(ok(false, Json::Null, lines.join("\n"), vec![]));
            }
            let mut lines = Vec::new();
            for g in &graphs {
                if !a.all && g.nodes.len() <= 1 {
                    let what = if a.dependencies { "dependencies" } else { "dependents" };
                    lines.push(format!("no {what} for {}", g.roots[0]));
                    continue;
                }
                for n in &g.nodes {
                    lines.push(if a.compact {
                        format!("{} {} [{}] depth {}", n.seed.id, n.seed.title, n.seed.status, n.depth)
                    } else {
                        format!("{}{}", "  ".repeat(n.depth), output::seed_line(&n.seed))
                    });
                }
                if a.all {
                    lines.push(String::new());
                }
            }
            Ok(ok(json, value, lines.join("\n").trim_end().to_string(), vec![]))
        }
        Command::History { id } => {
            if at.is_some() {
                return Err(SdError::usage("history lists every version; it takes no --at"));
            }
            let entries = engine::history(b, id)?;
            // wu's triage ruling: history NAMES its meaning, because br's verb of
            // the same name manages local backup files.
            let meaning = "transaction history: every version of the seed in quipu, \
                           with the tx that wrote it (read any one with --at <tx>); \
                           not br's local backup files";
            let value = serde_json::json!({
                "meaning": meaning,
                "id": id,
                "versions": entries.iter().map(|e| {
                    let mut o = e.seed.to_json();
                    o["tx"] = serde_json::json!(e.tx);
                    o["changes"] = serde_json::json!(e.changes);
                    o
                }).collect::<Vec<_>>(),
            });
            let mut lines = vec![format!("{id}: {meaning}")];
            for e in &entries {
                lines.push(format!(
                    "tx {}  revision {}  {}  {}",
                    e.tx, e.seed.revision, e.seed.updated_at, e.seed.status
                ));
                if e.changes.is_empty() && e.seed.revision == 1 {
                    lines.push("  created".into());
                }
                lines.extend(e.changes.iter().map(|c| format!("  {c}")));
            }
            Ok(ok(json, value, lines.join("\n"), vec![]))
        }
        Command::Changelog(a) => {
            let git_date = |rev: &str| -> Result<String> {
                let o = std::process::Command::new("git")
                    .args(["log", "-1", "--format=%cI", rev, "--"])
                    .output()
                    .map_err(|e| SdError::failed(format!("cannot run git: {e}")))?;
                let d = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !o.status.success() || d.is_empty() {
                    return Err(SdError::usage(format!("git does not know {rev:?}")));
                }
                // git gives a local offset; the ledger's instants are UTC.
                Ok(to_utc(&d).unwrap_or(d))
            };
            let since = match (&a.since, &a.since_tag, &a.since_commit) {
                (Some(s), _, _) => Some(engine::parse_since(s, &ctx.now)?),
                (_, Some(t), _) => Some(git_date(&format!("refs/tags/{t}"))?),
                (_, _, Some(c)) => Some(git_date(c)?),
                _ => None,
            };
            let groups = engine::changelog(b, since.as_deref(), at)?;
            let total: usize = groups.iter().map(|g| g.issues.len()).sum();
            let value = serde_json::json!({
                "since": since.clone().unwrap_or_else(|| "all".into()),
                "until": ctx.now,
                "total_closed": total,
                "groups": groups.iter().map(|g| serde_json::json!({
                    "issue_type": g.issue_type, "label": g.label,
                    "issues": g.issues.iter().map(|s| serde_json::json!({
                        "id": s.id, "title": s.title, "priority": format!("P{}", s.priority),
                        "closed_at": s.closed_at,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            });
            let mut lines = vec![format!(
                "Changelog since {} ({total} closed seed{}):",
                since.as_deref().unwrap_or("all"),
                if total == 1 { "" } else { "s" }
            )];
            for g in &groups {
                lines.push(String::new());
                lines.push(format!("{}:", g.label));
                lines.extend(
                    g.issues
                        .iter()
                        .map(|s| format!("- [P{}] {} {}", s.priority, s.id, s.title)),
                );
            }
            Ok(ok(json, value, lines.join("\n"), vec![]))
        }
        Command::Lint(a) => {
            let results = engine::lint(b, &a.ids, a.issue_type.as_deref(), a.status.as_deref(), at)?;
            let warnings: usize = results.iter().map(|r| r.missing.len()).sum();
            let value = serde_json::json!({
                "total": warnings,
                "issues": results.len(),
                "results": results.iter().map(|r| serde_json::json!({
                    "id": r.seed.id, "title": r.seed.title, "type": r.seed.issue_type,
                    "missing": r.missing.iter().map(|(s, _)| s).collect::<Vec<_>>(),
                    "warnings": r.missing.len(),
                    "suggestions": r.missing.iter()
                        .map(|(s, h)| serde_json::json!({"section": s, "hint": h}))
                        .collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            });
            let text = if results.is_empty() {
                "no template warnings".to_string()
            } else {
                let mut lines = vec![format!(
                    "Template warnings ({} seed{}, {warnings} warning{}):",
                    results.len(),
                    if results.len() == 1 { "" } else { "s" },
                    if warnings == 1 { "" } else { "s" }
                )];
                for r in &results {
                    lines.push(String::new());
                    lines.push(format!("{} [{}]: {}", r.seed.id, r.seed.issue_type, r.seed.title));
                    lines.extend(r.missing.iter().map(|(s, h)| format!("  missing: {s} - {h}")));
                }
                lines.join("\n")
            };
            Ok(ok(json, value, text, vec![]))
        }
        Command::Stale(a) => {
            let seeds = engine::stale(b, ctx, a.days, &a.status, at)?;
            let text = if seeds.is_empty() {
                format!("no seeds untouched for {} days", a.days)
            } else {
                seeds
                    .iter()
                    .map(|s| format!("{}  (updated {})", output::seed_line(s), s.updated_at))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(ok(json, output::seeds_json(&seeds), text, vec![]))
        }
        Command::Blocked(a) => {
            let req = engine::BlockedReq {
                types: a.issue_type.clone(),
                priorities: a.priority.clone(),
                labels: a.label.clone(),
                limit: a.limit,
            };
            let page = engine::blocked(b, &req, at)?;
            Ok(ok(
                json,
                output::blocked_json(&page),
                output::blocked_text(&page),
                vec![],
            ))
        }
        Command::Count(a) => {
            let req = engine::CountReq {
                filter: Filter {
                    status: a.status.clone(),
                    issue_type: a.issue_type.clone(),
                    assignee: a.assignee.clone(),
                    ..Filter::default()
                },
                by: a.by.clone(),
                include_closed: a.include_closed,
            };
            let c = engine::count(b, &req, at)?;
            Ok(ok(
                json,
                output::count_json(&c),
                output::count_text(&c),
                vec![],
            ))
        }
        Command::Update(a) => {
            let req = engine::UpdateReq {
                title: a.title.clone(),
                description: a.description.clone(),
                notes: a.notes.clone(),
                owner: a.owner.clone(),
                status: a.status.clone(),
                priority: a.priority.clone(),
                assignee: a.assignee.clone(),
                claim: a.claim,
                add_labels: a.add_label.clone(),
                remove_labels: a.remove_label.clone(),
                defer: a.defer.clone(),
                workflow_run: a.workflow_run.clone(),
            };
            let (seeds, tx) = engine::update(b, ctx, &a.ids, &req)?;
            let text = seeds
                .iter()
                .map(|s| format!("updated {}{}", output::seed_line(s), tx_note(tx)))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ok(
                json,
                output::with_tx(output::seeds_json(&seeds), Some(tx)),
                text,
                vec![],
            ))
        }
        Command::Close(a) => {
            let (seeds, tx, warnings) = engine::close_as(
                b,
                ctx,
                &a.ids,
                a.reason.as_deref(),
                a.outcome.as_deref(),
                a.force,
            )?;
            let text = seeds
                .iter()
                .map(|s| format!("closed {}{}", output::seed_line(s), tx_note(tx)))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ok(
                json,
                output::with_tx(output::seeds_json(&seeds), Some(tx)),
                text,
                warnings,
            ))
        }
        Command::Delete(a) => {
            let r = engine::delete(b, ctx, &a.ids, &a.reason, a.cascade, a.force, a.dry_run)?;
            let value = if r.preview {
                serde_json::json!({"preview": true, "would_delete": r.deleted,
                                   "cascade_delete": r.cascade,
                                   "blocked_dependents": r.blocked_dependents,
                                   "orphaned_issues": r.orphaned})
            } else {
                serde_json::json!({"deleted": r.deleted, "deleted_count": r.deleted.len(),
                                   "dependencies_removed": 0, "labels_removed": 0,
                                   "events_removed": 0, "references_updated": 0,
                                   "orphaned_issues": r.orphaned, "tx": r.tx})
            };
            let mut warnings = vec![];
            let text = if r.preview {
                if !a.dry_run {
                    warnings.push(format!(
                        "nothing deleted: {} has dependents ({}); pass --cascade to delete them \
                         too or --force to leave them pointing at a tombstone",
                        r.deleted.join(", "),
                        r.blocked_dependents.join(", ")
                    ));
                }
                format!(
                    "would delete {}{}",
                    r.deleted.join(", "),
                    if r.cascade.is_empty() {
                        String::new()
                    } else {
                        format!(" (--cascade would also delete {})", r.cascade.join(", "))
                    }
                )
            } else {
                format!("deleted {}{}", r.deleted.join(", "), tx_note(r.tx))
            };
            Ok(ok(json, value, text, warnings))
        }
        Command::Reopen(a) => {
            let r = engine::reopen(b, ctx, &a.ids, a.reason.as_deref())?;
            Ok(transitions(json, "reopened", "reopened", r))
        }
        Command::Defer(a) => {
            let r = engine::defer(b, ctx, &a.ids, a.until.as_deref())?;
            Ok(transitions(json, "deferred", "deferred", r))
        }
        Command::Undefer(a) => {
            let r = engine::undefer(b, ctx, &a.ids)?;
            Ok(transitions(json, "undeferred", "undeferred", r))
        }
        Command::Epic { command } => match command {
            EpicCommand::Status { eligible_only } => {
                let rows = engine::epic_status(b, *eligible_only, at)?;
                Ok(ok(
                    json,
                    epic_rows_json(&rows),
                    epic_rows_text(&rows),
                    vec![],
                ))
            }
            EpicCommand::CloseEligible { dry_run: true } => {
                let rows = engine::epic_status(b, true, at)?;
                Ok(ok(
                    json,
                    epic_rows_json(&rows),
                    epic_rows_text(&rows),
                    vec![],
                ))
            }
            EpicCommand::CloseEligible { dry_run: false } => {
                let (closed, skipped, tx) = engine::epic_close_eligible(b, ctx)?;
                let ids: Vec<&str> = closed.iter().map(|s| s.id.as_str()).collect();
                let mut value = serde_json::json!({"closed": ids, "count": ids.len(), "tx": tx});
                if !skipped.is_empty() {
                    value["skipped"] = skipped
                        .iter()
                        .map(|s| serde_json::json!({"id": s.id, "reason": s.reason}))
                        .collect();
                }
                let mut lines: Vec<String> = closed
                    .iter()
                    .map(|s| format!("closed {}{}", output::seed_line(s), tx_note(tx)))
                    .collect();
                lines.extend(
                    skipped
                        .iter()
                        .map(|s| format!("skipped {}: {}", s.id, s.reason)),
                );
                if lines.is_empty() {
                    lines.push("no epics are eligible to close".into());
                }
                Ok(ok(json, value, lines.join("\n"), vec![]))
            }
        },
        Command::Dep { command } => match command {
            DepCommand::Add {
                issue,
                depends_on,
                dep_type,
            } => {
                let d = engine::dep_add(b, ctx, issue, depends_on, dep_type)?;
                let text = format!(
                    "{} {} depends on {} ({}){}",
                    d.action,
                    d.issue_id,
                    d.depends_on_id,
                    d.dep_type,
                    tx_note(d.tx)
                );
                Ok(ok(json, output::dep_change_json(&d), text, vec![]))
            }
            DepCommand::Remove {
                issue,
                depends_on,
                dep_type,
            } => {
                let d = engine::dep_remove(b, ctx, issue, depends_on, dep_type)?;
                let text = format!(
                    "removed: {} no longer depends on {} ({}){}",
                    d.issue_id,
                    d.depends_on_id,
                    d.dep_type,
                    tx_note(d.tx)
                );
                Ok(ok(json, output::dep_change_json(&d), text, vec![]))
            }
            DepCommand::List { id, direction } => {
                let up = direction == "up";
                let rows = engine::dep_list(b, id, up, at)?;
                let text = if rows.is_empty() {
                    format!(
                        "{id} has no {}",
                        if up { "dependents" } else { "dependencies" }
                    )
                } else {
                    rows.iter()
                        .map(|r| {
                            let other = if up { &r.issue_id } else { &r.depends_on_id };
                            match &r.other {
                                Some(s) => {
                                    format!("{other} ({}) [{}] {}", r.dep_type, s.status, s.title)
                                }
                                None => format!("{other} ({})", r.dep_type),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Ok(ok(json, output::dep_list_json(&rows), text, vec![]))
            }
        },
        Command::Info => info_outcome(json, cfg, b),
        Command::Doctor => doctor(json, cfg, b),
        Command::Export(_)
        | Command::Import(_)
        | Command::Sync(_)
        | Command::MergeDriver(_)
        | Command::Version(_)
        | Command::Completions(_)
        | Command::Init(_)
        | Command::Where
        | Command::Schema { .. }
        | Command::Capabilities { .. }
        | Command::Config { .. }
        | Command::Query(_)
        | Command::Upgrade(_)
        | Command::Gate(_)
        | Command::Scheduler(_)
        | Command::Audit(_)
        | Command::RobotDocs(_) => Err(SdError::usage(
            "export, import, sync, merge-driver, completions, init, version, where and the mapped verbs are handled before dispatch",
        )),
        Command::Label { command } => label(json, at, ctx, b, command),
        Command::Comments { command } => match command {
            CommentsCommand::Add {
                id,
                text,
                file,
                message,
                author,
            } => {
                let given = [!text.is_empty(), file.is_some(), message.is_some()]
                    .iter()
                    .filter(|x| **x)
                    .count();
                if given > 1 {
                    return Err(SdError::usage(
                        "give the comment text one way: positional, --message or --file",
                    ));
                }
                let body = match (file, message) {
                    (Some(f), _) => read_text(f)?,
                    (None, Some(m)) => m.clone(),
                    (None, None) => text.join(" "),
                };
                let (c, tx) = engine::comment_add(b, ctx, id, &body, author.as_deref())?;
                let text = format!("added comment {} to {}{}", c.index, c.seed, tx_note(tx));
                Ok(ok(
                    json,
                    output::with_tx(c.to_json(), Some(tx)),
                    text,
                    vec![],
                ))
            }
            CommentsCommand::List { id } => {
                let cs = engine::comment_list(b, id, at)?;
                let text = if cs.is_empty() {
                    format!("{id} has no comments")
                } else {
                    cs.iter()
                        .map(|c| {
                            format!(
                                "[{}] {} ({}):\n  {}",
                                c.index,
                                c.author,
                                c.created_at,
                                c.text.replace('\n', "\n  ")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                Ok(ok(json, output::comments_json(&cs), text, vec![]))
            }
        },
    }
}

/// br's envelope for reopen/defer/undefer: `{"<key>": [...], "skipped": [...]}`
/// (`skipped` only when non-empty), plus the transaction.
fn transitions(json: bool, key: &str, verb: &str, r: engine::Transitions) -> Outcome {
    let (done, skipped, tx) = r;
    let rows: Vec<Json> = done
        .iter()
        .map(|t| {
            let mut o = serde_json::json!({"id": t.seed.id, "title": t.seed.title,
                                           "previous_status": t.previous_status,
                                           "status": t.seed.status});
            if let Some(d) = &t.seed.defer_until {
                o["defer_until"] = serde_json::json!(d);
            }
            o
        })
        .collect();
    let mut value = serde_json::json!({ key: rows, "tx": tx });
    if !skipped.is_empty() {
        value["skipped"] = skipped
            .iter()
            .map(|s| serde_json::json!({"id": s.id, "reason": s.reason}))
            .collect();
    }
    let mut lines: Vec<String> = done
        .iter()
        .map(|t| {
            let until = t
                .seed
                .defer_until
                .as_deref()
                .map(|d| format!(" until {d}"))
                .unwrap_or_default();
            format!(
                "{verb} {}{until}{}",
                output::seed_line(&t.seed),
                tx_note(tx)
            )
        })
        .collect();
    lines.extend(
        skipped
            .iter()
            .map(|s| format!("skipped {}: {}", s.id, s.reason)),
    );
    ok(json, value, lines.join("\n"), vec![])
}

/// `epic status --json`: br's array of `{epic, total_children,
/// closed_children, eligible_for_close}`.
fn epic_rows_json(rows: &[engine::EpicStatus]) -> Json {
    Json::Array(
        rows.iter()
            .map(|r| {
                serde_json::json!({"epic": r.epic.to_json(), "total_children": r.total_children,
                                   "closed_children": r.closed_children,
                                   "eligible_for_close": r.eligible_for_close})
            })
            .collect(),
    )
}

fn epic_rows_text(rows: &[engine::EpicStatus]) -> String {
    if rows.is_empty() {
        return "no open epics".to_string();
    }
    rows.iter()
        .map(|r| {
            format!(
                "{}  {}/{} children closed{}",
                output::seed_line(&r.epic),
                r.closed_children,
                r.total_children,
                if r.eligible_for_close {
                    " · eligible to close"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn label(
    json: bool,
    at: Option<u64>,
    ctx: &Ctx,
    b: &mut dyn Backend,
    command: &LabelCommand,
) -> Result<Outcome> {
    match command {
        LabelCommand::Add { issues, label } | LabelCommand::Remove { issues, label } => {
            let add = matches!(command, LabelCommand::Add { .. });
            let verb = if add { "add" } else { "remove" };
            let (ids, label) = LabelCommand::ids_and_label(issues, label).ok_or_else(|| {
                SdError::usage(format!(
                    "usage: label {verb} <id...> <label> or label {verb} <id...> -l <label>"
                ))
            })?;
            let (changes, tx) = engine::label_change(b, ctx, &ids, &label, add)?;
            let value = Json::Array(
                changes
                    .iter()
                    .map(|c| {
                        serde_json::json!({"status": c.status, "issue_id": c.issue_id,
                                           "label": c.label, "tx": tx})
                    })
                    .collect(),
            );
            let text = changes
                .iter()
                .map(|c| match c.status {
                    "added" => format!("added label {} to {}{}", c.label, c.issue_id, tx_note(tx)),
                    "removed" => format!(
                        "removed label {} from {}{}",
                        c.label,
                        c.issue_id,
                        tx_note(tx)
                    ),
                    "exists" => format!("{} already has label {}", c.issue_id, c.label),
                    _ => format!("{} has no label {}", c.issue_id, c.label),
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ok(json, value, text, vec![]))
        }
        LabelCommand::List { issue } => {
            let labels = engine::labels(b, issue.as_deref(), at)?;
            let text = match (issue, labels.is_empty()) {
                (Some(id), true) => format!("{id} has no labels"),
                (None, true) => "no labels in use".to_string(),
                _ => labels.join("\n"),
            };
            Ok(ok(json, serde_json::json!(labels), text, vec![]))
        }
        LabelCommand::ListAll => {
            let counts = engine::label_counts(b, at)?;
            let value = Json::Array(
                counts
                    .iter()
                    .map(|(l, n)| serde_json::json!({"label": l, "count": n}))
                    .collect(),
            );
            let text = if counts.is_empty() {
                "no labels in use".to_string()
            } else {
                counts
                    .iter()
                    .map(|(l, n)| format!("{l} ({n} seed{})", if *n == 1 { "" } else { "s" }))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(ok(json, value, text, vec![]))
        }
        LabelCommand::Rename { old_name, new_name } => {
            let (n, tx) = engine::label_rename(b, ctx, old_name, new_name)?;
            let value = serde_json::json!({"old_name": old_name, "new_name": new_name,
                                           "affected_issues": n, "tx": tx});
            let text = format!(
                "renamed label {old_name} -> {new_name} on {n} seed{}{}",
                if n == 1 { "" } else { "s" },
                if n > 0 { tx_note(tx) } else { String::new() }
            );
            Ok(ok(json, value, text, vec![]))
        }
    }
}

/// The process entry point used by `src/main.rs`.
pub fn main_entry() -> i32 {
    use clap::Parser;
    let cli = Cli::parse();
    let o = run(&cli);
    if !o.stdout.is_empty() {
        println!("{}", o.stdout);
    }
    if !o.stderr.is_empty() {
        eprintln!("{}", o.stderr);
    }
    o.code
}

#[cfg(test)]
mod changelog_tests {
    use super::to_utc;

    #[test]
    fn git_dates_convert_to_utc_both_sides_of_zero() {
        assert_eq!(
            to_utc("2026-09-30T14:00:00-04:00").as_deref(),
            Some("2026-09-30T18:00:00Z")
        );
        assert_eq!(
            to_utc("2026-09-30T01:30:00+02:30").as_deref(),
            Some("2026-09-29T23:00:00Z")
        );
        assert_eq!(
            to_utc("2026-12-31T23:00:00-02:00").as_deref(),
            Some("2027-01-01T01:00:00Z")
        );
        assert_eq!(
            to_utc("2026-09-30T14:00:00Z").as_deref(),
            Some("2026-09-30T14:00:00Z")
        );
        assert_eq!(
            to_utc("2026-09-30T14:00:00+00:00").as_deref(),
            Some("2026-09-30T14:00:00Z")
        );
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;
    use crate::backend::{SeedWrite, WriteBatch};
    use crate::model::Seed;

    fn cfg() -> Resolved {
        let dir = std::env::temp_dir().join(format!("seeds-doctor-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        config::resolve(&config::Inputs {
            cwd: dir,
            ..config::Inputs::default()
        })
        .unwrap()
    }

    fn seed(id: &str, blocked_on: &[&str]) -> SeedWrite {
        SeedWrite {
            seed: Seed {
                id: id.into(),
                title: id.into(),
                status: "open".into(),
                priority: 2,
                issue_type: "task".into(),
                created_at: "2026-09-30T00:00:00Z".into(),
                updated_at: "2026-09-30T00:00:00Z".into(),
                revision: 1,
                blocked_on: blocked_on.iter().map(|s| s.to_string()).collect(),
                ..Seed::default()
            },
            expected_revision: None,
        }
    }

    fn run(b: &mut QuipuBackend) -> (i32, Json) {
        let o = doctor(true, &cfg(), b).unwrap();
        (o.code, serde_json::from_str(&o.stdout).unwrap())
    }

    fn status(v: &Json, name: &str) -> String {
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .map(|c| c["status"].as_str().unwrap().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn a_healthy_ledger_passes_and_a_blocks_cycle_fails_with_exit_1() {
        let ctx = Ctx {
            now: "2026-09-30T00:00:00Z".into(),
            actor: "t".into(),
            prefix: "sd".into(),
        };
        let mut b = QuipuBackend::in_memory("https://seeds.local/project/doctor").unwrap();
        b.commit(
            &WriteBatch {
                seeds: vec![seed("sd-a", &[]), seed("sd-b", &["sd-a"])],
                source: "test".into(),
                ..WriteBatch::default()
            },
            &ctx,
        )
        .unwrap();
        let (code, v) = run(&mut b);
        assert_eq!(
            (code, status(&v, "deps.no_cycles"), v["ok"].clone()),
            (0, "ok".into(), Json::Bool(true)),
            "{v}"
        );
        // A cycle cannot be made through the verbs (dep add refuses it), but a
        // raw write or a bad merge can: doctor must see it.
        let snap = b.snapshot(None).unwrap();
        let mut a = snap.get("sd-a").unwrap().clone();
        a.blocked_on.insert("sd-b".into());
        a.revision += 1;
        b.commit(
            &WriteBatch {
                seeds: vec![SeedWrite {
                    seed: a,
                    expected_revision: Some(1),
                }],
                source: "test".into(),
                ..WriteBatch::default()
            },
            &ctx,
        )
        .unwrap();
        let (code, v) = run(&mut b);
        assert_eq!(code, 1, "{v}");
        assert_eq!(status(&v, "deps.no_cycles"), "error");
        assert_eq!(v["ok"], false);
    }
}

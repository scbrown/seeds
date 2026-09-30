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
use cli::{Cli, Command, CommentsCommand, DepCommand, LabelCommand};
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
    let result = config::Inputs::from_env(cli.store.clone(), cli.quipu.clone(), cli.graph.clone())
        .and_then(|i| config::resolve(&i))
        .and_then(|cfg| run_with(cli, &cfg));
    match result {
        Ok(o) => o,
        Err(e) => error_outcome(cli.json, &e, Vec::new()),
    }
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
        _ => dispatch(cli, ctx, &mut remote),
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
        let o = dispatch(cli, ctx, &mut h.backend)?;
        if cli.command.writes() {
            store::export_to_pendant(&h.backend, path, dir)?;
        }
        return Ok(with_notes(o, notes));
    }
    if cli.command.writes() {
        let mut h = store::open_for_write(path, cfg.graph())?;
        return dispatch(cli, ctx, &mut h.backend);
    }
    match store::open_for_read(path, cfg.graph())? {
        Some(mut b) => dispatch(cli, ctx, &mut b),
        None => {
            // Nothing written yet: answer from an empty graph, and say so,
            // so an empty answer is never mistaken for "nothing matched".
            let mut empty = QuipuBackend::in_memory(cfg.graph())?;
            let o = dispatch(cli, ctx, &mut empty)?;
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

fn dispatch(cli: &Cli, ctx: &Ctx, b: &mut dyn Backend) -> Result<Outcome> {
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
                labels: split_csv(&a.labels),
                parent: a.parent.clone(),
                deps: split_csv(&a.deps),
                workflow_run: a.workflow_run.clone(),
                dry_run: a.dry_run,
            };
            let (seed, tx) = engine::create(b, ctx, &req)?;
            let text = if a.silent {
                seed.id.clone()
            } else if a.dry_run {
                format!("would create {}", output::seed_line(&seed))
            } else {
                format!("created {}{}", output::seed_line(&seed), tx_note(tx))
            };
            let tx = (!a.dry_run).then_some(tx);
            Ok(ok(json, output::with_tx(seed.to_json(), tx), text, vec![]))
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
            let (seeds, tx, warnings) =
                engine::close(b, ctx, &a.ids, a.reason.as_deref(), a.force)?;
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
        Command::Export(_) | Command::Import(_) | Command::Sync(_) | Command::MergeDriver(_) => {
            Err(SdError::usage(
                "export, import and sync are handled before dispatch",
            ))
        }
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

//! The native `sd` CLI: arguments, configuration, the store file, the clock.
//!
//! Everything in this module may touch the filesystem, the environment and the
//! system clock; nothing outside it may (enforced by `clippy.toml`'s
//! disallowed lists, which this module opts out of). It turns a parsed
//! [`cli::Cli`] into calls on [`crate::engine`] and prints the result.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod cli;
pub mod config;
pub mod store;

use serde_json::Value as Json;

use crate::backend::{Backend, Ctx};
use crate::engine::{self, Filter};
use crate::error::{ErrorKind, Result, SdError};
use crate::output;
use crate::quipu_backend::QuipuBackend;
use cli::{Cli, Command, CommentsCommand, DepCommand};
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
pub fn run_with(cli: &Cli, cfg: &Resolved) -> Result<Outcome> {
    let path = match &cfg.location {
        Location::Store(p) => p.clone(),
        Location::Url(url) => {
            return Err(match store::probe(url) {
                Err(why) => SdError::new(
                    ErrorKind::Unreachable,
                    format!(
                        "cannot reach quipu at {url} (from {}): {why}. seeds does not fall back \
                         to a local store, because that would fork the ledger.",
                        cfg.location_source
                    ),
                ),
                Ok(()) => SdError::new(
                    ErrorKind::NotBuilt,
                    format!(
                        "quipu at {url} (from {}) is reachable, but the shared-server backend \
                         is not built yet; use a local store ([quipu] store) for now.",
                        cfg.location_source
                    ),
                ),
            });
        }
    };
    let ctx = Ctx {
        now: now(),
        actor: actor(cli),
        prefix: cfg.prefix.clone(),
    };
    if cli.command.writes() {
        if cli.at.is_some() {
            return Err(SdError::usage(
                "--at pins a read; a write always applies to the current state",
            ));
        }
        let mut h = store::open_for_write(&path, &cfg.graph)?;
        dispatch(cli, &ctx, &mut h.backend)
    } else {
        match store::open_for_read(&path, &cfg.graph)? {
            Some(mut b) => dispatch(cli, &ctx, &mut b),
            None => {
                // Nothing written yet: answer from an empty graph, and say so,
                // so an empty answer is never mistaken for "nothing matched".
                let mut empty = QuipuBackend::in_memory(&cfg.graph)?;
                let mut o = dispatch(cli, &ctx, &mut empty)?;
                let note = format!(
                    "sd: no seeds store at {} yet (it is created on first write)",
                    path.display()
                );
                o.stderr = if o.stderr.is_empty() {
                    note
                } else {
                    format!("{note}\n{}", o.stderr)
                };
                Ok(o)
            }
        }
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
                dry_run: a.dry_run,
            };
            let (seed, tx) = engine::create(b, ctx, &req)?;
            let text = if a.silent {
                seed.id.clone()
            } else if a.dry_run {
                format!("would create {}", output::seed_line(&seed))
            } else {
                format!("created {} (tx {tx})", output::seed_line(&seed))
            };
            Ok(ok(json, seed.to_json(), text, vec![]))
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
            };
            let (seeds, tx) = engine::update(b, ctx, &a.ids, &req)?;
            let text = seeds
                .iter()
                .map(|s| format!("updated {} (tx {tx})", output::seed_line(s)))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ok(json, output::seeds_json(&seeds), text, vec![]))
        }
        Command::Close(a) => {
            let (seeds, tx, warnings) =
                engine::close(b, ctx, &a.ids, a.reason.as_deref(), a.force)?;
            let text = seeds
                .iter()
                .map(|s| format!("closed {} (tx {tx})", output::seed_line(s)))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ok(json, output::seeds_json(&seeds), text, warnings))
        }
        Command::Dep { command } => match command {
            DepCommand::Add {
                issue,
                depends_on,
                dep_type,
            } => {
                let d = engine::dep_add(b, ctx, issue, depends_on, dep_type)?;
                let text = format!(
                    "{} {} depends on {} ({}) (tx {})",
                    d.action, d.issue_id, d.depends_on_id, d.dep_type, d.tx
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
                    "removed: {} no longer depends on {} ({}) (tx {})",
                    d.issue_id, d.depends_on_id, d.dep_type, d.tx
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
                let text = format!("added comment {} to {} (tx {tx})", c.index, c.seed);
                Ok(ok(json, c.to_json(), text, vec![]))
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

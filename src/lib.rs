//! seeds: beads-shaped work items stored as facts in a quipu knowledge graph.
//!
//! v0 is a shell. Every verb parses a br/bd-compatible subset of flags and then
//! refuses with a distinct exit code, so a caller (or a desire-path capture)
//! can tell which verb was asked for and record the demand.

use clap::{Args, Parser, Subcommand};

/// The stderr line every unimplemented verb prints.
pub fn not_yet_message(verb: &str) -> String {
    format!("seeds: {verb} not yet implemented (see docs/book)")
}

#[derive(Debug, Parser)]
#[command(
    name = "sd",
    version,
    about = "Beads-shaped work items as facts in a quipu graph (experiment)",
    long_about = "seeds is an experiment testing fact-level versus snapshot versioning \
                  of work items. Reads go to quipu SPARQL, writes to quipu knots. \
                  v0 is a shell: every verb exits with a distinct 'not yet' code."
)]
pub struct Cli {
    /// Output as JSON, in bd's output shape
    #[arg(long, global = true)]
    pub json: bool,

    /// Actor name for attribution
    #[arg(long, global = true, env = "SEEDS_ACTOR")]
    pub actor: Option<String>,

    /// Base URL of the quipu server
    #[arg(long, global = true, env = "SEEDS_QUIPU_URL")]
    pub quipu: Option<String>,

    /// Named graph (project) to read and write
    #[arg(long, global = true, env = "SEEDS_GRAPH")]
    pub graph: Option<String>,

    /// Resolve reads as of this quipu transaction (a pin)
    #[arg(long, global = true, value_name = "TX")]
    pub at: Option<u64>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a seed
    Create(CreateArgs),
    /// Show one or more seeds
    Show(ShowArgs),
    /// List seeds
    List(ListArgs),
    /// List open seeds with no open blockers
    Ready(ReadyArgs),
    /// Count seeds
    Count(CountArgs),
    /// Update fields on a seed
    Update(UpdateArgs),
    /// Close one or more seeds
    Close(CloseArgs),
    /// Manage dependencies
    Dep {
        #[command(subcommand)]
        command: DepCommand,
    },
    /// Manage comments
    Comments {
        #[command(subcommand)]
        command: CommentsCommand,
    },
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Title (positional form)
    pub title: Option<String>,
    /// Title (flag form)
    #[arg(long = "title", value_name = "TITLE")]
    pub title_flag: Option<String>,
    #[arg(short = 't', long = "type")]
    pub issue_type: Option<String>,
    #[arg(short, long)]
    pub priority: Option<String>,
    #[arg(short, long)]
    pub description: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub description_file: Option<String>,
    #[arg(short, long)]
    pub assignee: Option<String>,
    #[arg(short, long)]
    pub labels: Option<String>,
    #[arg(long)]
    pub parent: Option<String>,
    #[arg(long)]
    pub deps: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub silent: bool,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[arg(short, long)]
    pub status: Option<String>,
    #[arg(short = 't', long = "type")]
    pub issue_type: Option<String>,
    #[arg(long)]
    pub assignee: Option<String>,
    #[arg(long)]
    pub unassigned: bool,
    #[arg(short, long)]
    pub label: Vec<String>,
    #[arg(short, long)]
    pub priority: Option<String>,
    #[arg(short, long)]
    pub all: bool,
    #[arg(long)]
    pub limit: Option<usize>,
    #[arg(long)]
    pub sort: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReadyArgs {
    #[arg(long)]
    pub limit: Option<usize>,
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub assignee: Option<String>,
    #[arg(long)]
    pub unassigned: bool,
    #[arg(short, long)]
    pub label: Vec<String>,
    #[arg(short = 't', long = "type")]
    pub issue_type: Option<String>,
    #[arg(short, long)]
    pub priority: Option<String>,
    #[arg(long)]
    pub parent: Option<String>,
}

#[derive(Debug, Args)]
pub struct CountArgs {
    #[arg(long)]
    pub by: Option<String>,
    #[arg(long)]
    pub status: Option<String>,
    #[arg(long = "type")]
    pub issue_type: Option<String>,
    #[arg(long)]
    pub assignee: Option<String>,
    #[arg(long)]
    pub include_closed: bool,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(short, long)]
    pub description: Option<String>,
    #[arg(long)]
    pub notes: Option<String>,
    #[arg(short, long)]
    pub status: Option<String>,
    #[arg(short, long)]
    pub priority: Option<String>,
    #[arg(long)]
    pub assignee: Option<String>,
    /// Atomically claim: set assignee and in_progress if unclaimed
    #[arg(long)]
    pub claim: bool,
    #[arg(long)]
    pub add_label: Vec<String>,
    #[arg(long)]
    pub remove_label: Vec<String>,
    #[arg(long)]
    pub defer: Option<String>,
}

#[derive(Debug, Args)]
pub struct CloseArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
    #[arg(short, long)]
    pub reason: Option<String>,
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Debug, Subcommand)]
pub enum DepCommand {
    /// Add a dependency: <issue> depends on <depends-on>
    Add {
        issue: String,
        depends_on: String,
        #[arg(short = 't', long = "type", default_value = "blocks")]
        dep_type: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum CommentsCommand {
    /// Add a comment to a seed
    Add {
        id: String,
        text: Vec<String>,
        #[arg(short, long)]
        file: Option<String>,
        #[arg(short, long, alias = "content")]
        message: Option<String>,
        #[arg(long)]
        author: Option<String>,
    },
}

/// Exit code for each unimplemented verb. Distinct, non-zero, and clear of
/// clap's usage-error code (2), so a caller can tell which verb was refused.
pub fn not_yet_code(command: &Command) -> i32 {
    match command {
        Command::Create(_) => 10,
        Command::Show(_) => 11,
        Command::List(_) => 12,
        Command::Ready(_) => 13,
        Command::Count(_) => 14,
        Command::Update(_) => 15,
        Command::Close(_) => 16,
        Command::Dep { .. } => 17,
        Command::Comments { .. } => 18,
    }
}

/// The verb name as a user typed it.
pub fn verb_name(command: &Command) -> &'static str {
    match command {
        Command::Create(_) => "create",
        Command::Show(_) => "show",
        Command::List(_) => "list",
        Command::Ready(_) => "ready",
        Command::Count(_) => "count",
        Command::Update(_) => "update",
        Command::Close(_) => "close",
        Command::Dep {
            command: DepCommand::Add { .. },
        } => "dep add",
        Command::Comments {
            command: CommentsCommand::Add { .. },
        } => "comments add",
    }
}

/// Run a parsed command. Returns the process exit code and the stderr line.
pub fn run(cli: &Cli) -> (i32, String) {
    (
        not_yet_code(&cli.command),
        not_yet_message(verb_name(&cli.command)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use std::collections::HashSet;

    const CASES: &[(&[&str], i32, &str)] = &[
        (&["create", "a title", "-p", "1", "--json"], 10, "create"),
        (&["show", "s-1", "--json"], 11, "show"),
        (&["list", "--status", "open", "--limit", "0"], 12, "list"),
        (&["ready", "--json", "--limit", "5"], 13, "ready"),
        (&["count", "--by", "status"], 14, "count"),
        (&["update", "s-1", "--claim", "--json"], 15, "update"),
        (&["close", "s-1", "--reason", "done"], 16, "close"),
        (&["dep", "add", "s-1", "s-2"], 17, "dep add"),
        (
            &["comments", "add", "s-1", "hello", "--json"],
            18,
            "comments add",
        ),
    ];

    fn parse(args: &[&str]) -> Cli {
        let mut argv = vec!["seeds"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).expect("parses")
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_verb_parses_and_returns_its_not_yet_code() {
        for (args, code, verb) in CASES {
            let cli = parse(args);
            let (got, msg) = run(&cli);
            assert_eq!(got, *code, "exit code for {verb}");
            assert_eq!(
                msg,
                format!("seeds: {verb} not yet implemented (see docs/book)")
            );
        }
    }

    #[test]
    fn not_yet_codes_are_distinct_and_nonzero_and_not_usage() {
        let codes: HashSet<i32> = CASES.iter().map(|(_, c, _)| *c).collect();
        assert_eq!(codes.len(), CASES.len());
        assert!(codes.iter().all(|c| *c != 0 && *c != 2));
    }

    #[test]
    fn json_flag_is_accepted_on_every_verb() {
        for (args, _, _) in CASES {
            let mut with_json: Vec<&str> =
                args.iter().copied().filter(|a| *a != "--json").collect();
            with_json.push("--json");
            assert!(parse(&with_json).json);
        }
    }

    #[test]
    fn help_works() {
        let err = Cli::try_parse_from(["seeds", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(err.to_string().contains("ready"));
        let err = Cli::try_parse_from(["seeds", "dep", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn version_works() {
        let err = Cli::try_parse_from(["seeds", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn unknown_verb_is_a_usage_error_not_a_not_yet() {
        let err = Cli::try_parse_from(["seeds", "sync"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn pin_flag_parses_as_a_transaction() {
        let cli = parse(&["show", "s-1", "--at", "42"]);
        assert_eq!(cli.at, Some(42));
    }
}

//! The `sd` command line. Flags follow br, so what agents already type parses.

use clap::{Args, Parser, Subcommand};

/// `sd`: beads-shaped work items as facts in a quipu graph.
#[derive(Debug, Parser)]
#[command(
    name = "sd",
    version,
    about = "Beads-compatible work items as facts in a quipu graph",
    long_about = "seeds is the beads-compatible tracker for the quipu stack, with \
                  fact-level versioning of work items. Each work item is a set of facts in a quipu knowledge \
                  graph; every write is one quipu transaction, and --at <tx> reads any \
                  seed as it stood at that transaction.\n\n\
                  Where the graph lives comes from --store/--quipu, then SEEDS_QUIPU_STORE/\
                  SEEDS_QUIPU_URL, then .seeds/config.toml (found by walking up from the \
                  current directory), then ~/.config/seeds/config.toml, then the default: \
                  a local store at .seeds/seeds.db, created on first write."
)]
pub struct Cli {
    /// Output as JSON, in bd's output shape
    #[arg(long, global = true)]
    pub json: bool,

    /// Actor name recorded on writes and used by --claim (default: $USER)
    #[arg(long, global = true, env = "SEEDS_ACTOR")]
    pub actor: Option<String>,

    /// Local quipu store file to use (overrides config)
    #[arg(long, global = true, value_name = "PATH", conflicts_with = "quipu")]
    pub store: Option<String>,

    /// URL of a shared quipu server to use (overrides config)
    #[arg(long, global = true, value_name = "URL")]
    pub quipu: Option<String>,

    /// Named graph (project) IRI to read and write (overrides config)
    #[arg(long, global = true, value_name = "IRI")]
    pub graph: Option<String>,

    /// Read as of this quipu transaction (a pin). Reads only.
    #[arg(long, global = true, value_name = "TX")]
    pub at: Option<u64>,

    #[command(subcommand)]
    pub command: Command,
}

/// The verbs.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a seed
    Create(CreateArgs),
    /// Show one or more seeds, with dependencies and comments
    Show(ShowArgs),
    /// List seeds (open ones by default; 50 at a time, --limit 0 for all)
    List(ListArgs),
    /// List open seeds with no open blockers. Never truncated unless you pass
    /// --limit, and then it says so.
    Ready(ReadyArgs),
    /// Count seeds (open ones by default)
    Count(CountArgs),
    /// Update fields on one or more seeds
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
    /// Write the ledger as a pendant (quipu's share files) to a directory
    Export(ExportArgs),
    /// Read a pendant into the configured store; conflicts are reported, never
    /// resolved silently
    Import(ImportArgs),
    /// Three-way sync between the local store and the [sync] remote
    Sync(SyncArgs),
    /// Git merge driver for a pendant's export.nt: a field-level three-way
    /// merge of the ledgers. Register it with
    /// `git config merge.seeds.driver "sd merge-driver %O %A %B"`
    MergeDriver(MergeDriverArgs),
}

/// `sd merge-driver`.
#[derive(Debug, Args)]
pub struct MergeDriverArgs {
    /// The common ancestor's export.nt (git's %O)
    pub base: String,
    /// Ours (git's %A); the merge result is written here
    pub ours: String,
    /// Theirs (git's %B)
    pub theirs: String,
}

/// `sd export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Directory to write (default: the configured [pendant] dir)
    #[arg(long, value_name = "DIR")]
    pub to: Option<String>,
}

/// `sd import`.
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// The pendant directory to read
    pub dir: String,
    /// Where the two disagree, take this side: pendant or store
    #[arg(long, value_parser = ["pendant", "store"])]
    pub prefer: Option<String>,
    /// Make the store exactly the pendant (removes seeds the pendant lacks)
    #[arg(long, conflicts_with = "prefer")]
    pub replace: bool,
}

/// `sd sync`.
#[derive(Debug, Args)]
pub struct SyncArgs {
    /// The remote to sync with (default: [sync] remote)
    #[arg(long, value_name = "URL")]
    pub remote: Option<String>,
    /// Apply removals: seeds the last sync with this remote saw that one side
    /// no longer has. Refused by default, because it usually means the other
    /// side was reset or is a different store.
    #[arg(long)]
    pub allow_remote_deletes: bool,
}

/// `sd create`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Title (positional form)
    pub title: Option<String>,
    /// Title (flag form)
    #[arg(long = "title", value_name = "TITLE")]
    pub title_flag: Option<String>,
    /// task, bug, feature, epic, chore, docs or question (default task)
    #[arg(short = 't', long = "type")]
    pub issue_type: Option<String>,
    /// 0-4 or P0-P4 (default 2)
    #[arg(short, long)]
    pub priority: Option<String>,
    /// Description
    #[arg(short, long)]
    pub description: Option<String>,
    /// Read the description from a file
    #[arg(long, value_name = "PATH")]
    pub description_file: Option<String>,
    /// Assignee
    #[arg(short, long)]
    pub assignee: Option<String>,
    /// Comma-separated labels
    #[arg(short, long)]
    pub labels: Option<String>,
    /// Parent seed; the new seed is minted as <parent>.<n>
    #[arg(long)]
    pub parent: Option<String>,
    /// Comma-separated dependencies: <id> (blocks) or <type>:<id>
    #[arg(long)]
    pub deps: Option<String>,
    /// The shuttle workflow run that creates or drives this seed: a run IRI or
    /// a bare run id (becomes urn:shuttle:run:<id>)
    #[arg(long = "workflow-run", value_name = "RUN")]
    pub workflow_run: Option<String>,
    /// Print what would be created without writing it
    #[arg(long)]
    pub dry_run: bool,
    /// Print only the new id
    #[arg(long)]
    pub silent: bool,
}

/// `sd show`.
#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
}

/// `sd list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Only this status (includes closed when you ask for closed)
    #[arg(short, long)]
    pub status: Option<String>,
    /// Only this type
    #[arg(short = 't', long = "type")]
    pub issue_type: Option<String>,
    /// Only this assignee
    #[arg(long)]
    pub assignee: Option<String>,
    /// Only unassigned seeds
    #[arg(long)]
    pub unassigned: bool,
    /// Only seeds with this label (repeatable; all must match)
    #[arg(short, long)]
    pub label: Vec<String>,
    /// Only this priority
    #[arg(short, long)]
    pub priority: Option<String>,
    /// Include closed seeds
    #[arg(short, long)]
    pub all: bool,
    /// Page size (default 50; 0 = all). The output says when it was cut short.
    #[arg(long)]
    pub limit: Option<usize>,
    /// priority (default), created, updated, id or title
    #[arg(long)]
    pub sort: Option<String>,
}

/// `sd ready`.
#[derive(Debug, Args)]
pub struct ReadyArgs {
    /// Cut the list at N. Without it, ready is NOT truncated; with it, a
    /// truncated list says so on stderr.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Only this assignee (bare --assignee means the current actor)
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    pub assignee: Option<String>,
    /// Only unassigned seeds
    #[arg(long)]
    pub unassigned: bool,
    /// Only seeds with this label (repeatable; all must match)
    #[arg(short, long)]
    pub label: Vec<String>,
    /// Only this type
    #[arg(short = 't', long = "type")]
    pub issue_type: Option<String>,
    /// Only this priority
    #[arg(short, long)]
    pub priority: Option<String>,
    /// Only children of this seed
    #[arg(long)]
    pub parent: Option<String>,
}

/// `sd count`.
#[derive(Debug, Args)]
pub struct CountArgs {
    /// Group by status, priority, type, assignee or label
    #[arg(long)]
    pub by: Option<String>,
    /// Only this status
    #[arg(long)]
    pub status: Option<String>,
    /// Only this type
    #[arg(long = "type")]
    pub issue_type: Option<String>,
    /// Only this assignee
    #[arg(long)]
    pub assignee: Option<String>,
    /// Count closed seeds too
    #[arg(long)]
    pub include_closed: bool,
}

/// `sd update`.
#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
    /// New title
    #[arg(long)]
    pub title: Option<String>,
    /// New description
    #[arg(short, long)]
    pub description: Option<String>,
    /// New notes
    #[arg(long)]
    pub notes: Option<String>,
    /// New status: open, in_progress, blocked, deferred or closed
    #[arg(short, long)]
    pub status: Option<String>,
    /// New priority
    #[arg(short, long)]
    pub priority: Option<String>,
    /// New assignee ("" clears it)
    #[arg(long)]
    pub assignee: Option<String>,
    /// Atomically claim: set assignee to the actor and status to in_progress,
    /// only if the seed is open, unclaimed and unblocked (exit 4 otherwise)
    #[arg(long)]
    pub claim: bool,
    /// Add a label (repeatable)
    #[arg(long)]
    pub add_label: Vec<String>,
    /// Remove a label (repeatable)
    #[arg(long)]
    pub remove_label: Vec<String>,
    /// Hide from ready until this date or instant ("" clears it)
    #[arg(long)]
    pub defer: Option<String>,
    /// The shuttle workflow run driving the seed ("" clears it)
    #[arg(long = "workflow-run", value_name = "RUN")]
    pub workflow_run: Option<String>,
}

/// `sd close`.
#[derive(Debug, Args)]
pub struct CloseArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
    /// Why: what landed and how you know. Closing without one warns.
    #[arg(short, long)]
    pub reason: Option<String>,
    /// Close even if the seed still has open blockers
    #[arg(short, long)]
    pub force: bool,
}

/// `sd dep ...`.
#[derive(Debug, Subcommand)]
pub enum DepCommand {
    /// Add a dependency: <issue> depends on <depends-on>
    Add {
        /// The dependent seed
        issue: String,
        /// The seed it depends on
        depends_on: String,
        /// blocks (default), related, parent-child or discovered-from
        #[arg(short = 't', long = "type", default_value = "blocks")]
        dep_type: String,
    },
    /// Remove a dependency
    Remove {
        /// The dependent seed
        issue: String,
        /// The seed it depended on
        depends_on: String,
        /// blocks (default), related, parent-child or discovered-from
        #[arg(short = 't', long = "type", default_value = "blocks")]
        dep_type: String,
    },
    /// List a seed's dependencies (down, the default) or dependents (up)
    List {
        /// The seed
        id: String,
        /// down: what it depends on; up: what depends on it
        #[arg(long, default_value = "down", value_parser = ["down", "up"])]
        direction: String,
    },
}

/// `sd comments ...`.
#[derive(Debug, Subcommand)]
pub enum CommentsCommand {
    /// Add a comment to a seed
    Add {
        /// The seed
        id: String,
        /// Comment text (joined with spaces)
        text: Vec<String>,
        /// Read the text from a file
        #[arg(short, long)]
        file: Option<String>,
        /// Comment text (flag form)
        #[arg(short, long, alias = "content")]
        message: Option<String>,
        /// Author (default: the actor)
        #[arg(long)]
        author: Option<String>,
    },
    /// List a seed's comments
    List {
        /// The seed
        id: String,
    },
}

impl Command {
    /// Whether the verb writes (and so takes the store's write lock).
    pub fn writes(&self) -> bool {
        match self {
            Command::Create(_) | Command::Update(_) | Command::Close(_) => true,
            Command::Dep { command } => !matches!(command, DepCommand::List { .. }),
            Command::Comments { command } => matches!(command, CommentsCommand::Add { .. }),
            Command::Import(_) | Command::Sync(_) | Command::MergeDriver(_) => true,
            Command::Export(_)
            | Command::Show(_)
            | Command::List(_)
            | Command::Ready(_)
            | Command::Count(_) => false,
        }
    }

    /// The verb as a user typed it.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Create(_) => "create",
            Command::Show(_) => "show",
            Command::List(_) => "list",
            Command::Ready(_) => "ready",
            Command::Count(_) => "count",
            Command::Update(_) => "update",
            Command::Close(_) => "close",
            Command::Dep { command } => match command {
                DepCommand::Add { .. } => "dep add",
                DepCommand::Remove { .. } => "dep remove",
                DepCommand::List { .. } => "dep list",
            },
            Command::Comments { command } => match command {
                CommentsCommand::Add { .. } => "comments add",
                CommentsCommand::List { .. } => "comments list",
            },
            Command::Export(_) => "export",
            Command::Import(_) => "import",
            Command::Sync(_) => "sync",
            Command::MergeDriver(_) => "merge-driver",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    const CASES: &[(&[&str], &str, bool)] = &[
        (&["create", "a title", "-p", "1", "--json"], "create", true),
        (&["show", "s-1", "--json"], "show", false),
        (&["list", "--status", "open", "--limit", "0"], "list", false),
        (&["ready", "--json", "--limit", "5"], "ready", false),
        (&["count", "--by", "status"], "count", false),
        (&["update", "s-1", "--claim", "--json"], "update", true),
        (&["close", "s-1", "--reason", "done"], "close", true),
        (&["dep", "add", "s-1", "s-2"], "dep add", true),
        (
            &["dep", "remove", "s-1", "s-2", "-t", "related"],
            "dep remove",
            true,
        ),
        (
            &["dep", "list", "s-1", "--direction", "up"],
            "dep list",
            false,
        ),
        (
            &["comments", "add", "s-1", "hello", "--json"],
            "comments add",
            true,
        ),
        (&["comments", "list", "s-1"], "comments list", false),
        (&["export", "--to", "p"], "export", false),
        (&["import", "p", "--prefer", "store"], "import", true),
        (&["sync"], "sync", true),
        (&["merge-driver", "o", "a", "b"], "merge-driver", true),
    ];

    fn parse(args: &[&str]) -> Cli {
        let mut argv = vec!["sd"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).expect("parses")
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_verb_parses_with_its_name_and_write_class() {
        for (args, name, writes) in CASES {
            let cli = parse(args);
            assert_eq!(cli.command.name(), *name);
            assert_eq!(cli.command.writes(), *writes, "{name}");
        }
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
    fn help_and_version_work() {
        let err = Cli::try_parse_from(["sd", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(err.to_string().contains("ready"));
        let err = Cli::try_parse_from(["sd", "ready", "--help"]).unwrap_err();
        assert!(
            err.to_string().contains("NOT truncated"),
            "ready's help must say it is not truncated by default"
        );
        let err = Cli::try_parse_from(["sd", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn unknown_verb_is_a_usage_error() {
        let err = Cli::try_parse_from(["sd", "frobnicate"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn store_and_quipu_flags_conflict() {
        let err = Cli::try_parse_from(["sd", "ready", "--store", "a.db", "--quipu", "http://x"])
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn pin_flag_parses_as_a_transaction() {
        let cli = parse(&["show", "s-1", "--at", "42"]);
        assert_eq!(cli.at, Some(42));
    }
}

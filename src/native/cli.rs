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
    /// Find seeds whose id, title, description or comments contain the text
    /// (closed ones are hidden and counted unless --all)
    Search(SearchArgs),
    /// List open seeds with no open blockers. Never truncated unless you pass
    /// --limit, and then it says so.
    Ready(ReadyArgs),
    /// List seeds that are not closed and have an open blocker (50 at a time,
    /// --limit 0 for all; the output says when it was cut short)
    Blocked(BlockedArgs),
    /// List seeds not updated for --days days (default 30), oldest first
    Stale(StaleArgs),
    /// Summary counts (by status, ready, lead time) with optional breakdowns
    #[command(visible_alias = "status")]
    Stats(StatsArgs),
    /// Quick capture: create a seed and print only its id
    Q(QArgs),
    /// Saved queries live in quipu (stored queries), not in sd
    Query(MappedArgs),
    /// sd is installed and upgraded by caboodle
    Upgrade(MappedArgs),
    /// Workflow gates live in shuttle
    Gate(MappedArgs),
    /// Ranking ready work for a swarm lives in shuttle
    Scheduler(MappedArgs),
    /// Provenance lives in quipu: every sd write is a transaction with its actor
    Audit(MappedArgs),
    /// The automation docs are the book and `sd <verb> --help`
    RobotDocs(MappedArgs),
    /// What a seed unblocks (or, with --dependencies, what it waits on)
    Graph(GraphArgs),
    /// Count seeds (open ones by default)
    Count(CountArgs),
    /// Update fields on one or more seeds
    Update(UpdateArgs),
    /// Close one or more seeds
    Close(CloseArgs),
    /// Delete seeds (tombstone: hidden everywhere, kept in history)
    Delete(DeleteArgs),
    /// Reopen closed seeds
    Reopen(ReopenArgs),
    /// Defer seeds: status deferred, hidden from ready until a date
    Defer(DeferArgs),
    /// Undefer seeds: back to open, defer date cleared
    Undefer(UndeferArgs),
    /// Epic progress and closing
    Epic {
        #[command(subcommand)]
        command: EpicCommand,
    },
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
    /// Manage labels
    Label {
        #[command(subcommand)]
        command: LabelCommand,
    },
    /// Print a shell completion script (bash, zsh, fish, powershell, elvish)
    Completions(CompletionsArgs),
    /// Print the version (and how this build was made)
    Version(VersionArgs),
    /// Show where the ledger lives: store file or server, graph, prefix, pendant
    Where,
    /// Show the ledger's location, mode and size
    Info,
    /// Make the current directory a seeds project (.seeds/ with config,
    /// project id and .gitignore)
    Init(InitArgs),
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

/// `sd completions`.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// The shell to generate completions for
    pub shell: clap_complete::Shell,
    /// Write `<dir>/<file>` for the shell instead of printing to stdout
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<String>,
}

/// `sd version`.
#[derive(Debug, Args)]
pub struct VersionArgs {
    /// Print only the version number (for scripts)
    #[arg(short, long)]
    pub short: bool,
}

/// `sd init`.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Id prefix for new seeds (default: $SEEDS_PREFIX, else "sd")
    #[arg(long)]
    pub prefix: Option<String>,
    /// Restore missing files in an existing project. Never changes its id or prefix.
    #[arg(long)]
    pub force: bool,
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
    /// Owner (usually an email)
    #[arg(long)]
    pub owner: Option<String>,
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
    /// The workflow step creating this seed (needs --workflow-run). The id is
    /// derived from run, step and visit, so repeating the create returns the
    /// same seed instead of a duplicate
    #[arg(long, value_name = "STEP")]
    pub step: Option<String>,
    /// Which entry into --step this is, counting from 1 (default 1). A step the
    /// run enters again gets a new seed
    #[arg(long, value_name = "N")]
    pub visit: Option<u32>,
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

/// `sd search`.
#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Text to find (case-insensitive)
    pub query: String,
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

/// `sd blocked`.
#[derive(Debug, Args)]
pub struct BlockedArgs {
    /// Page size (default 50; 0 = all). The output says when it was cut short.
    #[arg(long)]
    pub limit: Option<usize>,
    /// Only this type (repeatable: any of them)
    #[arg(short = 't', long = "type")]
    pub issue_type: Vec<String>,
    /// Only this priority (repeatable: any of them)
    #[arg(short, long)]
    pub priority: Vec<String>,
    /// Only seeds with this label (repeatable; all must match)
    #[arg(short, long)]
    pub label: Vec<String>,
}

/// `sd stale`.
#[derive(Debug, Args)]
pub struct StaleArgs {
    /// Minimum days since the last update
    #[arg(long, default_value_t = 30)]
    pub days: u32,
    /// Only this status (repeatable or comma-separated; closed only if named)
    #[arg(long)]
    pub status: Vec<String>,
}

/// `sd stats`.
#[derive(Debug, Args)]
pub struct StatsArgs {
    /// Add a breakdown by issue type
    #[arg(long)]
    pub by_type: bool,
    /// Add a breakdown by priority
    #[arg(long)]
    pub by_priority: bool,
    /// Add a breakdown by assignee
    #[arg(long)]
    pub by_assignee: bool,
    /// Add a breakdown by label
    #[arg(long)]
    pub by_label: bool,
}

/// `sd q`.
#[derive(Debug, Args)]
pub struct QArgs {
    /// Title words (joined with spaces)
    #[arg(required = true)]
    pub title: Vec<String>,
    /// 0-4 or P0-P4 (default 2)
    #[arg(short, long)]
    pub priority: Option<String>,
    /// task, bug, feature, epic, chore, docs or question (default task)
    #[arg(short = 't', long = "type")]
    pub issue_type: Option<String>,
    /// Labels (repeatable; comma-separated allowed)
    #[arg(short, long)]
    pub labels: Vec<String>,
    /// Description
    #[arg(short, long, alias = "body")]
    pub description: Option<String>,
    /// Parent seed
    #[arg(long)]
    pub parent: Option<String>,
}

/// A br verb whose capability lives elsewhere in the stack: any arguments are
/// accepted so the pointer is printed instead of a usage error.
#[derive(Debug, Args)]
pub struct MappedArgs {
    /// Ignored
    #[arg(allow_hyphen_values = true)]
    pub rest: Vec<String>,
}

/// `sd graph`.
#[derive(Debug, Args)]
pub struct GraphArgs {
    /// The root seed (required unless --all)
    #[arg(required_unless_present = "all")]
    pub issue: Option<String>,
    /// Every open, in-progress or blocked seed, as connected components
    #[arg(long, conflicts_with = "issue")]
    pub all: bool,
    /// Walk what the seed waits on instead of what waits on it
    #[arg(long)]
    pub dependencies: bool,
    /// One line per seed
    #[arg(long)]
    pub compact: bool,
    /// Graphviz DOT (pipe to `dot -Tsvg`); overrides text and --json
    #[arg(long)]
    pub dot: bool,
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
    /// New owner (usually an email; "" clears it)
    #[arg(long)]
    pub owner: Option<String>,
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
    /// How it ended: done (default), abandoned, superseded or failed. Stored
    /// as a field, so a workflow can branch on it without parsing the reason
    #[arg(long, value_name = "OUTCOME")]
    pub outcome: Option<String>,
    /// Close even if the seed still has open blockers
    #[arg(short, long)]
    pub force: bool,
}

/// `sd delete`.
#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
    /// Why (stored as the comment "Deleted: <reason>")
    #[arg(long, default_value = "delete")]
    pub reason: String,
    /// Delete dependents too, recursively
    #[arg(long, conflicts_with = "force")]
    pub cascade: bool,
    /// Delete even with dependents; they are left pointing at a tombstone,
    /// which never blocks
    #[arg(long)]
    pub force: bool,
    /// Show what would be deleted; write nothing
    #[arg(long)]
    pub dry_run: bool,
}

/// `sd reopen`.
#[derive(Debug, Args)]
pub struct ReopenArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
    /// Why (stored as the comment "Reopened: <reason>")
    #[arg(short, long)]
    pub reason: Option<String>,
}

/// `sd defer`.
#[derive(Debug, Args)]
pub struct DeferArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
    /// Until when: +30m, +2h, +1d, +1w, tomorrow, YYYY-MM-DD or an RFC 3339
    /// instant (none: deferred with no date)
    #[arg(long)]
    pub until: Option<String>,
}

/// `sd undefer`.
#[derive(Debug, Args)]
pub struct UndeferArgs {
    /// Seed id(s)
    #[arg(required = true)]
    pub ids: Vec<String>,
}

/// `sd epic ...`.
#[derive(Debug, Subcommand)]
pub enum EpicCommand {
    /// Every epic that is not closed, with child progress and eligibility
    Status {
        /// Only epics whose children are all closed
        #[arg(long)]
        eligible_only: bool,
    },
    /// Close every epic whose children are all closed
    CloseEligible {
        /// List what would be closed without writing
        #[arg(long)]
        dry_run: bool,
    },
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

/// `sd label ...`. Same shapes as br: `label add <id...> <label>` or
/// `label add <id...> -l <label>`.
#[derive(Debug, Subcommand)]
pub enum LabelCommand {
    /// Add a label to one or more seeds
    Add {
        /// Seed id(s), then the label unless -l is given
        issues: Vec<String>,
        /// Label to add
        #[arg(short, long)]
        label: Option<String>,
    },
    /// Remove a label from one or more seeds
    Remove {
        /// Seed id(s), then the label unless -l is given
        issues: Vec<String>,
        /// Label to remove
        #[arg(short, long)]
        label: Option<String>,
    },
    /// List a seed's labels, or every label in use when no id is given
    List {
        /// The seed (optional)
        issue: Option<String>,
    },
    /// List every label in use, with how many seeds carry it
    ListAll,
    /// Rename a label on every seed that carries it
    Rename {
        /// Current label
        old_name: String,
        /// New label
        new_name: String,
    },
}

impl LabelCommand {
    /// Split `add`/`remove` arguments into (ids, label) the way br does: with
    /// -l every positional is an id; without it the last positional is the label.
    pub fn ids_and_label(
        issues: &[String],
        label: &Option<String>,
    ) -> Option<(Vec<String>, String)> {
        match label {
            Some(l) if !issues.is_empty() => Some((issues.to_vec(), l.clone())),
            None if issues.len() >= 2 => {
                let (last, ids) = issues.split_last()?;
                Some((ids.to_vec(), last.clone()))
            }
            _ => None,
        }
    }
}

impl Cli {
    /// Whether JSON output was asked for. A mapped verb accepts any arguments
    /// (so the pointer is printed instead of a usage error), which can swallow
    /// a trailing `--json`; it still counts.
    pub fn wants_json(&self) -> bool {
        self.json
            || match &self.command {
                Command::Query(a)
                | Command::Upgrade(a)
                | Command::Gate(a)
                | Command::Scheduler(a)
                | Command::Audit(a)
                | Command::RobotDocs(a) => a.rest.iter().any(|r| r == "--json"),
                _ => false,
            }
    }
}

impl Command {
    /// Whether the verb writes (and so takes the store's write lock).
    pub fn writes(&self) -> bool {
        match self {
            Command::Q(_) => true,
            Command::Create(_)
            | Command::Update(_)
            | Command::Close(_)
            | Command::Reopen(_)
            | Command::Delete(_)
            | Command::Defer(_)
            | Command::Undefer(_) => true,
            Command::Dep { command } => !matches!(command, DepCommand::List { .. }),
            Command::Epic { command } => {
                matches!(command, EpicCommand::CloseEligible { dry_run: false })
            }
            Command::Comments { command } => matches!(command, CommentsCommand::Add { .. }),
            Command::Label { command } => matches!(
                command,
                LabelCommand::Add { .. }
                    | LabelCommand::Remove { .. }
                    | LabelCommand::Rename { .. }
            ),
            Command::Import(_) | Command::Sync(_) | Command::MergeDriver(_) | Command::Init(_) => {
                true
            }
            Command::Export(_)
            | Command::Completions(_)
            | Command::Version(_)
            | Command::Where
            | Command::Info
            | Command::Show(_)
            | Command::List(_)
            | Command::Ready(_)
            | Command::Blocked(_)
            | Command::Stale(_)
            | Command::Graph(_)
            | Command::Stats(_)
            | Command::Query(_)
            | Command::Upgrade(_)
            | Command::Gate(_)
            | Command::Scheduler(_)
            | Command::Audit(_)
            | Command::RobotDocs(_)
            | Command::Search(_)
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
            Command::Blocked(_) => "blocked",
            Command::Stale(_) => "stale",
            Command::Graph(_) => "graph",
            Command::Stats(_) => "stats",
            Command::Q(_) => "q",
            Command::Query(_) => "query",
            Command::Upgrade(_) => "upgrade",
            Command::Gate(_) => "gate",
            Command::Scheduler(_) => "scheduler",
            Command::Audit(_) => "audit",
            Command::RobotDocs(_) => "robot-docs",
            Command::Search(_) => "search",
            Command::Count(_) => "count",
            Command::Update(_) => "update",
            Command::Close(_) => "close",
            Command::Reopen(_) => "reopen",
            Command::Delete(_) => "delete",
            Command::Defer(_) => "defer",
            Command::Undefer(_) => "undefer",
            Command::Epic { command } => match command {
                EpicCommand::Status { .. } => "epic status",
                EpicCommand::CloseEligible { .. } => "epic close-eligible",
            },
            Command::Dep { command } => match command {
                DepCommand::Add { .. } => "dep add",
                DepCommand::Remove { .. } => "dep remove",
                DepCommand::List { .. } => "dep list",
            },
            Command::Comments { command } => match command {
                CommentsCommand::Add { .. } => "comments add",
                CommentsCommand::List { .. } => "comments list",
            },
            Command::Label { command } => match command {
                LabelCommand::Add { .. } => "label add",
                LabelCommand::Remove { .. } => "label remove",
                LabelCommand::List { .. } => "label list",
                LabelCommand::ListAll => "label list-all",
                LabelCommand::Rename { .. } => "label rename",
            },
            Command::Completions(_) => "completions",
            Command::Version(_) => "version",
            Command::Where => "where",
            Command::Info => "info",
            Command::Init(_) => "init",
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
        (
            &["graph", "s-1", "--dependencies", "--compact"],
            "graph",
            false,
        ),
        (&["graph", "--all", "--dot"], "graph", false),
        (&["stats", "--by-type", "--by-label"], "stats", false),
        (&["status"], "stats", false),
        (&["q", "a", "quick", "one", "-p", "1"], "q", true),
        (&["query", "list", "--json"], "query", false),
        (&["upgrade"], "upgrade", false),
        (
            &["stale", "--days", "7", "--status", "open,in_progress"],
            "stale",
            false,
        ),
        (
            &["search", "lexer", "-a", "--limit", "0", "-l", "x"],
            "search",
            false,
        ),
        (
            &[
                "blocked", "-t", "bug", "-t", "task", "-p", "1", "--limit", "0",
            ],
            "blocked",
            false,
        ),
        (&["update", "s-1", "--claim", "--json"], "update", true),
        (&["close", "s-1", "--reason", "done"], "close", true),
        (&["reopen", "s-1", "-r", "not done"], "reopen", true),
        (
            &["delete", "s-1", "--cascade", "--reason", "dup"],
            "delete",
            true,
        ),
        (&["defer", "s-1", "s-2", "--until", "+1d"], "defer", true),
        (&["undefer", "s-1"], "undefer", true),
        (&["dep", "add", "s-1", "s-2"], "dep add", true),
        (&["epic", "status", "--eligible-only"], "epic status", false),
        (&["epic", "close-eligible"], "epic close-eligible", true),
        (
            &["epic", "close-eligible", "--dry-run"],
            "epic close-eligible",
            false,
        ),
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
        (&["label", "add", "s-1", "s-2", "l"], "label add", true),
        (&["label", "add", "s-1", "-l", "l"], "label add", true),
        (&["label", "remove", "s-1", "l"], "label remove", true),
        (&["label", "list", "s-1"], "label list", false),
        (&["label", "list"], "label list", false),
        (&["label", "list-all"], "label list-all", false),
        (&["label", "rename", "a", "b"], "label rename", true),
        (&["export", "--to", "p"], "export", false),
        (&["init", "--prefix", "ab"], "init", true),
        (&["version", "--short"], "version", false),
        (&["completions", "bash"], "completions", false),
        (&["where"], "where", false),
        (&["info"], "info", false),
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
            assert!(parse(&with_json).wants_json());
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
    fn label_arguments_split_like_br() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let split = LabelCommand::ids_and_label;
        assert_eq!(
            split(&v(&["s-1", "s-2", "l"]), &None),
            Some((v(&["s-1", "s-2"]), "l".into()))
        );
        assert_eq!(
            split(&v(&["s-1", "s-2"]), &Some("l".into())),
            Some((v(&["s-1", "s-2"]), "l".into()))
        );
        assert_eq!(split(&v(&["s-1"]), &None), None);
        assert_eq!(split(&[], &Some("l".into())), None);
    }

    #[test]
    fn pin_flag_parses_as_a_transaction() {
        let cli = parse(&["show", "s-1", "--at", "42"]);
        assert_eq!(cli.at, Some(42));
    }
}

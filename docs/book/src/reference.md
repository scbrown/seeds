# Reference

The command is `sd` (crate `seeds-ai`). Each verb has its own page under
[The verbs](verbs/README.md); this page is the summary.

## Global options

Accepted on every verb.

| option | meaning |
|---|---|
| `--json` | output as JSON, in br's output shape |
| `--actor <NAME>` | actor recorded on writes and used by `--claim` (env `SEEDS_ACTOR`, else `$USER`) |
| `--store <PATH>` | use this local quipu store file ([configuration](config.md)) |
| `--quipu <URL>` | use this quipu server ([configuration](config.md); not built yet) |
| `--graph <IRI>` | the project's named graph ([configuration](config.md)) |
| `--at <TX>` | read as of this transaction (a [pin](pinning.md)); refused on writes |
| `-h`, `--help` / `-V`, `--version` | help and version |

## Verbs

| verb | flags (a br-compatible subset) |
|---|---|
| [`create [TITLE]`](verbs/create.md) | `--title`, `-t/--type`, `-p/--priority`, `-d/--description`, `--description-file`, `-a/--assignee`, `-l/--labels`, `--parent`, `--deps`, `--dry-run`, `--silent` |
| [`show <ID>...`](verbs/show.md) | |
| [`list`](verbs/list.md) | `-s/--status`, `-t/--type`, `--assignee`, `--unassigned`, `-l/--label`, `-p/--priority`, `-a/--all`, `--limit` (default 50, 0 = all), `--sort` |
| [`ready`](verbs/ready.md) | `--limit` (default: none), `--assignee [NAME]`, `--unassigned`, `-l/--label`, `-t/--type`, `-p/--priority`, `--parent` |
| [`count`](verbs/count.md) | `--by`, `--status`, `--type`, `--assignee`, `--include-closed` |
| [`update <ID>...`](verbs/update.md) | `--title`, `-d/--description`, `--notes`, `-s/--status`, `-p/--priority`, `--assignee`, `--claim`, `--add-label`, `--remove-label`, `--defer` |
| [`close <ID>...`](verbs/close.md) | `-r/--reason`, `-f/--force` |
| [`dep add <ISSUE> <DEPENDS_ON>`](verbs/dep.md) | `-t/--type` (default `blocks`) |
| [`dep remove <ISSUE> <DEPENDS_ON>`](verbs/dep.md) | `-t/--type` (default `blocks`) |
| [`dep list <ID>`](verbs/dep.md) | `--direction down\|up` |
| [`comments add <ID> [TEXT]...`](verbs/comments.md) | `-f/--file`, `-m/--message` (alias `--content`), `--author` |
| [`comments list <ID>`](verbs/comments.md) | |

## Exit codes

Exit codes are a contract. A code is never reused or renumbered.

| code | meaning |
|---|---|
| 0 | success |
| 1 | failed: store I/O or anything without a more specific code |
| 2 | usage: an unknown verb, a bad flag or value, `--at` on a write |
| 3 | not found: an unknown id, dependency or comment |
| 4 | conflict: a lost `--claim`, or the seed changed since it was read; nothing was written |
| 5 | refused: the shapes rejected the write, a dependency cycle, closing a seed with open blockers without `--force` |
| 6 | configuration: contradictory or unreadable config (for example `store` and `url` both set) |
| 7 | unreachable: the configured quipu server cannot be reached (seeds never falls back to a local store) |
| 10-18 | **retired**: the v0 shell's per-verb "not yet implemented" codes. Never reused. |
| 20 | not built: the configured capability (a quipu server URL) exists in the design but not in this build |

In `--json` mode an error prints `{"error": {"code": "<NAME>", "message":
"..."}}` on stdout, where `<NAME>` is `FAILED`, `USAGE`, `NOT_FOUND`,
`CONFLICT`, `REFUSED`, `CONFIG`, `UNREACHABLE` or `NOT_BUILT`. The message
always goes to stderr as well.

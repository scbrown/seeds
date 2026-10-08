# Reference

The command is `sd` (crate `seeds-ai`). Each verb has its own page under
[The verbs](verbs/index.md); this page is the summary.

## Global options

Accepted on every verb.

| option | meaning |
|---|---|
| `--json` | output as JSON, in br's output shape |
| `--actor <NAME>` | actor recorded on writes and used by `--claim` (env `SEEDS_ACTOR`, else `$USER`) |
| `--store <PATH>` | use this local quipu store file ([configuration](config.md)) |
| `--quipu <URL>` | use this quipu server ([mode 2](storage-modes.md#mode-2-a-quipu-server)) |
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
| [`export`](verbs/sync.md#sd-export) | `--to <DIR>` |
| [`import <DIR>`](verbs/sync.md#sd-import) | `--prefer pendant\|store`, `--replace` |
| [`sync`](verbs/sync.md#sd-sync) | `--remote <URL>`, `--allow-remote-deletes`, `--dry-run`, `--status` |
| [`merge-driver <BASE> <OURS> <THEIRS>`](verbs/sync.md#sd-merge-driver) | |

`create` and `update` also take `--workflow-run <RUN>` ([Formulas](formulas.md)).

## Exit codes

Exit codes are a contract. A code is never reused or renumbered.

| code | meaning |
|---|---|
| 0 | success |
| 1 | failed: store I/O or anything without a more specific code |
| 2 | usage: an unknown verb, a bad flag or value, `--at` on a write |
| 3 | not found: an unknown id, dependency or comment |
| 4 | conflict: a lost `--claim`, the seed changed since it was read, or two ledgers disagree (import, sync, merge-driver, a pendant that changed alongside the store); nothing was written |
| 5 | refused: the shapes rejected the write, a dependency cycle, closing a seed with open blockers without `--force`, a write over the request limit (`SEEDS_MAX_WRITE_BYTES`, `SEEDS_MAX_WRITE_CLAUSES` or the server's HTTP 413; nothing was sent) |
| 6 | configuration: contradictory or unreadable config (for example `store` and `url` both set) |
| 7 | unreachable: the configured quipu server cannot be reached (seeds never falls back to a local store) |
| 8 | indeterminate: a remote write's response was lost and a read-back does not show it; it may still land. Check the named ids before doing anything; do not simply retry |
| 10-18 | **retired**: the v0 shell's per-verb "not yet implemented" codes. Never reused. |
| 20 | not built: reserved for a configured capability that exists in the design but not in this build (no verb returns it today) |
| 21 | elsewhere: a br verb whose capability lives in another tool of the stack (`query`, `upgrade`, `gate`, `scheduler`, `audit`, `robot-docs`); the message says where |

In `--json` mode an error prints `{"error": {"code": "<NAME>", "message":
"..."}}` on stdout, where `<NAME>` is `FAILED`, `USAGE`, `NOT_FOUND`,
`CONFLICT`, `REFUSED`, `CONFIG`, `UNREACHABLE`, `INDETERMINATE`, `NOT_BUILT` or `ELSEWHERE`. The message
always goes to stderr as well.

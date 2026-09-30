# Reference

## Global options

These are accepted on every verb.

| option | meaning |
|---|---|
| `--json` | output as JSON, in the `bd` output shape |
| `--actor <NAME>` | actor for attribution (env `SEEDS_ACTOR`) |
| `--quipu <URL>` | base URL of the quipu server (env `SEEDS_QUIPU_URL`) |
| `--graph <IRI>` | named graph (project) to read and write (env `SEEDS_GRAPH`) |
| `--at <TX>` | resolve reads as of this transaction (a [pin](pinning.md)) |
| `-h`, `--help` / `-V`, `--version` | help and version |

## Verbs and exit codes

In v0 every verb parses its flags, prints
`seeds: <verb> not yet implemented (see docs/book)` to stderr, and exits with
its own code. Exit **2** is reserved for usage errors (an unknown verb or a bad
flag), so a caller can tell "you called me wrong" from "not built yet".

| verb | flags accepted (subset of `br`) | v0 exit |
|---|---|---|
| `create [TITLE]` | `--title`, `-t/--type`, `-p/--priority`, `-d/--description`, `--description-file`, `-a/--assignee`, `-l/--labels`, `--parent`, `--deps`, `--dry-run`, `--silent` | 10 |
| `show <ID>...` | | 11 |
| `list` | `-s/--status`, `-t/--type`, `--assignee`, `--unassigned`, `-l/--label`, `-p/--priority`, `-a/--all`, `--limit`, `--sort` | 12 |
| `ready` | `--limit`, `--assignee [NAME]`, `--unassigned`, `-l/--label`, `-t/--type`, `-p/--priority`, `--parent` | 13 |
| `count` | `--by`, `--status`, `--type`, `--assignee`, `--include-closed` | 14 |
| `update <ID>...` | `--title`, `-d/--description`, `--notes`, `-s/--status`, `-p/--priority`, `--assignee`, `--claim`, `--add-label`, `--remove-label`, `--defer` | 15 |
| `close <ID>...` | `-r/--reason`, `-f/--force` | 16 |
| `dep add <ISSUE> <DEPENDS_ON>` | `-t/--type` (default `blocks`) | 17 |
| `comments add <ID> [TEXT]...` | `-f/--file`, `-m/--message` (alias `--content`), `--author` | 18 |

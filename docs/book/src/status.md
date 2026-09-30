# What is built, and what is not

## Built (v0.1 core)

- **Verbs**: `create`, `show`, `list`, `ready`, `count`, `update` (including
  an atomic `--claim`), `close`, `dep add|remove|list`, `comments add|list`,
  each with `--json` in br's shape. See [the verbs](verbs/README.md).
- **Storage**: work items as facts in a quipu named graph, typed with
  camayoc's WorkItem vocabulary and validated against its shape on every
  write. See [The storage model](storage.md).
- **Pinning**: `--at <tx>` on every read resolves the project as of that
  transaction. See [Pinning](pinning.md).
- **Concurrency**: writers are serialised by a file lock and every write is a
  revision-checked compare-and-set; no lost updates, and a lost claim is a
  clean exit 4.
- **Configuration**: TOML, project then user then default, with a local store
  as the default. See [Configuration](config.md).
- **WebAssembly**: the core builds and its tests run on
  `wasm32-unknown-unknown`. See [WebAssembly](wasm.md).

## Not built yet

| what | why it matters | where |
|---|---|---|
| **beads sync**: two-way sync between a beads/br store and a seeds graph | reading existing work, and handing work back | a later phase |
| **shared quipu server backend** (`[quipu] url`) | one ledger for a team | needs an expected-value precondition on quipu's write path; configured URLs are refused today (exit 7 / 20) |
| **reading quipu's existing WorkItem records** | existing beads readable from day one | depends on the server backend |
| **`ready` / `blocked` / plate as camayoc stored queries** | one definition shared by every caller | the ready SPARQL is in `src/vocab.rs` (`ready_query`), ready to move |
| **`sd capabilities --json`** | intent 4 of [the intent map](intent-map.md) | not started |
| **`sd release <id> --if-assignee X`** | intent 1: a conditional release | `update --claim` is the claim half |
| **desire-path redirect** (`dp alias --cmd bd --replace sd`) | agents keep typing `bd` | lives in desire-path |
| **caboodle install and verify** | stack installation | lives in caboodle |
| **qpack sharing of a seeds ledger** | cross-team ledgers | quipu's machinery applies to the graph as is; nothing seeds-specific yet |

Every `bd` verb seeds does not know is a usage error (exit 2), which is what
desire-path records as demand.

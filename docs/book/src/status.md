# What is built, and what is not

## Built (v0.1 core)

- **Verbs**: `create`, `show`, `list`, `ready`, `count`, `update` (including
  an atomic `--claim`), `close`, `dep add|remove|list`, `comments add|list`,
  each with `--json` in br's shape. See [the verbs](verbs/index.md).
- **Storage**: work items as facts in a quipu named graph, typed as
  `schema:Action` (schema.org first, Quechua for governance) and validated
  against the seeds profile on every write. See [The storage model](storage.md).
- **Pinning**: `--at <tx>` on every read resolves the project as of that
  transaction. See [Pinning](pinning.md).
- **Concurrency**: writers are serialised by a file lock and every write is a
  revision-checked compare-and-set; no lost updates, and a lost claim is a
  clean exit 4.
- **Configuration**: TOML, project then user then default, with a local store
  as the default. See [Configuration](config.md).
- **Storage modes**: a repo-local pendant committed with the code (mode 1), a
  quipu server over HTTP (mode 2), and `sd sync` between them with a
  three-way, field-level merge (mode 3); `sd export` / `sd import` in any mode;
  a git merge driver for pendants. See [Storage modes](storage-modes.md).
- **A link to shuttle runs**: `--workflow-run` records the run that creates or
  drives a seed. See [Formulas](formulas.md).
- **WebAssembly**: the core builds and its tests run on
  `wasm32-unknown-unknown`. See [WebAssembly](wasm.md).

## Not built yet

| what | why it matters | where |
|---|---|---|
| **beads sync**: two-way sync between a beads/br store and a seeds graph | reading existing work, and handing work back | a later phase |
| **formulas** (molecules) | workflow templates that stamp and drive chains of seeds | shuttle integration, sketched in [Formulas](formulas.md); seeds will not grow its own engine |
| **a cheaper remote write** | mode 2 writes cost as much as the server's whole store (quipu's `/update`) | an expected-value precondition on quipu's `/set` or `/knot` |
| **pins across a pendant** | `--at` history in a clone | quipu's full reconstruction share |
| **reading quipu's existing WorkItem records** | existing beads readable from day one | camayoc's tracker projection keeps status on Observations; a read adapter is needed |
| **`ready` / `blocked` / plate as camayoc stored queries** | one definition shared by every caller | the ready SPARQL is in `src/vocab.rs` (`ready_query`), ready to move |
| **`sd capabilities --json`** | intent 4 of [the intent map](intent-map.md) | not started |
| **`sd release <id> --if-assignee X`** | intent 1: a conditional release | `update --claim` is the claim half |
| **desire-path redirect** (`dp alias --cmd bd --replace sd`) | agents keep typing `bd` | lives in desire-path |
| **caboodle install and verify** | stack installation | lives in caboodle |
| **signed or delta pendants** | verifiable ledger history between teams | quipu's attestation and delta shares; seeds writes full, unsigned shares today |

Every `bd` verb seeds does not know is a usage error (exit 2), which is what
desire-path records as demand.

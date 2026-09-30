# The bd intent map

Some agent tooling does not stop at the `bd` CLI. [Gas City](https://github.com/gastownhall/gascity),
for example, reaches past it into raw SQL and Dolt procedure calls when a verb
it needs does not exist. seeds does not copy that surface. Instead, each raw
call was read for its **intent**, and each intent is mapped to what seeds must
provide on a graph store.

Seven intents cover every raw call. **Three of them go away** when the store is
a quipu graph.

| # | intent | what callers do today | what seeds provides |
|---|---|---|---|
| 1 | Conditional claim / release | `UPDATE … WHERE status='in_progress' AND assignee=X`, plus a revision token | a compare-and-swap: `sd update <id> --claim`, `sd release <id> --if-assignee X` |
| 2 | Project identity | read or upsert a project-id metadata row; ask for the active branch; probe for the issues table | the project **is** a named-graph IRI, so identity comes built in |
| 3 | Commit / cursor | stage, commit, count dirty tables, read the head hash | **goes away**: every knot is a transaction and the transaction id is the cursor |
| 4 | Capability / schema probe | read the max migration version; list tables and columns | `sd capabilities --json`, derived from the loaded quipu shapes |
| 5 | Health / liveness | list databases, read the process list and data dir, start or stop the SQL server | **goes away**: quipu's own health endpoint; there is no server lifecycle |
| 6 | Maintenance | garbage-collect, purge dropped databases, compact | **goes away**: quipu's own job; a no-op in seeds |
| 7 | Read projections | `COUNT`, list and `ready` union queries | SPARQL behind `sd count`, `sd list` and `sd ready`, as camayoc stored queries |

## What each surviving intent needs

### 1. Conditional claim / release

Two agents must not both claim the same seed. quipu's SPARQL update endpoint
already runs `DELETE/INSERT … WHERE` under the store lock, so the `WHERE`
clause is an atomic precondition. What is missing for this to be a hot path:

- an **affected count**, so the caller knows whether the swap matched without a
  second read;
- **attribution**, so the write records who claimed it;
- a cheaper primitive than a whole-store update, most likely an `expected`
  field on a single-predicate write.

### 2. Project identity

A project is a named graph. Its IRI is stable, globally unique and travels with
the data, so "which project is this?" never disagrees with the data itself.

### 4. Capability probe

Callers ask "what does this store support?" to decide which code path to take.
seeds answers from the shapes quipu has loaded, which are the real contract,
rather than from a migration counter.

### 7. Read projections

`ready`, `blocked` and a worker's plate are **definitions**, not code. They
live as stored queries next to the shapes (in camayoc), so the definition of
"ready" is the same for every caller and travels with a shared ledger.

## The verbs

The CLI verbs cover what agents type directly: `create`, `show`, `list`,
`ready`, `count`, `update`, `close`, `dep add|remove|list` and
`comments add|list`, each with `--json` in the `bd` output shape. See
[Reference](reference.md).

## Where each intent stands

| # | intent | status |
|---|---|---|
| 1 | conditional claim / release | `update --claim` is built (a compare-and-set, exit 4 on a lost claim); `release --if-assignee` is not |
| 2 | project identity | built: one project is one named graph |
| 3 | commit / cursor | built: every write prints its transaction, and `--at` reads from it |
| 4 | capability probe | not built |
| 5 | health / liveness | nothing to build locally; the store is a file |
| 6 | maintenance | nothing to build |
| 7 | read projections | built: `count`, `list` and `ready` (ready is a SPARQL query) |

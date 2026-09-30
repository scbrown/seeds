# How it works

```text
  agent types `bd ready --json`
          │
          ▼
  desire-path  (dp alias --cmd bd --replace sd)        [not built yet]
          │ rewritten to `sd ready --json`
          ▼
  sd (native CLI)   config: flags > env > .seeds/config.toml > ~/.config/seeds > default
          │         clock, store file, write lock, pendant files, HTTP
          ▼
  seeds core        verbs, ready, JSON, pendant, merge   ◀── also builds for wasm32
          │  Backend trait
          ├──▶ QuipuBackend ── quipu (embedded) ── .seeds/seeds.db ⇄ .seeds/pendant/ (git)
          └──▶ RemoteBackend ── quipu server: /query, /update (compare-and-set)
                one project = one named graph; one write = one transaction
                shapes: camayoc WorkItem + seeds
```

## The pieces

- **seeds core** (this repo, `src/` outside `src/native/`). The work-item
  model, the verbs, the ready computation and the JSON output. It reads no
  clock, file or environment: time arrives in a `Ctx`, storage as a `Backend`.
  That is what lets it build for wasm32. See [WebAssembly](wasm.md).
- **`sd`, the native CLI** (`src/native/`). Its clap definitions follow `br`,
  so the flags match what agents already type. It resolves
  [configuration](config.md), opens the store file under a write lock, reads
  the system clock, and prints results in the `bd` JSON shape.
- **quipu** is the store, embedded as a library. Reads are SPARQL
  (`ready` runs a query) and fact scans; each write is one transaction of
  retractions and assertions. See [The storage model](storage.md), including
  why seeds embeds quipu rather than calling a quipu server.
- **camayoc** owns what a work item *means*: seeds reuses its `WorkItem`
  vocabulary and validates every write against its shape. The ready query is
  written to become a camayoc stored query.
- **A crew harness** can treat seeds as one more work-item tracker: the
  `--json` shapes are br's, so a tracker adapter written for br reads them.
- **caboodle** and **desire-path** integration (install and verify; the `bd`
  redirect) are not built yet. See [What is built](status.md).

## Where writes go

To the project's named graph in the configured store: by default a local file,
`.seeds/seeds.db`, created on first write. The graph IRI is the project's
identity.

## Why there is no commit verb

Every write is a transaction. The transaction id is the cursor a caller keeps,
and "what did this look like then" is a read with `--at`. There is no staging
area, no dirty state and nothing to commit. See [Pinning](pinning.md) for what
that buys.

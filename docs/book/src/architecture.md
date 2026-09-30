# How it works

```text
  agent types `bd ready --json`
          │
          ▼
  desire-path  (dp alias --cmd bd --replace seeds)
          │ rewritten to `seeds ready --json`
          ▼
  seeds CLI ──── reads ────▶ quipu  POST /query   (SPARQL)
          │                    ▲
          └───── writes ───────┘  POST /knot    (bearer-authenticated)
                               │
               shapes + stored queries from camayoc
```

## The pieces

- **seeds CLI** (this repo). A Rust binary whose clap definitions follow
  `br`, so the flags match what agents already type. It turns each verb into a
  SPARQL query or a knot and prints results in the `bd` JSON shape.
- **quipu** is the store. Reads go through its SPARQL `/query` endpoint.
  Writes go through `/knot`, which validates them against the loaded SHACL
  shapes and records each one as a transaction.
- **camayoc** owns what a seed *means*: the `WorkItem` shapes, and the
  competency queries (`ready`, `blocked`, a worker's plate) stored as named
  queries, so every caller shares one definition.
- **A crew harness** can treat seeds as one more work-item tracker. A
  `SeedsTracker` implements the same three-method tracker protocol (get,
  update, create) as the harness's other trackers.
- **caboodle** installs seeds and asserts it in `caboodle verify` with a
  functional round trip.
- **desire-path** redirects `bd` to seeds with a pre-tool-use rewrite
  (`dp alias --cmd bd --replace seeds`). Callers that need real beads pass
  through unchanged. Every `bd` verb seeds rejects is recorded, so
  `dp paths` becomes the seeds backlog.

## Reading existing work from day one

quipu already ingests bead lifecycle events as `WorkItem` records. seeds reads
those, so `list`, `show` and `ready` work over existing data before any write
path exists. Reads first, writes second.

## Where writes go

Writes land in a sandbox named graph, `seeds`, not on any live board. A seed's
IRI lives in that graph, and the graph IRI is the project's identity.

## Why there is no commit verb

Every knot is a transaction. The transaction id is the cursor a caller keeps,
and "what changed since my cursor" is a query. There is no staging area, no
dirty state and nothing to commit. See [Pinning](pinning.md) for what that
buys.

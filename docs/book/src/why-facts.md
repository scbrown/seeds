# Why facts, not snapshots

seeds is the beads-compatible tracker for the quipu stack: it answers the
`bd`/`br` verbs agents already type, and it stores every work item as facts in
a quipu graph. This chapter explains the storage choice, because it is what
the features below are built on.

[beads](https://github.com/gastownhall/beads) versions the whole issue
database: a commit captures every table at once, and history is a sequence of
database snapshots. That design has real strengths: branching, merging and a
familiar SQL surface.

seeds versions *facts*. Each field of each seed is a triple in a quipu graph,
written in a transaction. History is the sequence of transactions, and any
single fact can be read as of any transaction.

## What fact-level versioning gives you

1. **Fact-level pins.** Pinning a single seed to a transaction and resolving
   it later (see [Pinning](pinning.md)) answers a question that a
   whole-database snapshot answers badly.
2. **A smaller operational surface.** Of the seven things agent tooling asks a
   beads store to do (see [The bd intent map](intent-map.md)), three go away
   entirely when the store is a graph.
3. **Ledgers that share cleanly.** Two teams can exchange and reconcile work as
   qpacks (see [Sharing a ledger](qpack-sharing.md)).

## Design rules

- **Map the intent, not the SQL surface.** Tools that reach around `bd` into
  raw SQL are served by mapping the *intent* of each call, not its syntax.
- **`bd` callers move over without changes.** desire-path redirects `bd` to
  `sd`, and every verb seeds refuses is recorded, so the remaining parity work
  is driven by what agents actually type.

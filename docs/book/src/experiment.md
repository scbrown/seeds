# An experiment, not a competing beads

seeds exists to answer one question:

> **Is fact-level versioning a better fit for agent work tracking than
> snapshot versioning?**

[beads](https://github.com/gastownhall/beads) versions the whole issue
database: a commit captures every table at once, and history is a sequence of
database snapshots. That is a proven design with real strengths: branching,
merging and a familiar SQL surface.

seeds versions *facts*. Each field of each seed is a triple in a quipu graph,
written in a transaction. History is the sequence of transactions, and any
single fact can be read as of any transaction.

## What the experiment has to show

The experiment succeeds if it shows, on real agent traffic, one of:

1. **Fact-level pins are useful.** Pinning a single seed to a transaction and
   resolving it later (see [Pinning](pinning.md)) answers a question that a
   whole-database snapshot answers badly.
2. **Operational surface shrinks.** Of the seven things agent tooling asks a
   beads store to do (see [The bd intent map](intent-map.md)), three go away
   entirely when the store is a graph.
3. **Ledgers share cleanly.** Two teams can exchange and reconcile work as
   qpacks (see [Sharing a ledger](qpack-sharing.md)).

It also succeeds if it shows none of these, as long as it says why. A measured
"no" is a result.

## What it deliberately does not do

- **It does not replace beads for anyone.** The existing `bd` path keeps
  working, and callers that need the real thing pass through to it.
- **It does not copy the SQL surface.** Tools that reach around `bd` into raw
  SQL are served by mapping the *intent* of each call, not its syntax.
- **It does not write to anyone's board by default.** Writes go to a sandbox
  named graph until the experiment has earned more.
- **It does not aim for full feature parity.** Only the verbs agents actually
  type are in scope, and the backlog is driven by recorded demand.

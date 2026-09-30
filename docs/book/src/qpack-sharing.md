# Sharing a ledger as a qpack

A project is one named graph, and that graph **is** the ledger. quipu already
knows how to package a graph for another team as a **qpack**, so sharing work
between teams is the same operation as sharing any other knowledge.

## Full share

A qpack carries:

- `manifest.ttl`: what is in the pack and who produced it;
- `payload.nq`: the graph as RDF canonicalised with RDFC-1.0, whose sha256 is
  the ledger's identity;
- `shapes.ttl`: the shapes the data conforms to.

The standard share carries data and shapes only, **not** stored queries.
Queries travel in quipu's older SQLite pack (`quipu pack --queries`) and in
the full reconstruction share. So for a recipient to get the definition of
"ready" along with the data, seeds' ready query has to be shipped as a camayoc
stored query installed separately, or quipu's standard share has to learn to
carry queries.

## Incremental share

A delta is a SPARQL update bound to its parent's hash. A chain of deltas is a
hash chain of ledger updates: the equivalent of a push, with each step
verifiable against the one before.

## What leaves, and what gets in

- **Outbound**, a scrub step decides what leaves: private comments and
  internal names stay home. Signing names the producer.
- **Inbound**, a term from a vocabulary the recipient does not know is
  quarantined rather than silently accepted.

## Reconciling two ledgers

When two teams have both changed the same work, a shape-aware three-way merge
reconciles them:

- **functional** predicates (status, assignee) cannot hold two values, so a
  conflict escalates to a person;
- **multi-valued** predicates (labels, dependencies, comments) take the union.

That replaces a database-level sync or merge. One known limit: if the two
sides minted different IRIs for the same item, the merge cannot see it.

## Status

Nothing in this chapter is implemented in seeds yet. The qpack machinery lives
in quipu; seeds' job is to keep one project in one named graph so the
machinery applies without change.

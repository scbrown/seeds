# Introduction

**seeds** is an issue tracker for AI coding agents that stores each work item
("a seed") as facts in a [quipu](https://github.com/scbrown/quipu) knowledge
graph. It answers the verbs agents already type against `bd` and `br`
(`create`, `show`, `list`, `ready`, `count`, `update`, `close`, `dep add`,
`comments add`), with `--json` output in the same shape, so an agent does not
have to learn anything new.

> **Status: v0 is a shell.** Every verb parses its flags and exits with a
> distinct "not yet" code. This book describes the design the code will grow
> into. See [Reference](reference.md) for exactly what runs today.

## The name

A *quipucamayoc*, the keeper of the quipus, did arithmetic on a *yupana*, a
counting board of compartments, by moving seeds or pebbles across it. When the
count was settled it was knotted into the quipu, the permanent record.

The stack already uses that picture: quipu is the record, yupana is the board.
Seeds are the counters moved across the board before they become knots, which
is what a work item is: something in motion that ends up in the record.

## Why use it

- **Pin one item, not the whole database.** A snapshot-versioned tracker can
  tell you what the *whole database* looked like at a commit. seeds can tell
  you what *one item* looked like at a transaction, while the rest of the
  project keeps moving. See [Pinning](pinning.md).
- **No commit step, no server lifecycle.** Every write is a quipu transaction
  and the transaction id is the cursor. There is no dirty state to commit,
  no garbage collection to schedule and no SQL server to start and stop.
- **Identity is built in.** A project is a named graph, and its IRI is its
  identity. There is no separate project-id row to drift out of sync.
- **Governed writes.** quipu validates writes against SHACL shapes, so a seed
  with an unknown status or a malformed dependency is refused at the door.
- **Share a ledger like a package.** A project's graph, its shapes and the
  queries that define "ready" travel together as a qpack.
  See [Sharing a ledger](qpack-sharing.md).

## What it is not

It is not a replacement for beads, and it does not try to be. It is an
experiment built to answer one design question. Read
[An experiment, not a competing beads](experiment.md) before anything else.

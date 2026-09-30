# The storage model

A seed is a set of facts in a [quipu](https://github.com/scbrown/quipu)
graph. This page says which facts, where they live, how a write lands, and why
seeds embeds quipu instead of calling a quipu server.

## One project, one named graph

A project is one **named graph** in a quipu store. Its IRI is the project's
identity, so "which project is this?" is answered by the data itself, never
by a separate metadata row. The default IRI is
`https://seeds.local/project/<prefix>`; set another with `[project] graph`,
`SEEDS_GRAPH` or `--graph` (see [Configuration](config.md)).

A seed is an entity in that graph, at `https://seeds.local/item/<id>`.

## The vocabulary

seeds reuses [camayoc](https://github.com/scbrown/camayoc)'s WorkItem
vocabulary wherever camayoc names the thing, and mints a term in its own
`seeds:` namespace only for what a tracker needs and camayoc does not model.
camayoc publishes its vocabulary under the RDF namespace
`http://aegis.gastown.local/ontology/` (written `aegis:` below); that string
is a namespace name, not a host anyone contacts.

| field | predicate | object |
|---|---|---|
| type | `rdf:type` | `aegis:WorkItem` |
| provenance | `aegis:sourceKind` | `"declared"` (an agent or person said so) |
| id | `aegis:identifier` | `"sd-a3f"` |
| title | `rdfs:label` | string |
| created | `aegis:createdAt` | ISO-8601 UTC string |
| closed | `aegis:closedAt` | ISO-8601 UTC string, only when closed |
| outcome | `aegis:outcome` | `"done"`, only when closed (camayoc: absence means open) |
| assignee | `aegis:assignedTo` | IRI `https://seeds.local/principal/<name>` |
| `blocks` dependency | `aegis:blockedOn` | the blocker's item IRI |
| status | `seeds:status` | `open`, `in_progress`, `blocked`, `deferred`, `closed` |
| priority | `seeds:priority` | integer 0-4 |
| type | `seeds:issueType` | `task`, `bug`, `feature`, `epic`, `chore`, `docs`, `question` |
| description, notes | `seeds:description`, `seeds:notes` | string |
| labels | `seeds:label` | one fact per label |
| updated, creator | `seeds:updatedAt`, `seeds:createdBy` | string |
| close reason, defer | `seeds:closeReason`, `seeds:deferUntil` | string |
| `related`, `parent-child`, `discovered-from` | `seeds:relatedTo`, `seeds:childOf`, `seeds:discoveredFrom` | item IRI |
| compare-and-set token | `seeds:revision` | integer, 1 at create, +1 per write |
| driving workflow run | `seeds:workflowRun` | a shuttle run IRI, `urn:shuttle:run:<id>` ([Formulas](formulas.md)) |

A comment is its own entity, `https://seeds.local/item/<id>/comment/<n>`, typed
`seeds:Comment`, with `seeds:commentOn`, `seeds:commentIndex`, `seeds:author`,
`seeds:text` and `aegis:createdAt`. Comments are append-only.

**One deliberate difference from camayoc's tracker projection.** camayoc's
ingress keeps mutable tracker state (status) on a versioned `Observation`
rather than on the WorkItem, because its write path (`/episode`) appends
rather than replaces. seeds *is* the tracker and writes through quipu's
transaction API, which retracts the old value and asserts the new one in the
same transaction, so status sits on the seed itself and history is still
complete: every old value stays readable with `--at`.

## Shapes

Every write in the native CLI is validated against
[`shapes/seeds.shapes.ttl`](https://github.com/scbrown/seeds/blob/main/shapes/seeds.shapes.ttl)
before it is committed:

- **camayoc's `CamayocWorkItemShape`**, reproduced from camayoc with the same constraints: exactly
  one `sourceKind`, exactly one `rdfs:label`, an `outcome` from camayoc's
  closed list, and `blockedOn` pointing at a WorkItem the graph actually holds;
- **`SeedsWorkItemShape`**: exactly one identifier, status, priority (0-4),
  type and revision.

A write that does not conform is refused whole (exit 5) and nothing is
written. The wasm build has no SHACL engine (quipu leaves it out of its own
wasm gate), so there `QuipuBackend::validates()` is false and only the
structural rule that matters most, `blockedOn` pointing at a real seed, is
checked by hand.

## How a write lands

1. The verb reads a snapshot of the project.
2. It computes the complete post-state of every seed it touches, bumps each
   one's revision, and hands the backend a batch saying "based on revision N".
3. The backend re-reads each seed's current facts, refuses the whole batch
   with a **conflict** (exit 4) if any seed is no longer at the revision the
   writer read, validates the post-state against the shapes, and then writes
   the difference (retract what changed, assert its replacement) as **one**
   quipu transaction.

So a verb that touches several seeds (`close a b c`, `update a b`) lands all
of it or none of it, and the transaction id it prints is the cursor for
[`--at`](pinning.md).

### Concurrency

Two `sd` processes writing the same store are serialised by an exclusive lock
on `<store>.lock`, held from the snapshot read to the commit. The second writer
reads the first's result, so nothing is lost: eight concurrent
`update --add-label` processes leave eight labels and revision 9, and six
simultaneous `--claim`s leave exactly one winner and five conflicts
(`tests/cli.rs`).

The lock is load-bearing, and the tests show it: with the lock removed, both
of those tests fail. The backend's revision check is atomic only within one
store handle, because quipu's library API has no way to hold SQLite's write
lock across a read and a `transact`. A program that writes the same file
without taking the lock can therefore still race `sd`. Closing that gap needs
an expected-value precondition on quipu's own write path, which is the same
primitive the shared-server backend needs (below).

Reads take no lock. quipu keeps the store in SQLite WAL mode, so a reader sees
the last committed transaction while a write is in flight.

## Why the default is an embedded quipu

The original brief sketched reads through quipu's SPARQL `/query` and writes
through `/knot` on a quipu server. seeds instead embeds quipu as a library
(`quipu-ai`) with a local store file by default, as the other stack tools do,
and reaches a server only when configured to
([Storage modes](storage-modes.md)):

1. **A lost-update-free write needs a precondition.** `/knot` has none.
   `/update`'s `WHERE` clause is one, and mode 2 uses it, but it returns no
   affected count (seeds reads back to confirm), records no actor, and copies
   the whole store into memory on every call. Embedded, seeds checks the
   revision and commits in one process, under a lock it controls, for the
   cost of the seeds that changed.
2. **Out of the box means local.** `sd create` in an empty directory works with
   no server, no credentials and no configuration.
3. **The same core runs on wasm32.** quipu's library builds for
   `wasm32-unknown-unknown` with an in-memory store; see [WebAssembly](wasm.md).
4. **Testable.** Every verb, the concurrency guarantees and the JSON shapes
   are tested against a real quipu store with nothing to stand up; the
   remote mode is tested against a real `quipu-server`.

The store file is an ordinary quipu store, and a project's ledger travels as a
pendant, quipu's own share format.

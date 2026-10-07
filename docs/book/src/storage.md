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

**Ephemeral seeds** (`sd create --ephemeral`, br's flag) live in a sibling
graph, `<project IRI>#seeds-ephemeral`, created the first time one is written.
Every read (`show`, `list`, `search`, `count`, ...) sees both graphs as one
project, and the seed's `--json` carries `"ephemeral": true`. A pendant, an
export and `sd sync` carry only the project graph, so an ephemeral seed is
never shared, pulled or removed by a ledger, and it is never `ready`. A seed
stays in the graph it was created in; there is no verb to move it.

A shared seed cannot depend on an ephemeral one, by any dependency type:
the edge would be shared while its target is not, so the ledger would name a
seed no clone can resolve. br accepts such an edge, and its own
`sync --import-only` then refuses the export it wrote. seeds refuses the
edge instead (exit 2, nothing written). An ephemeral seed may depend on
anything.

## The vocabulary

A seed is a `schema:Action`. The terms follow one order of preference: W3C
first, then [schema.org](https://schema.org/), then another public
vocabulary, then [Quechua](https://scbrown.github.io/quechua/) for the
governance terms camayoc's gate reads, and seeds' own `seeds:` namespace only
for tracker mechanics no public vocabulary names. The mapping was ruled in
aegis-bqgdr3 (table v1.1). Item, comment, principal and graph IRIs are seeds'
own and did not change with it.

| prefix | namespace |
|---|---|
| `schema:` | `https://schema.org/` |
| `quechua:` | `https://scbrown.github.io/quechua/ns#` |
| `ical:` | `http://www.w3.org/2002/12/cal/ical#` |
| `dcterms:` | `http://purl.org/dc/terms/` |
| `prov:` | `http://www.w3.org/ns/prov#` |
| `seeds:` | `https://seeds.local/ontology/` |

These are vocabulary names, not hosts anyone contacts.

| field | predicate | object |
|---|---|---|
| type | `rdf:type` | `schema:Action` |
| provenance | `quechua:sourceKind` | `"declared"` (an agent or person said so) |
| id | `schema:identifier` | `"sd-a3f"` |
| title | `schema:name` **and** `rdfs:label` | the same string in both (quipu's label floor and `/search` read `rdfs:label`; the shapes hold them equal) |
| description | `schema:description` | string |
| status | `seeds:status` | `open`, `hooked`, `in_progress`, `blocked`, `deferred`, `closed`, `tombstone`: the field the claim compare-and-set reads |
| public status | `schema:actionStatus` | DERIVED in the same write: `open`/`hooked`/`blocked`/`deferred` are `schema:PotentialActionStatus`, `in_progress` is `ActiveActionStatus`, `closed` with outcome `done` is `CompletedActionStatus`, `closed` with `abandoned`/`superseded`/`failed` is `FailedActionStatus`, a tombstone has none |
| compare-and-set token | `schema:version` | integer, 1 at create, +1 per write |
| priority | `seeds:priority` | integer 0-4 |
| type | `seeds:issueType` | `task`, `bug`, `feature`, `epic`, `chore`, `docs`, `question` |
| assignee | `schema:agent` | principal IRI `https://seeds.local/principal/<name>` |
| creator, owner | `schema:creator`, `schema:accountablePerson` | principal IRI |
| labels | `schema:keywords` | one fact per label |
| created, updated, closed | `schema:dateCreated`, `schema:dateModified`, `schema:endTime` | `xsd:dateTime` |
| due | `ical:due` | `xsd:date` when a bare `YYYY-MM-DD`, else `xsd:dateTime` |
| defer | `schema:scheduledTime` | `xsd:date` or `xsd:dateTime`, as for due |
| estimate | `schema:timeRequired` | `xsd:duration` of the minutes, canonical (`PT30M`, `PT1H30M`, `P1D`) |
| close reason | `schema:result` | string |
| outcome | `quechua:outcome` | `"done"` (the default), `abandoned`, `superseded` or `failed`, only when closed |
| `blocks` dependency | `quechua:blockedOn` | the blocker's item IRI |
| `parent-child`, `related`, `discovered-from` | `schema:isPartOf`, `dcterms:relation`, `prov:wasDerivedFrom` | item IRI |
| notes, design, acceptance criteria, agent context, external ref | `seeds:notes`, `seeds:design`, `seeds:acceptanceCriteria`, `seeds:agentContext`, `seeds:externalRef` | string (an external ref need not be a URL) |
| driving workflow run | `seeds:workflowRun` | a shuttle run IRI, `urn:shuttle:run:<id>` ([Formulas](formulas.md)) |

**Times are typed, validated and written in canonical form.** A value that
is not a valid `xsd:dateTime` (or, for due and defer, a bare `YYYY-MM-DD`
`xsd:date`) is refused with a usage error (exit 2) naming the seed, the field
and the value, and nothing is written. A space instead of `T`, for example,
is refused, not repaired.

A valid value is written in its XSD canonical spelling, which keeps the value
and changes only the bytes: trailing zeros in fractional seconds go
(`…59.014436670Z` is `…59.01443667Z`, `.000Z` is `Z`), `+00:00` is `Z`,
`24:00:00` is the next day's `00:00:00`, and an estimate of 90 minutes is
`PT1H30M`. This is the form a quipu server's `/update` stores typed literals
in, and seeds computes it with the same code (oxigraph's `oxsdatatypes`), so
a local store, a server and a sync between them hold identical bytes. `sd
cutover` reports how many times it respelled (`canonicalized_times`), and its
export gives br back the original spelling.

**A ledger in the old vocabulary is refused, not read as empty.** sd 0.1
does not read what sd 0.0.x wrote (`aegis:WorkItem`, `seeds:revision`). Every
command first counts such items in the project graph (one bounded query), and
when there are any it refuses (exit 5) naming the store or server, the graph
and the count, with the migration: `sd cutover export` with the old sd, then
`sd cutover import` with this one into a new store. That recipe drops any
new-vocabulary seed from a mixed store, and only a pre-0.1.0 sd writing beside
0.1.0 could have made one. `sd doctor` reports it as a failed
`store.vocabulary` check, and a pendant in the old vocabulary fails validation
the same way. A count that cannot be read (a query error, no answer, a
malformed or missing number) refuses too: the check fails closed, never as a
clean ledger.

A comment is its own entity, `https://seeds.local/item/<id>/comment/<n>`, typed
`schema:Comment`, with `schema:parentItem` (the seed), `schema:position`
(1-based), `schema:author` (a principal IRI), `schema:text` and
`schema:dateCreated` (`xsd:dateTime`). Comments are append-only.

Who wrote what is kept in a side graph that exports and snapshots never read:
`seeds:Write` records (`seeds:actor`, `seeds:wrote`, `seeds:version`) and
their `seeds:AttributionClaim`s. That vocabulary did not change.

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
before it is committed. It is the whole profile from the aegis-bqgdr3 ruling,
not a subset: quipu's `/update` does not apply shapes, so on a shared board
this client-side check is the only gate.

- **`quechua:ActionGovernanceShape`**: exactly one `sourceKind`, an `outcome`
  from the closed list, and `blockedOn` pointing at an Action the graph
  actually holds (camayoc's constraints, carried onto `schema:Action`);
- **`seeds:SeedShape`**: exactly one identifier, `schema:name` (equal to the
  one `rdfs:label`), status, priority (0-4), type and version; typed dates;
  principal IRIs; and an `sh:xone` that holds status, outcome and
  `schema:actionStatus` in agreement;
- **`seeds:CommentShape`**: one parent seed, one position from 1, one author
  IRI, one text;
- **`seeds:CalendarAkaShape`**: a calendar to-do (`ical:Vtodo`) is a separate
  node linked to its seed by `skos:exactMatch`. sd writes none today.

A write that does not conform is refused whole (exit 5) and nothing is
written. The wasm build has no SHACL engine (quipu leaves it out of its own
wasm gate), so there `QuipuBackend::validates()` is false and only the
structural rules are checked by hand: `blockedOn` and comments pointing at a
real seed, single-valued fields holding one value, and the time forms.

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

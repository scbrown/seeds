# Cutover: lossless beads JSONL exchange

`sd cutover` rehearses a migration on a **copy** of a beads JSONL board. It
requires an explicit local `--store` and `--graph` and refuses a configured
pendant or remote destination. It never opens or updates a br database.
Keep br authoritative until the migration owner approves the cutover.

```sh
sd --store rehearsal.db --graph https://example.org/project/trial \
  cutover verify --file board-copy.jsonl
sd --store rehearsal.db --graph https://example.org/project/trial \
  cutover import --file board-copy.jsonl --dry-run
sd --store rehearsal.db --graph https://example.org/project/trial \
  cutover import --file board-copy.jsonl
sd --store rehearsal.db --graph https://example.org/project/trial \
  cutover export --file returned.jsonl
```

`verify` imports into an isolated in-memory Quipu store, reads the committed
facts back and compares JSON by exact record ID and field path. Its JSON
report includes input and output counts, a difference count and every changed
field, including nested comment and dependency metadata. Nonzero differences
exit 1. Object key order is immaterial; array order and absent versus null
remain significant. Malformed records and duplicate IDs are refused.

Each imported seed carries its original JSON in a `seeds:beadsJson` fact.
Normal seed fields are also projected to the seeds vocabulary
([The storage model](../storage.md)); a time that is not a valid
`xsd:date`/`xsd:dateTime` refuses the import, naming each record and field.
A valid time is stored in its canonical spelling (the same instant); the
output's `canonicalized_times` counts them, and export restores br's original
spelling.
Export overlays edits to those modeled fields on the original JSON. Unknown
fields, dependency types, timestamps, author names and nested metadata survive.
The legacy `relates-to` dependency projects as `related`, retaining its original
spelling on export. `hooked` is retained as a non-ready status. Unknown issue
types are retained by the bridge even when the create CLI does not offer them.
Unsupported dependency types are carried, not interpreted as blockers.

Seeds-authored items include a `_seeds` extension holding the revision and
unmodeled RDF facts (including literal datatypes and languages). Comments carry
their graph indexes there too. An occupied foreign `_seeds` key is refused when
it would be overwritten. A third-party importer must retain that extension to
preserve those facts; this command does not change third-party import behavior.
Historical br transactions absent from the source JSONL cannot be reconstructed.

New native dependencies retain their creation actor and time in carried
`seeds:dependencyOrigin` facts. Later item edits do not change edge attribution;
removing and re-adding an edge records its new creation. Imported dependency
metadata remains verbatim, including deliberate peer corrections. A legacy native
edge with neither carried JSON metadata nor creation provenance refuses export:
the item creator and its last update are not evidence of who created that edge.
Reconcile its provenance explicitly before using it in a cutover.

Newly synthesized label/dependency unions use br's canonical ordering. Unchanged
imported arrays retain their original order.

br comment IDs are global integers; a seeds comment index is local to its item.
The native bridge reserves global IDs in
`<store>.cutover-<graph-sha256>.comments.json`, bound to the store path and graph.
It retains imported IDs and maps each new `(item ID, comment index)` to an
unused positive integer. The high-water mark and deleted slots stay reserved.
The map is locked and saved before publishing an export, so another process or
a later export destination retains the same identities. Keep this sidecar with
the store when backing it up; do not delete it to reset a conflict. A malformed
map, identity collision or exhausted integer sequence refuses publication.
Concurrent independently allocated IDs may still conflict and require explicit
reconciliation; they are never silently reassigned.

## Ongoing exchange on a copied workstream

```sh
sd --actor migration-operator --store rehearsal.db \
  --graph https://example.org/project/trial \
  cutover sync --file board-copy.jsonl --base trial.cursor.json --dry-run
# Inspect the would-change report, then repeat without --dry-run.
```

The cursor holds the last common JSON state and is bound to the store path,
graph and JSONL path. Reusing it for another pair refuses. A missing cursor
means a first exchange, not permission to resolve conflicting existing items.
A corrupt or unreadable cursor is an error. Record removals refuse unless
explicitly enabled with `--allow-deletes`. Import is additive and refuses
changed existing IDs; use sync to reconcile updates against a common base.

Sync compares each field with that common state. Independent edits merge;
conflicting scalar edits report their record IDs and fields and write nothing.
Labels, dependencies and comments union on simultaneous changes. `updated_at`
keeps the later UTC timestamp when both sides changed it, including mixed
fractional precision. Non-UTC concurrent timestamps require reconciliation. Unmodeled scalar fields have the
same conflict protection as modeled ones. An unchanged second sync does not
commit or replace the JSONL or cursor.

Each store commit records the supplied `--actor` and `source=beads-sync`.
Original creator and comment attribution are carried unchanged. A JSONL export
does not identify the actor of a later edit; the caller must supply that actor,
not infer it from `created_by`.

Writes hold the normal sd store lock plus a cursor lock and a JSONL sidecar
lock (`<file>.cutover.lock`). Any process producing that JSONL must cooperate
with that lock. **Do not point this command at a live br export**: br's exporter
does not acquire the cutover lock. A pre-publication content check catches
changes observed during planning, but cannot fence an uncooperative writer.

A durable `<cursor>.pending` journal is written before committing the store.
It carries both pre-images and the intended common state. After a failure,
repeat the identical sync: it completes publication if both sides still match
an expected state. Newer edits cause a refusal that preserves the journal for
reconciliation. The cursor advances last. This makes interrupted publication
recoverable; it does not make two independent stores one atomic transaction.
Keep the cursor and pending journal private, like the board itself.

`--dry-run` creates no store, lock, journal, identity map or cursor. Export dry-run reports
its count and destination without writing a file.

This is an explicit one-shot operation. It installs no scheduler or fleet
cutover. A trial runner must state its polling interval, maintain a complete
rollback br copy, verify both directions and observe the required soak before
any production switch. A successful isolated round trip is one gate, not a
certificate that the fleet has migrated.

Sync reports aggregate field counts and the first 100 differences for each
side, with an explicit truncation flag. Use `verify` for a complete round-trip
difference report. Dry-run also validates the planned graph against the same
shapes as a real commit.

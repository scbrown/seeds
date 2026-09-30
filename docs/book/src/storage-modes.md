# Storage modes

Where the ledger lives is configuration ([Configuration](config.md)). There are
three arrangements, and they compose.

| mode | config | the ledger lives | reads need | writes go |
|---|---|---|---|---|
| **local** (default) | nothing, or `[quipu] store` | a quipu store file, `.seeds/seeds.db` | nothing | the file |
| **1. repo-local pendant** | `[pendant] dir = ".seeds/pendant"` | a **pendant** committed in the repository, plus a local working store | nothing: clone the repo and you have the board | the working store, then exported to the pendant in the same command |
| **2. remote** | `[quipu] url = "https://…"` | a quipu server | the server | the server, live, over HTTP |
| **3. sync** | a local store (with or without a pendant) and `[sync] remote = "https://…"` | both | nothing | locally; `sd sync` exchanges with the remote |

In every mode the core is the same: the same verbs, the same WorkItem facts,
the same [compare-and-set](storage.md#how-a-write-lands). The native CLI picks
the storage; the core never touches a file or the network, which keeps it
[wasm-clean](wasm.md).

## The pendant

A **pendant** is quipu's share artifact (formerly called a qpack), used as is.
seeds does not have a file format of its own:

| file | what it is |
|---|---|
| `export.nt` | **the ledger**: every current fact of the project graph as RDFC-1.0 canonical N-Triples, one fact per line, sorted |
| `shapes.ttl` | the shapes the ledger conforms to |
| `manifest.json`, `manifest.ttl` | the share manifest: hashes of the two files above, the producing store and transaction, the share id |
| `.gitattributes` | how git should merge these files (below) |

`export.nt` is deterministic: the same ledger always produces the same bytes,
so a change to one seed changes only that seed's lines, and a pull request that
closes a seed shows exactly the status, closed-at and revision lines that moved.
The manifest names the store that produced it; two clones exporting the same
ledger write identical `export.nt` files and manifests that differ in that
name. seeds treats `export.nt` as the data and the manifest as a seal it
re-checks.

The pendant is stamped `destination: internal`, quipu's own marker that no
outward identifier scrub ran on it (the scrub needs an identifier-policy
catalogue a seeds store does not carry). The marker is part of the share id, so
it cannot be edited out.

`sd export --to <dir>` writes a pendant anywhere, from any mode. `sd import
<dir>` reads one into the configured store.

## Mode 1: the ledger in the repository

```toml
# .seeds/config.toml, committed
[pendant]
dir = ".seeds/pendant"
```

```text
.seeds/
  config.toml          commit
  pendant/             commit: the ledger
    export.nt
    shapes.ttl
    manifest.json
    manifest.ttl
    .gitattributes
  .gitignore           written by sd: keeps the working store out of git
  seeds.db             the local working store (ignored)
  seeds.db.pendant     which export.nt the store last matched (ignored)
```

Every command first reconciles the working store with the pendant, using the
marker file as the common base:

| working store since the marker | pendant since the marker | what sd does |
|---|---|---|
| unchanged (or no store yet: a fresh clone) | changed: a pull, a checkout, a merge | loads the pendant into the store |
| changed | unchanged | exports the store to the pendant |
| changed | changed | **refuses** (exit 4) and says how to choose: `sd import <dir> --prefer pendant\|store`, `--replace`, or `sd export` |

A write then exports to the pendant before the command returns, under the same
lock. Reading a fresh clone does not touch the pendant files, so a clone's
working tree stays clean until something actually changes.

A loaded pendant is always **validated on its own terms**, not just checked
against its manifest: every seed must hold exactly one status, priority, title
and revision, every `blocks` edge must point at a seed in the ledger, and (in
the native build) the whole ledger must pass the WorkItem shapes. A pendant that
fails is refused with every problem listed, and the store is left as it was.

### Merging branches

Two branches that both changed the ledger merge in `export.nt`. seeds ships a
git merge driver that merges the two ledgers **field by field** against their
common ancestor, with the same rules as [`sd sync`](#mode-3-sync):

```bash
git config merge.seeds.driver "sd merge-driver %O %A %B"
```

The pendant's `.gitattributes` already names it (`export.nt merge=seeds`), and
merges the manifests with `merge=union` (sd re-seals them afterwards).

- One branch closes a seed while the other relabels a different one (or the
  same one): the driver merges both.
- Both branches set the same field to different values: the driver exits
  non-zero, git marks `export.nt` conflicted, and the file's first line is a
  conflict notice that does not parse, followed by each conflict. sd refuses
  to load it until a person resolves it. Nothing is decided by picking a side.

Without the driver registered, git falls back to its line merge. That often
works too (the file is sorted, one fact per line), and when it leaves conflict
markers or two values for one field, sd refuses the result with the line
number or the field, exactly as above.

## Mode 2: a quipu server

```toml
[quipu]
url = "https://quipu.example.org"
token_file = "~/.config/seeds/quipu-token"   # if the server wants a bearer for writes
```

Reads are SPARQL on the server's `/query`, paged so the server's row ceiling
can never silently shorten a snapshot (a page count that disagrees with a
`COUNT` is re-read, then refused). Writes are one SPARQL Update per command,
shaped as a compare-and-set:

```text
DELETE { GRAPH <project> { <seed> ?p ?o } }
INSERT { GRAPH <project> { …the new facts… } }
WHERE  { GRAPH <project> { <seed> seeds:revision 4 }       # the revision read
         FILTER NOT EXISTS { GRAPH <project> { <new-seed> ?x ?y } }
         { GRAPH <project> { <seed> ?p ?o } } UNION { } }
```

quipu runs an update under its store lock, so the `WHERE` clause is an atomic
precondition: if the seed moved, nothing matches and nothing is written. The
server's `/update` reports no affected count, so seeds reads the written seeds
back and compares; a mismatch is a conflict (exit 4), never a success. Four
simultaneous `--claim`s against one server leave exactly one winner
(`tests/modes.rs`).

Two things found while building this, which anyone running mode 2 should know:

- **quipu's `/update` evaluates `WHERE` only over registered named graphs.**
  Against an unregistered graph a precondition matches nothing, so a
  `FILTER NOT EXISTS` guard passes vacuously and a second create writes a
  duplicate (reproduced against quipu-server 0.9.1). seeds registers the
  project graph (`POST /graph/create`) before its first write.
- **`/update` copies the whole store into memory on every call**, so its cost
  grows with the server's store. A seeds project on a dedicated quipu server is
  fine; a large shared knowledge graph makes every write as slow as that graph
  is big. An expected-value precondition on quipu's cheaper write paths would
  remove this.

A configured server that cannot be reached is exit 7. seeds never falls back to
a local store, because two stores would then hold two ledgers.

## Mode 3: sync

```toml
[pendant]
dir = ".seeds/pendant"     # optional
[sync]
remote = "https://quipu.example.org"
```

`sd sync` merges the local store and the remote against the ledger as of the
last sync (kept in `<store>.sync-base.nt`), writes the merge to the remote, then
locally, then exports the pendant:

- a side that did not change a seed since the base yields to the side that did;
- a seed both sides changed is merged field by field; labels and dependencies
  take every addition and removal from both sides;
- a field both sides changed to different values is a **conflict**: every
  conflict is listed, and nothing is written on either side (exit 4);
- a seed one side deleted and the other changed is a conflict;
- comments are append-only: both sides' new comments are kept, and a local one
  that took the same number as a remote one is renumbered after it.

The remote write is a compare-and-set on the revisions sync read, so a remote
change that lands mid-sync fails the sync cleanly (nothing local is written)
and `sd sync` can simply run again. This is the `br sync` analogue: `export`
is the flush, `import` is the load, and `sync` is both with a three-way merge
instead of last-writer-wins.

## What each mode does not do yet

- **Pins across a pendant.** `export.nt` carries current facts only, so
  `--at <tx>` history stays in the store that recorded it. quipu's full
  reconstruction share could carry history; seeds does not use it yet.
- **Automatic sync.** `sd sync` runs when asked; nothing syncs in the
  background.
- **Delta pendants.** Every export is a full share. quipu's delta shares
  (`delta.ru`, a hash chain of updates) would make a pendant's history
  verifiable step by step.

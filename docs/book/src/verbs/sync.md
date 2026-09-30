# sd export, sd import, sd sync, sd merge-driver

These move a ledger between stores. The modes they serve are described in
[Storage modes](../storage-modes.md).

## sd export

```bash
sd export                    # to the configured [pendant] dir
sd export --to /tmp/ledger   # anywhere
```

Writes the ledger as a pendant (`export.nt`, `shapes.ttl`, `manifest.json`,
`manifest.ttl`). Works from a local store or a remote one; the `export.nt` is
the same bytes either way. A pendant that already holds exactly this ledger
under an intact seal is left untouched.

**`--json`**: `{status, dir, changed, seeds, export_hash, files}`.

## sd import

```bash
sd import /tmp/ledger                   # add what is new; refuse disagreements
sd import /tmp/ledger --prefer pendant  # disagreements: take the pendant's
sd import /tmp/ledger --prefer store    # disagreements: keep the store's
sd import /tmp/ledger --replace         # make the store exactly the pendant
```

Reads a pendant into the configured store (local or remote), after validating
the ledger on its own terms. A seed only in the pendant is created; an
identical one is left alone; one that differs is a **conflict** unless
`--prefer` names a side. Conflicts are listed and nothing is written (exit 4).
With `--prefer pendant`, the pendant's version lands at a revision above both
sides, so no reader holding the store's old revision can write over it.

**`--json`**: `{status, created, updated, removed, unchanged, comments_added, tx, wrote}`.

## sd sync

```bash
sd sync                              # with [sync] remote
sd sync --remote https://quipu.example.org
```

Three-way merge of the local store and the remote against the ledger as of the
last sync with that remote, written to both sides; conflicts are listed and
nothing is written. Removals (a seed the base had that one side lacks) are
refused with exit 5 unless `--allow-remote-deletes`.
See [Mode 3](../storage-modes.md#mode-3-sync).

**`--json`**: `{status, local: {…}, remote: {…}}`, each side shaped like
import's; `wrote` says whether that side was written (a quipu server reports no
transaction id, so `tx` is 0 there).

## sd merge-driver

```bash
git config merge.seeds.driver "sd merge-driver %O %A %B"
```

A git merge driver for a pendant's `export.nt`: the same field-level merge as
`sd sync`, with the sides named `ours` and `theirs`. Needs no store and no
configuration. See [Merging branches](../storage-modes.md#merging-branches).

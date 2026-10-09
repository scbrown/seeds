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
sd sync --push-only                  # only items changed locally since the last sync
sd sync --dry-run                    # what a sync would do; writes nothing
sd sync --status                     # in-sync, local-ahead, remote-ahead, diverged or conflicted
```

Three-way merge of the local store and the remote against the ledger as of the
last sync with that remote, written to both sides; conflicts are listed and
nothing is written. Removals (a seed the base had that one side lacks) are
refused with exit 5 unless `--allow-remote-deletes`.
See [Mode 3](../storage-modes.md#mode-3-sync).

**`--push-only`** reads only the remote seeds and comments for items changed
locally since the last successful sync with this remote. It keeps the same
field-level merge, conflict, revision and removal guards for those items.
Remote-only changes on untouched items are not pulled or acknowledged; use a
normal two-way sync to discover them. The first push considers every local
item, never deletes unrelated remote items, and a replay with no local changes
reads no remote items. Both `--dry-run` and `--status` can preview this scoped
plan; `in-sync` with `--push-only` means no local changes need pushing, not
that the entire remote agrees. The base advances only after all writes succeed.

**`--json`**: `{status, local: {…}, remote: {…}}`, each side shaped like
import's; `wrote` says whether that side was written (a quipu server reports no
transaction id, so `tx` is 0 there).

**`--dry-run`** plans from the same merge and stops: no write to either side
and no new sync base. It exits as the sync would, so a conflict (exit 4) or a
refused removal (exit 5) fails the dry run too. Its `--json` adds
`dry_run: true`, and `wrote` is false on both sides.

**`--status`** reports where the two stand and never fails on a disagreement:
`{status, sync: {state, remote_url, synced_before, conflicts, removals_need_allow,
would_refuse, local, remote}}`. The state is `local-ahead` when only the remote
would be written, `remote-ahead` when only the local store would, `diverged`
when both would, `in-sync` when neither would, and `conflicted` when a sync
would refuse on conflicts.

Neither preview writes anything beyond what every sd command does in
[mode 1](../storage-modes.md): reconcile the store with its pendant first.

**A push larger than the server accepts is split.** A quipu server caps a
request body (64 MiB for `/update`), and the first sync of a whole board is far
over that. A batch is also limited by how many guard clauses it nests:
two per new seed, one per comment, one per updated or removed item. quipu-server
aborts on an `/update` nesting about two thousand of them, whatever the body
size. When the remote write would exceed `SEEDS_MAX_WRITE_BYTES` (default
48 MiB) or `SEEDS_MAX_WRITE_CLAUSES` (default 500), sync pushes it as several batches, each its own transaction, and
prints `sd: remote batch N/M landed` to stderr as each one commits. The order
keeps every batch valid by itself: a seed lands after the seeds it is blocked
on, seeds in a dependency cycle land together, a comment lands with its seed,
and removals land last. A write that fits is still one transaction.

If a split push stops part way, the error says how many batches landed and
keeps the failure's own exit code (an unknown outcome is still exit 8, read
it back first). The local store and the sync base are untouched, so running
the same `sd sync` again continues: what already landed is identical on both
sides and is not written again.

Any write whose request would exceed the limit is refused before it is sent,
with exit 5 and a message starting `write too large for the server`; so is a
write nesting more guard clauses than the limit, and so is a server's own HTTP
413. Nothing was written in either case. (Sending it anyway
lets the server close the connection mid-request, which reads as a lost
response, exit 8, for a write that was never evaluated.) A single seed or
dependency cycle over the limit cannot be split and is refused the same way.

br's other `sync` flags (`--apply`, `--force`, `--orphans` and the JSONL
export/import modes) act on br's SQLite/JSONL machinery and have no seeds
counterpart. In particular `--force` does not lift the removal guard; only
`--allow-remote-deletes` does.

## sd merge-driver

```bash
git config merge.seeds.driver "sd merge-driver %O %A %B"
```

A git merge driver for a pendant's `export.nt`: the same field-level merge as
`sd sync`, with the sides named `ours` and `theirs`. Needs no store and no
configuration. See [Merging branches](../storage-modes.md#merging-branches).

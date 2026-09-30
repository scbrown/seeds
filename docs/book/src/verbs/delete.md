# sd delete

Delete seeds. A deleted seed becomes a **tombstone**: hidden from every
listing and count, but kept, so sync carries the delete like any other change
and `--at` still reads what was there.

```bash
sd delete sd-a3f --reason "duplicate of sd-b7c"
sd delete sd-a3f --dry-run          # what would go
sd delete sd-a3f --cascade          # and everything that depends on it
sd delete sd-a3f --force            # leave dependents pointing at a tombstone
```

- A seed that other live seeds depend on is **not deleted** without
  `--cascade` or `--force`: sd prints br's preview, writes nothing, and says
  so on stderr.
- `--cascade` tombstones every live seed that (transitively) depends on the
  target. `--force` deletes only the target; its dependents are reported as
  orphaned, and since **a tombstone never blocks**, they are not stuck.
- A tombstone **cannot gain a dependency** in either direction, and
  `sd update --status tombstone` is refused: `delete` is the only way in.
- A delete **is not a close**: no outcome, no `closed_at`. The reason is kept
  as the comment `Deleted: <reason>`, written in the same transaction.
- `show` still returns a tombstone; `list`, `ready`, `search`, `count`,
  `stale`, `blocked`, `epic` and `stats` leave it out (`stats` reports how
  many as `tombstone_issues`). `--status tombstone` lists them explicitly.
- `sd sync` carries a delete in both directions **without**
  `--allow-remote-deletes`: to sync, a tombstone is an update, not a removal.

**`--json`**: br's `{deleted, deleted_count, dependencies_removed,
labels_removed, events_removed, references_updated, orphaned_issues}` plus
`tx`; a preview prints br's `{preview: true, would_delete, cascade_delete,
blocked_dependents, orphaned_issues}`.

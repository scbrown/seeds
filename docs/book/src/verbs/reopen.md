# sd reopen, defer, undefer

Status transitions br has as their own verbs.

```bash
sd reopen sd-a3f -r "the fix regressed in 9c1e2f0"
sd defer sd-a3f sd-b7c --until +1w
sd defer sd-a3f --until 2026-11-01
sd undefer sd-a3f
```

- **`reopen`**: closed seeds become `open`, and `closed_at` and
  `close_reason` are cleared. `-r` is stored as the comment
  `Reopened: <reason>` **in the same transaction** as the status change.
  A seed that is not closed is skipped.
- **`defer`**: status `deferred`, so the seed leaves `ready`. `--until`
  takes `+30m`, `+2h`, `+1d`, `+1w`, `tomorrow`, `YYYY-MM-DD` or an RFC 3339
  instant (relative forms resolve to UTC). Without `--until` the seed is
  deferred with no date. Closed seeds are skipped.
- **`undefer`**: deferred seeds become `open` with no date. Others are skipped.
- Every named seed changes in one transaction; an unknown id is exit 3 and
  nothing is written. Skipped seeds are reported with br's reason, never
  silently.

**`--json`**: `{"reopened" | "deferred" | "undeferred": [{id, title,
previous_status, status, defer_until?}], "skipped": [{id, reason}], "tx"}`.
`skipped` appears only when something was skipped, as in br.

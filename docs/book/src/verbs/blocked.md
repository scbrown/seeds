# sd blocked

List seeds that are waiting on something.

```bash
sd blocked
sd blocked -t bug -t task -p 0 -p 1 --limit 0 --json
```

| flag | meaning |
|---|---|
| `-t, --type` | only these types (repeatable: any of them) |
| `-p, --priority` | only these priorities (repeatable: any of them) |
| `-l, --label` | only seeds with this label (repeatable: all must match) |
| `--limit` | page size (default 50, as br; 0 = all) |

- A seed is blocked when it is **not closed and has at least one open
  `blocks` dependency**. A `blocked` status on its own does not qualify: that
  is br's rule.
- Closing the last open blocker takes a seed off the list.
- A cut-short page says so, in text and as `has_more` in `--json`.

**`--json`**: br's envelope `{issues, total, limit, offset, has_more}`; each
seed also carries `blocked_by` (the open blocker ids) and `blocked_by_count`.

# sd stats

Summary counts for the whole ledger, with optional breakdowns.

```bash
sd stats
sd stats --by-type --by-priority --by-assignee --by-label --json
```

- **Summary**: total, and counts by status (`blocked` counts the *status*,
  as br does, not the dependency graph), what `ready` would list now, open
  epics whose children are all closed, and the mean hours from creation to
  close over closed seeds.
- **Breakdowns** count every seed, closed included, as br does: by type,
  priority (`P0`..`P4`), assignee (`(unassigned)`) and label (`(no labels)`).
- seeds has no drafts, tombstones or pins, so those counts are always 0.
  br's activity flags are not implemented: br's `--json` does not report
  activity either, and accepting a flag that does nothing would mislead.

**`--json`**: br's `{summary: {total_issues, open_issues, ...,
average_lead_time_hours}, breakdowns: [{dimension, counts: [{key, count}]}]}`;
`breakdowns` appears only when one is asked for.

# sd count

Count seeds (closed ones only with `--include-closed` or `--status closed`).

```bash
sd count
sd count --by status --include-closed --json
```

| flag | meaning |
|---|---|
| `--by` | group by `status`, `priority`, `type`, `assignee` or `label` |
| `--status`, `--type`, `--assignee` | filters |
| `--include-closed` | count closed seeds too |

**`--json`**: `{"count": N}`; with `--by`, `{"total": N, "groups": [{"group",
"count"}]}`. With `--by label` a seed counts once per label, so the groups
can sum to more than the total.

`--by-status`, `--by-priority`, `--by-type`, `--by-assignee` and `--by-label`
are br's shorthands for `--by <x>` (one grouping at a time). `--priority`,
`--title-contains` and `--unassigned` filter what is counted.

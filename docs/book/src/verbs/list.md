# sd list

List seeds. Closed seeds are hidden unless you ask for them (`--all`, or
`--status closed`).

```bash
sd list
sd list --status in_progress --assignee ian
sd list -l parser -l core --limit 0 --json
```

| flag | meaning |
|---|---|
| `-s, --status` | only this status |
| `-t, --type` | only this type |
| `--assignee NAME` / `--unassigned` | only this assignee / only unassigned |
| `-l, --label` | only seeds carrying this label (repeatable; all must match) |
| `-p, --priority` | only this priority |
| `-a, --all` | include closed seeds |
| `--limit N` | page size, **default 50**; `0` lists everything |
| `--sort` | `priority` (default), `created`, `updated`, `id`, `title` |

A list cut short by `--limit` says so: text mode ends with
`(showing N of M; --limit 0 shows all)`, stderr carries the same warning, and
`--json` carries `has_more: true`.

**`--json`**: br's envelope, `{issues, total, limit, offset, has_more}`. Each
issue is a seed object with these keys:

`id`, `title`, `description`, `notes`, `status`, `priority`, `issue_type`,
`assignee`, `labels`, `created_at`, `created_by`, `updated_at`, `closed_at`,
`close_reason`, `defer_until`, `parent`, `dependency_count`, `workflow_run`,
`revision`.

In `list`, `search` and `blocked` envelopes each issue also carries br's
`dependent_count`: how many seeds declare any dependency on it (blocks,
parent-child, related or discovered-from).

`revision` is the compare-and-set token (1 at create, +1 per write).

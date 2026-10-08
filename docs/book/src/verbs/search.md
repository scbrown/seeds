# sd search

Find seeds by title, with an explicit option for full-field search.

```bash
sd search lexer
sd search "chokes on" -t bug --all --json
sd search "comment phrase" --full --all
```

- By default, matches **titles only**, case-insensitively. Every result names
  this scope; an empty result says **0 title matches**. Descriptions and comments
  may still contain the query.
- `--full` also searches **ids, descriptions and comments** (notes are not
  searched). This administrative operation can read much more data. One full
  search at a time is permitted per user on a host, across all projects; a
  concurrent full search is refused rather than queued.
- Closed matches are **hidden and counted** unless `--all` or `--status`: the
  text says how many, and `--json` carries `hidden_closed_count`.
- Takes the [list](list.md) filters (`-s`, `-t`, `--assignee`,
  `--unassigned`, `-l`, `-p`), `--sort`, and `--limit` (default 50, as br;
  0 = all). A cut-short page says so.

**`--json`**: br's envelope `{issues, hidden_closed_count, limit, offset,
has_more}`, plus `total`, `search_scope` and `search_notice`. CSV results keep
their tabular stdout and report the scope on stderr.

**Filters** (also on [search](search.md)): `--title-contains`,
`--desc-contains`, `--notes-contains` (case-insensitive), `--label-any`
(repeatable, any of), `--priority-min`/`--priority-max`, `--id` (repeatable),
`--overdue` (due before now and not closed, as br; deferred seeds stay hidden
unless `--deferred`).
**Paging**: `--offset N` skips N results (the envelope's `offset` and
`has_more` account for it); `-r, --reverse` flips the sort.

> **Deferred seeds are hidden by default**, as in br: pass `--deferred` (or
> `--all`, or `--status deferred`) to see them. Earlier versions of sd listed
> them by default.

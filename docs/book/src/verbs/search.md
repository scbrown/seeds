# sd search

Find seeds by text.

```bash
sd search lexer
sd search "chokes on" -t bug --all --json
```

- Matches the query, case-insensitively, against the seed's **id, title,
  description and comments** (br's fields; notes are not searched, as in br).
- Closed matches are **hidden and counted** unless `--all` or `--status`: the
  text says how many, and `--json` carries `hidden_closed_count`.
- Takes the [list](list.md) filters (`-s`, `-t`, `--assignee`,
  `--unassigned`, `-l`, `-p`), `--sort`, and `--limit` (default 50, as br;
  0 = all). A cut-short page says so.

**`--json`**: br's envelope `{issues, hidden_closed_count, limit, offset,
has_more}`, plus `total`.

**Filters** (also on [search](search.md)): `--title-contains`,
`--desc-contains`, `--notes-contains` (case-insensitive), `--label-any`
(repeatable, any of), `--priority-min`/`--priority-max`, `--id` (repeatable).
**Paging**: `--offset N` skips N results (the envelope's `offset` and
`has_more` account for it); `-r, --reverse` flips the sort.

> **Deferred seeds are hidden by default**, as in br: pass `--deferred` (or
> `--all`, or `--status deferred`) to see them. Earlier versions of sd listed
> them by default.

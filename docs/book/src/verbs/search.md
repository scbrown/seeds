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

# sd changelog

What got done: closed seeds, grouped by type.

```bash
sd changelog                       # everything ever closed
sd changelog --since +7d           # the last week
sd changelog --since 2026-09-01
sd changelog --since-tag v0.0.2    # since a release tag's commit date
sd changelog --since-commit HEAD~20 --json
```

- Groups, in order: Epics, Features, Bugs, Tasks, Chores, Docs, Questions;
  within each, most recently closed first. Deleted seeds are not listed.
- `--since` takes a date, an RFC 3339 instant, or a span back from now
  (`+7d`, `+2w`, `+12h`: br's relative form). `--since-tag` and
  `--since-commit` ask git for that commit's date, converted to UTC.

**`--json`**: br's `{since, until, total_closed, groups: [{issue_type, label,
issues: [{id, title, priority, closed_at}]}]}`.

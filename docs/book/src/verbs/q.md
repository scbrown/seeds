# sd q, status, and the verbs that live elsewhere

```bash
sd q fix the flaky parser test -p 1 -l ci   # prints only the new id
sd status                                   # the same as sd stats
```

- `q` is quick capture: the title words are joined, and only the id is
  printed (in `--json`, `{id, title, tx}`). It takes `-p`, `-t`, `-l`
  (repeatable, comma-separated allowed), `-d`/`--body`, `--parent` and
  `-e`/`--estimate` (minutes).
- `status` is an alias of [stats](stats.md), as in br.

## br verbs that are not sd's

Some br verbs belong to another tool of the stack. `sd` accepts them, does
nothing, prints where the capability lives on stderr, and exits **21**
(`ELSEWHERE`), so a redirect from `bd`/`br` records the attempt instead of
failing silently or pretending:

| verb | where it lives |
|---|---|
| `query` | quipu stored queries |
| `upgrade` | caboodle (`caboodle update-release --tool seeds`) |
| `gate`, `scheduler` | shuttle |
| `audit` | quipu provenance: every sd write is a transaction with its actor; read past states with `sd show --at <tx>` |
| `robot-docs` | this book and `sd <verb> --help` |

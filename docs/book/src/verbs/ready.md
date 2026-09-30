# sd ready

List seeds that are ready to work on: status `open`, no `blocks` dependency on
a seed that is not closed, and no defer date in the future. Sorted by priority,
then age.

```bash
sd ready --json
sd ready --assignee            # mine (the current actor)
sd ready --unassigned -l parser
```

**`ready` is not truncated.** Without `--limit` it prints every ready seed.
With `--limit N` it prints N and warns on stderr:
`ready: TRUNCATED to N of M ready seeds by --limit`. A silently short ready
list reads as "nothing else is ready", which is the wrong answer, so seeds
never produces one.

| flag | meaning |
|---|---|
| `--limit N` | cut the list at N, and say so |
| `--assignee [NAME]` | only this assignee; bare `--assignee` means the current actor |
| `--unassigned` | only unassigned |
| `-l, --label` | only with this label (repeatable) |
| `-t, --type`, `-p, --priority` | only this type / priority |
| `--parent ID` | only children of this seed |

**The definition is a query.** The core runs the SPARQL in
`seeds::vocab::ready_query` against the project graph, then applies the
filters and the defer date. The same definition is also computed directly
over the model, and `tests/core.rs` checks the two agree after every step of a
scripted sequence of dependency changes, closes and reopens.

Only `blocks` dependencies gate readiness; `related`, `parent-child` and
`discovered-from` do not.

**`--json`**: a bare array of seed objects, as br prints it.

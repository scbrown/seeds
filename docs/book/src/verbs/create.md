# sd create

Create a seed.

```bash
sd create "Write the parser" -p 1 -t feature -l parser,core
sd create --title "Child task" --parent sd-a3f       # minted as sd-a3f.1
sd create "Blocked work" --deps sd-a3f,related:sd-b7c
```

| flag | meaning |
|---|---|
| `TITLE` / `--title` | the title (one of the two, not both) |
| `-t, --type` | `task` (default), `bug`, `feature`, `epic`, `chore`, `docs`, `question` |
| `-p, --priority` | `0`-`4` or `P0`-`P4` (default 2) |
| `-d, --description` / `--description-file PATH` | the description |
| `-a, --assignee` | assignee |
| `-l, --labels` | comma-separated labels |
| `--parent ID` | parent seed; the new id is `<parent>.<n>` |
| `--deps` | comma-separated: `ID` (a `blocks` dependency) or `TYPE:ID` |
| `--dry-run` | print what would be created, write nothing |
| `--silent` | print only the new id |

**Ids** are `<prefix>-<hash>`: the prefix from [configuration](../config.md)
(`sd` by default), then three or more base36 characters from a hash of the
title, the creation instant and an attempt counter. A collision tries the next
attempt, and the hash grows by one character every four collisions, so ids stay
short and unique. No randomness is involved.

**`--json`**: the new seed object (the keys under [list](list.md)).

**Exit codes**: 0; 2 for a bad flag value; 3 when `--parent` or a `--deps`
target does not exist; 5 when the shapes refuse the write.

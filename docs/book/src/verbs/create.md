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
| `--owner` | who owns the work (br's `owner`, usually an email) |
| `--acceptance-criteria TEXT` (alias `--acceptance`) | acceptance criteria, often a `- [ ]` checklist |
| `--external-ref REF` | a reference to the same work elsewhere (br's `external_ref`) |
| `--agent-context JSON` | governing instructions for an agent (br's `agent_context`): inline JSON, `@path` (JSON) or `@path.yaml`/`.yml` (normalized to JSON). Stored as compact JSON with key order kept; `""` leaves it unset. YAML keys become JSON strings, so `1` and `"1"` are one key (the last wins) |
| `--slug SLUG` | embed a slug in the id, `<prefix>-<slug>-<hash>`: lowercase ASCII letters, digits and single hyphens, at most 48 characters (br's `--slug`). Ignored with `--parent`; refused with `--step` |
| `--due WHEN` | due date: `+1d`, `tomorrow`, `YYYY-MM-DD` or an RFC 3339 instant (br's `due_at`). A bare date and `tomorrow` mean 09:00 local time, stored as a UTC instant, as br does |
| `-e, --estimate MIN` | time estimate in minutes, 0 to 525960 (br's `estimated_minutes`) |
| `-l, --labels` | comma-separated labels |
| `--parent ID` | parent seed; the new id is `<parent>.<n>` |
| `--deps` | comma-separated: `ID` (a `blocks` dependency) or `TYPE:ID` |
| `--workflow-run RUN` | the shuttle run that creates or drives it: a run IRI, or a bare id (becomes `urn:shuttle:run:<id>`); see [Formulas](../formulas.md) |
| `--step STEP` | the workflow step creating the seed (needs `--workflow-run`). The id is derived from run, step and visit (`<prefix>-w<hash>`), so repeating the create returns the same seed (exit 0, `exists ...`) instead of a duplicate, even when two writers race |
| `--visit N` | which entry into `--step` this is, from 1 (default 1); a step the run enters again gets a new seed |
| `--dry-run` | print what would be created, write nothing |
| `--silent` | print only the new id |

**Ids** are `<prefix>-<hash>`: the prefix from [configuration](../config.md)
(`sd` by default), then three or more base36 characters from a hash of the
title, the creation instant and an attempt counter. A collision tries the next
attempt, and the hash grows by one character every four collisions, so ids stay
short and unique. No randomness is involved.

**`-f, --file FILE`** creates every item of a markdown file in **one
transaction** (br's bulk import). Each `## Title` starts a seed; under it,
`### Priority`, `### Type`, `### Labels`, `### Assignee`, `### Dependencies`,
`### Description`, `### Notes`, `### Design` and `### Acceptance Criteria` set
its fields, and the text between the
title and its first `###` is the description. Anything before the first `##`
is ignored, and headings inside fenced code blocks are text. Two differences
from br, both so that nothing is dropped silently: sd keeps the whole body
(br keeps only its first paragraph), and a section sd does not know, or an
item with both a body and a `### Description`, refuses the file (exit 2) and
writes nothing.
`--file` takes no per-seed flags; `--dry-run`, `--silent` and `--json` apply.
`--json` is an array of the new seeds.

**`--json`**: the new seed object (the keys under [list](list.md)) plus `tx`
(`null` for `--dry-run`).

**Exit codes**: 0; 2 for a bad flag value; 3 when `--parent` or a `--deps`
target does not exist; 5 when the shapes refuse the write.

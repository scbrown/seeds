# sd update

Change one or more seeds. All the named seeds change in one transaction, or
none do.

```bash
sd update sd-a3f --status in_progress --add-label parser
sd update sd-a3f --claim                 # atomically take it
sd update sd-a3f sd-b7c --priority 0
sd update sd-a3f --defer 2026-10-15      # hide from ready until then
```

| flag | meaning |
|---|---|
| `--title`, `-d, --description`, `--notes`, `--design`, `--acceptance-criteria` (alias `--acceptance`) | set the text (see below for replacing existing text) |
| `--force` | allow replacing a non-empty description, notes, design or acceptance criteria with different content |
| `-s, --status` | `open`, `in_progress`, `blocked`, `deferred`, `closed` |
| `-p, --priority` | `0`-`4` or `P0`-`P4` |
| `--assignee NAME` | set the assignee (`""` clears it) |
| `--owner WHO` | set the owner (`""` clears it) |
| `--external-ref REF` | a reference to the same work elsewhere, br's `external_ref` (`""` clears it) |
| `--claim` | set assignee to the actor and status to `in_progress`, only if the seed is open, unclaimed and unblocked |
| `--add-label`, `--remove-label` | repeatable |
| `--defer DATE` | hide from `ready` until this date or instant (`""` clears it) |
| `--workflow-run RUN` | the shuttle run driving it (`""` clears it); see [Formulas](../formulas.md) |

**`--claim` is a compare-and-set.** It succeeds for exactly one caller: a
second claimer, or a claim on a blocked or non-open seed, exits **4** and
writes nothing. Claiming your own claim again is a no-op. The actor is
`--actor`, else `SEEDS_ACTOR`, else `$USER`.

Setting status to `closed` records `closed_at`; setting it back to anything
else clears `closed_at` and `close_reason`. An update that changes nothing
writes nothing.

**Existing text is not replaced by accident.** Setting a description, notes,
design or acceptance criteria
that already has text to *different* text (including `""`) is refused with
exit **5**, naming the field, and nothing is written, unless `--force` is
given. This is br's guard (br's `--force`). Filling an empty field, or setting
the same text again, needs no `--force`, so re-running an update is safe. The
replaced text stays in the seed's history (`sd history`). The check runs in the
shared update path, so a local store and a quipu server refuse alike.

**`--json`**: an array of the updated seed objects, each with the `tx`.

**Exit codes**: 0; 2 for a bad value or `--claim` combined with `--assignee`
or `--status`; 3 for an unknown id; 4 for a lost claim or a concurrent change;
5 for replacing existing text without `--force`.

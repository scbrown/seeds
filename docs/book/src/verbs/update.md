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
| `--title`, `-d, --description`, `--notes` | replace the text |
| `-s, --status` | `open`, `in_progress`, `blocked`, `deferred`, `closed` |
| `-p, --priority` | `0`-`4` or `P0`-`P4` |
| `--assignee NAME` | set the assignee (`""` clears it) |
| `--claim` | set assignee to the actor and status to `in_progress`, only if the seed is open, unclaimed and unblocked |
| `--add-label`, `--remove-label` | repeatable |
| `--defer DATE` | hide from `ready` until this date or instant (`""` clears it) |

**`--claim` is a compare-and-set.** It succeeds for exactly one caller: a
second claimer, or a claim on a blocked or non-open seed, exits **4** and
writes nothing. Claiming your own claim again is a no-op. The actor is
`--actor`, else `SEEDS_ACTOR`, else `$USER`.

Setting status to `closed` records `closed_at`; setting it back to anything
else clears `closed_at` and `close_reason`. An update that changes nothing
writes nothing.

**`--json`**: an array of the updated seed objects.

**Exit codes**: 0; 2 for a bad value or `--claim` combined with `--assignee`
or `--status`; 3 for an unknown id; 4 for a lost claim or a concurrent change.

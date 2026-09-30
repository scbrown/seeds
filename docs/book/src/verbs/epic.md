# sd epic

Epic progress, and closing the epics whose work is done.

```bash
sd epic status                    # every epic that is not closed
sd epic status --eligible-only
sd epic close-eligible --dry-run  # what would close
sd epic close-eligible
```

- An epic's **children** are the seeds created with `--parent <epic>`.
- An epic is **eligible to close** when it has at least one child and every
  child is closed. An epic with no children is never eligible (br).
- `close-eligible` closes every eligible epic in one transaction, with br's
  reason `All children completed`. An eligible epic that still has an open
  blocker of its own is left open and reported as skipped; it is never
  force-closed.

**`--json`**: `status` and `close-eligible --dry-run` print br's array of
`{epic, total_children, closed_children, eligible_for_close}`;
`close-eligible` prints `{closed: [ids], count, tx}`, plus `skipped` when an
epic was left open.

# sd close

Close one or more seeds, in one transaction.

```bash
sd close sd-a3f --reason "parser merged in 4d3ac73; 18 tests green"
sd close sd-a3f sd-b7c -r "superseded by sd-c9d"
```

| flag | meaning |
|---|---|
| `-r, --reason` | why: what landed and how you know |
| `--outcome` | how it ended: `done` (default), `abandoned`, `superseded` or `failed`. the governed `quechua:outcome`, stored as a field and shown as `outcome` in `--json`, so a workflow branches on it instead of parsing the reason. Reopening clears it |
| `-f, --force` | close even though a `blocks` dependency is still open |

- **Closing without `--reason` warns** on stderr and still closes. The reason
  is what later readers search, and it cannot be improved after the fact
  without another write.
- A seed with an open blocker is **refused** (exit 5) unless `--force`.
- A seed that is already closed is left unchanged, with a warning.
- Closing records `closed_at`, the reason, and camayoc's `outcome: done`.

**`--json`**: an array of the closed seed objects, each with the `tx`.

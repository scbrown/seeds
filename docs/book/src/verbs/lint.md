# sd lint

Seeds whose description lacks the sections their type's template expects.

```bash
sd lint                      # open seeds
sd lint sd-a3f sd-b7c
sd lint -t bug -s all --json
```

| type | expected sections |
|---|---|
| bug | `## Steps to Reproduce`, `## Acceptance Criteria` |
| task, feature | `## Acceptance Criteria` |
| epic | `## Success Criteria` |
| chore, docs, question | none |

- These are br's templates and hints. A heading at any level counts, and
  the match ignores case.
- Warnings, not errors: `lint` exits 0 either way.
- Without ids it checks open seeds; `-s all` checks every status. Deleted
  seeds are skipped.

**`--json`**: br's `{total (warnings), issues (seeds with warnings), results:
[{id, title, type, missing, warnings, suggestions: [{section, hint}]}]}`.

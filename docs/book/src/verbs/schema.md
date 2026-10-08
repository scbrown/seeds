# sd schema

JSON Schemas (draft 2020-12) for `sd`'s `--json` output.

```bash
sd schema                  # every schema, plus each verb's top-level shape
sd schema issue-details
sd schema commands         # verb -> {shape, item, jq}
```

Targets: `issue`, `issue-with-counts`, `issue-details`, `ready-issue`,
`stale-issue`, `blocked-issue`, `comment`, `statistics`, `error`, `commands`,
and `all` (the default).

- The schemas are **generated from the same tables the contract tests pin**
  (`src/schema.rs`), and a test validates real `show`, `list`, `blocked`,
  `ready`, `stats`, `comments add` and error output against them, so the
  published schema cannot drift from what `sd` prints.
- Objects are closed-world: every listed key is required (absent values are
  `null`), and no other key appears. Writes add a `tx` on top of the object
  (see [the verbs](index.md)).
- Needs no configuration and reads no ledger.

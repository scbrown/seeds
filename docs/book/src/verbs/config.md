# sd config

The configuration `sd` actually resolved, from the layers described in
[Configuration](../config.md).

```bash
sd config list            # every resolved setting
sd config get project.prefix
sd config path            # which project and user files, and whether they exist
```

- Keys are `section.key`, matching the TOML files: `quipu.store`,
  `quipu.url`, `quipu.location_source`, `quipu.token`, `quipu.token_file`,
  `quipu.trusted_hosts`, `quipu.allow_plain_http_hosts`, `project.prefix`,
  `project.graph`, `project.id_file`, `pendant.dir`, `sync.remote`.
- **A token is never printed.** `quipu.token` says only whether one is set,
  and from where: `(set: SEEDS_QUIPU_TOKEN)`, `(set: token_file)` or `(unset)`.
- `set`, `delete` (`unset`) and `edit` are **not built** (exit 20) and name
  the file to edit. Changing `project.prefix` or `project.graph` moves the
  project to a different ledger (see [init](init.md)), so it is a deliberate
  edit, not a one-liner.

**`--json`**: `list` prints `{"<key>": value}`; `get` prints `{key, value}`;
`path` prints `{project: {path, exists}, user: {path, exists}}`.

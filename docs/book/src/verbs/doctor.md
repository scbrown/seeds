# sd doctor

Read-only checks of the configuration and the ledger.

```bash
sd doctor
sd doctor --json && echo healthy
```

| check | what it asserts |
|---|---|
| `config.resolves` | the configuration resolved (and from which layer) |
| `project.id` | `.seeds/project-id` holds an id (a warning before the first write) |
| `.gitignore` | the local store's directory ignores the store file |
| `store.opens` | the store or server answers; how many seeds and comments, at which tx |
| `ledger.valid` | the same validation `sd import` applies: shapes, single-valued fields, dangling `blocks` edges |
| `deps.targets_exist` | every dependency of every kind points at a seed |
| `deps.no_cycles` | no cycle in `blocks` (a cycle leaves every seed on it permanently unready) |

- **Exit 1 when any check is an error**, 0 otherwise (warnings do not fail),
  so a script or CI can gate on it. Every check is reported either way.
- Read-only: `doctor` never repairs. The verbs refuse to create most of these
  states; `doctor` finds the ones a raw write or a bad merge can.

**`--json`**: `{ok, checks: [{name, status: ok|warn|error, message}]}`.

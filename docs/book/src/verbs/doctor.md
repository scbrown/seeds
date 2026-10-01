# sd doctor

Read-only checks of the configuration and the ledger.

```bash
sd doctor
sd doctor --json && echo healthy
sd doctor --quick            # the cheap checks only (skips ledger.valid)
sd doctor --robot-triage     # one JSON envelope for an agent
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

**`--json`**: `{ok, checks: [{name, status: ok|warn|error|skipped, message}]}`.

**`--quick`** skips `ledger.valid`, the one check that exports and validates
the whole ledger (measured on 400 seeds: about 0.18 s with it, 0.01 s
without). It is reported as `skipped`, never as passed; every other check runs
unchanged.

**`--robot-triage`** prints `{schema_version: "sd.doctor.triage.v1", summary,
findings, actions_planned, recommended_command, quick_ref}`. `findings` lists
every check that is not ok, each with a `recommended_command` where sd has one
(for example `sd init --force` for a missing `.gitignore`, `sd dep remove
<issue> <depends-on>` for a dangling edge). `actions_planned` is always empty,
because doctor never repairs. Exit codes are those of `sd doctor`.

br's `--fix` (its `--repair`) and `--dry-run` (a preview of that repair) have
no counterpart: nothing in a quipu ledger is derived from anything else.

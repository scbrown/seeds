# sd capabilities

`sd`'s machine-readable contract, for agents and tools.

```bash
sd capabilities --json
sd capabilities --for "comments add" --json
```

- **commands**: every leaf verb with its summary, aliases, flags and
  `operation`: `read` (pinnable with `--at`), `write`, or `elsewhere` (a br
  verb that lives in another tool; exit 21).
- **global_flags**, **exit_codes** (with meanings), **env_vars**, and the
  **safety** guarantees.
- Derived from `sd`'s own definitions, not written out: commands and flags
  from the CLI definition, `operation` from the same flag that decides
  whether a verb takes the store's write lock, exit codes from the error
  kinds. It cannot drift from what the binary does.
- Needs no configuration and reads no ledger. For output shapes, see
  [schema](schema.md).

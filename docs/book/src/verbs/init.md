# sd init

Make the current directory a seeds project.

```bash
sd init --prefix ab
sd init --force        # restore missing files; never changes the id or prefix
```

- Writes `.seeds/config.toml` (with `[project] prefix`), `.seeds/project-id`
  and `.seeds/.gitignore` in the **current** directory. A parent project is
  not consulted. The store itself is created by the first write.
- The ledger a project writes to is named by its **prefix and project id
  together**. Changing either moves the project to a different, empty ledger
  and strands every seed already written. So:
  - `init` in a project that already has an id is refused (exit 4) and
    writes nothing.
  - `--force` only restores missing files. It keeps the existing id, and
    asking it to change a configured prefix is refused (exit 5).
- Commit `config.toml` and `project-id`.
- The default prefix is `$SEEDS_PREFIX`, else `sd`.

**`--json`**: `{path, prefix, project_id, graph, created, kept}`.

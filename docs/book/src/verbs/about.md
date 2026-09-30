# sd version, where, info

What `sd` is, and where its ledger lives.

```bash
sd version            # sd 0.0.x
sd version --short    # just the number, for scripts
sd where              # store file or server, graph, prefix, pendant
sd info --json        # where, plus how many seeds and comments, at which tx
```

- `version` needs no configuration. seeds does not embed its commit, branch
  or compiler, so those `--json` keys are `null` rather than guessed.
- `where` reads the **configuration only**. It never creates a store or a
  project id, so it is safe to run anywhere, including before the first
  write.
- `info` opens the ledger read-only and reports its size: seeds, comments,
  the current transaction and, for a local store, the file size.

**`--json`**: `version` prints br's `{version, build, commit, branch,
rust_version, target, features}`; `where` prints br's `{path, prefix,
database_path, jsonl_path}` plus `quipu_url`, `mode`, `graph`, `pendant_path`
and `location_source`; `info` prints br's `{database_path, beads_dir, mode,
issue_count, config: {issue_prefix}, db_size, jsonl_path}` plus
`comment_count`, `tx`, `graph` and `quipu_url`. `jsonl_path` is `null`: seeds
has no JSONL file.

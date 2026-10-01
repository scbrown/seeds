# sd version, where, info

What `sd` is, and where its ledger lives.

```bash
sd version            # sd 0.0.x
sd version --short    # just the number, for scripts
sd version --check    # is a newer release published? exit 0 / 1 / 7
sd where              # store file or server, graph, prefix, pendant
sd info --json        # where, plus how many seeds and comments, at which tx
sd info --schema      # plus the shapes' sha256 and target classes
sd info --whats-new   # this build's latest changelog section (no store)
sd info --thanks      # the projects sd builds on (no store)
```

- `version --check` asks for the latest published release (GitHub's latest
  release of scbrown/seeds; `SEEDS_RELEASES_URL` overrides it) and exits
  **0** when this build is current, **1** when a newer release exists, as br
  does, and **7** when it cannot tell: offline, an error status, or a tag that
  is not a plain `x.y.z`. It never reports "up to date" for an answer it could
  not compare. `--json`: `{current_version, latest_version, update_available,
  source}`.
- `version` needs no configuration. seeds does not embed its commit, branch
  or compiler, so those `--json` keys are `null` rather than guessed.
- `where` reads the **configuration only**. It never creates a store or a
  project id, so it is safe to run anywhere, including before the first
  write.
- `info` opens the ledger read-only and reports its size: seeds, comments,
  the current transaction and, for a local store, the file size.
- `info --schema` adds `schema: {shapes_sha256, target_classes, vocabularies,
  json_schemas}`. The digest identifies the shapes this build validates a
  ledger against (the same bytes as a pendant's `shapes.ttl`); `sd schema`
  prints the `--json` schemas.
- `info --whats-new` and `info --thanks` describe the build, not a ledger, so
  they open no store and work anywhere. `--whats-new` prints the latest
  release section of the changelog compiled into this binary
  (`{version, release, changes}`).

**`--json`**: `version` prints br's `{version, build, commit, branch,
rust_version, target, features}`; `where` prints br's `{path, prefix,
database_path, jsonl_path}` plus `quipu_url`, `mode`, `graph`, `pendant_path`
and `location_source`; `info` prints br's `{database_path, beads_dir, mode,
issue_count, config: {issue_prefix}, db_size, jsonl_path}` plus
`comment_count`, `tx`, `graph` and `quipu_url`. `jsonl_path` is `null`: seeds
has no JSONL file.

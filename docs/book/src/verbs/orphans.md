# sd orphans

Open or in-progress seeds that a git commit mentions: work that may already
be done and never closed.

```bash
sd orphans
sd orphans --details --json
sd orphans --fix             # ask, seed by seed, whether to close it
```

- Reads `git log` of the current repository, newest first, and reports each
  open or in-progress seed whose id appears in a commit's subject or body,
  with the newest such commit.
- Ids match as whole tokens, so a commit about `sd-a3f.1` does not make
  `sd-a3f` an orphan.
- Outside a git repository it is a usage error (exit 2).
- `--fix` is br's interactive close: for each orphan it asks on stderr
  `Close <id> (<title>)? [y/N]` and reads a line from stdin. Only `y` or `yes`
  closes it, with br's reason `Implemented (detected by orphans scan)`;
  anything else, and no input at all, skips it. So `sd orphans --fix
  </dev/null` changes nothing. `--at` cannot be combined with it (a write).

**`--json`**: br's bare array of `{issue_id, title, status, latest_commit,
latest_commit_message}`; `--details` adds `commit_hash` and `commit_body`; `--fix` adds `fix: closed|skipped`
(`status` is the seed's status before the fix).

# sd orphans

Open or in-progress seeds that a git commit mentions: work that may already
be done and never closed.

```bash
sd orphans
sd orphans --details --json
```

- Reads `git log` of the current repository, newest first, and reports each
  open or in-progress seed whose id appears in a commit's subject or body,
  with the newest such commit.
- Ids match as whole tokens, so a commit about `sd-a3f.1` does not make
  `sd-a3f` an orphan.
- Outside a git repository it is a usage error (exit 2).
- br's `--fix` (an interactive prompt) is not implemented: close the seed
  with `sd close` and a reason that names the commit.

**`--json`**: br's bare array of `{issue_id, title, status, latest_commit,
latest_commit_message}`; `--details` adds `commit_hash` and `commit_body`.

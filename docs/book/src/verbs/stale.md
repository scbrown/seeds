# sd stale

Find seeds nobody has touched in a while.

```bash
sd stale                          # not updated for 30 days
sd stale --days 7 --status open,in_progress --json
```

- Lists seeds whose last update is at least `--days` days old (default 30),
  **oldest first**. `--days 0` lists everything not closed.
- Closed seeds are left out unless `--status` names `closed`. `--status` is
  repeatable or comma-separated.

**`--json`**: a bare array of seed objects, as br prints it.

# sd history

Every version of one seed, with the transaction that wrote it.

```bash
sd history sd-a3f
sd history sd-a3f --json
sd show sd-a3f --at 12       # read any one version in full
```

> **Not br's `history`.** br's verb manages local backup files. In seeds
> every write is a quipu transaction, so a seed's history is already in the
> store; `sd history` lists it, and says so in its output.

- One entry per version, oldest first: the transaction, the revision, when,
  the status, and **what changed** from the previous version.
- Each entry is exactly what `sd show <id> --at <tx>` reads.
- Works against a local store and a quipu server alike. A quipu server
  reports no head transaction and its writes report none either
  (aegis-xajsgn), so history finds each version by searching `--at` reads,
  a handful of reads per version.
- `--at` does not apply: history is every version.

**`--json`**: `{meaning, id, versions: [<seed object> + tx + changes]}`.

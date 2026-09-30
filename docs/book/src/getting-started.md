# Getting started

## Build

There is no release yet, and the repository is private for now. Build from
source with Rust 1.85 or newer:

```bash
cargo install --git https://github.com/scbrown/seeds --locked
seeds --version
```

```text
seeds 0.0.1
```

## What v0 does

Every verb parses a `bd`/`br`-compatible subset of flags, prints one line to
stderr and exits with its own non-zero code:

```bash
seeds ready --json
echo "exit $?"
```

```text
seeds: ready not yet implemented (see docs/book)
exit 13
```

That is deliberate. With `bd` redirected to seeds through
[desire-path](https://github.com/scbrown/desire-path), each refused call is
recorded, and the list of recorded calls tells us which verb to build next.

## Troubleshooting

- **`error: unrecognized subcommand`, exit 2.** That verb is not in the v0
  surface at all. Exit 2 is always a usage error; "not yet" codes start at 10.
- **An older version prints.** Another copy is earlier on your `PATH`:
  `which -a seeds`.

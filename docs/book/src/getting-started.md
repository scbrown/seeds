# Getting started

## Install

The command is `sd`. The project is **seeds** and the crate is `seeds-ai`.

- **Prebuilt binaries.** Each [GitHub release](https://github.com/scbrown/seeds/releases)
  carries `sd-<version>-<target>.tar.gz` for `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin` and
  `x86_64-apple-darwin`, plus a `SHA256SUMS.txt` covering every archive.
  Verify before you extract. v0 is a shell; the first release has not been
  cut yet.
- **crates.io**, once the first version is published:
  `cargo install seeds-ai --locked` (Rust 1.85 or newer).
- **From source:** `cargo install --git https://github.com/scbrown/seeds --locked`.

```bash
sd --version
```

```text
sd 0.0.1
```

### If you already have chmln/sd

[chmln/sd](https://github.com/chmln/sd) is a widely used find-and-replace
tool whose command is also `sd` (crates.io `sd`, Homebrew `sd`).

- `cargo install seeds-ai` refuses to overwrite an existing `~/.cargo/bin/sd`;
  `--force` replaces it.
- With both installed in different directories, `PATH` order decides which
  runs. `which -a sd` shows the order.
- To keep both, install the seeds binary under another name (for example
  `seeds`) and point the desire-path alias at that name.

## What v0 does

Every verb parses a `bd`/`br`-compatible subset of flags, prints one line to
stderr and exits with its own non-zero code:

```bash
sd ready --json
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
  `which -a sd`. If `sd --version` does not print `sd <version>`, it is
  a different `sd`.

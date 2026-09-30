# Getting started

## Install

The command is `sd`. The project is **seeds** and the crate is `seeds-ai`.

- **Prebuilt binaries.** Each [GitHub release](https://github.com/scbrown/seeds/releases)
  carries `sd-<version>-<target>.tar.gz` for `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin` and
  `x86_64-apple-darwin`, plus a `SHA256SUMS.txt` covering every archive.
  Verify before you extract. The first release has not been cut yet.
- **crates.io**, once the first version is published:
  `cargo install seeds-ai --locked` (Rust 1.89 or newer).
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

## A first session

No server and no configuration are needed: the first write creates a local
store at `.seeds/seeds.db`.

```bash
sd create "Write the parser" -p 1          # created ○ sd-k2x [P1] [task] Write the parser (tx 1)
sd create "Design the grammar"             # created ○ sd-7mf ... (tx 2)
sd dep add sd-k2x sd-7mf                   # sd-k2x is blocked by sd-7mf
sd ready                                   # only sd-7mf
sd update sd-7mf --claim                   # take it (exit 4 if someone else has)
sd close sd-7mf --reason "grammar in docs/grammar.md"
sd ready                                   # now sd-k2x
sd show sd-k2x --at 2                      # sd-k2x as it stood at transaction 2
```

Add `--json` to any of them for br-shaped output. To keep the store somewhere
else, or to change the id prefix, see [Configuration](config.md).

## Troubleshooting

- **`error: unrecognized subcommand`, exit 2.** That verb is not in seeds.
  Exit 2 is always a usage error. Every code is in
  [Reference](reference.md#exit-codes).
- **`no seeds store at … yet`** on a read. Nothing has been written in this
  project (or the configuration points somewhere new); the answer is empty
  because the store is, not because nothing matched.
- **Exit 7 with a URL in the message.** A quipu server is configured
  (`[quipu] url` or `SEEDS_QUIPU_URL`) and cannot be reached. seeds does not
  fall back to a local store. See [Storage modes](storage-modes.md).
- **An older version prints.** Another copy is earlier on your `PATH`:
  `which -a sd`. If `sd --version` does not print `sd <version>`, it is
  a different `sd`.

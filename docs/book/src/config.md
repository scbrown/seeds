# Configuration

With no configuration at all, `sd` uses a **local** quipu store at
`.seeds/seeds.db` and creates it on the first write. Nothing needs a server.

## Where the graph lives

Two keys, and a store uses exactly one of them:

```toml
# .seeds/config.toml (or ~/.config/seeds/config.toml)
[quipu]
store = ".seeds/seeds.db"            # a local quipu store file (the default)
# url = "https://quipu.example.org"  # OR a shared quipu server
# token_file = "~/.config/seeds/quipu-token"   # bearer for server writes

[project]
prefix = "sd"                        # id prefix for new seeds (default sd)
# graph = "https://seeds.local/project/sd"   # the project's named graph

[pendant]
# dir = ".seeds/pendant"             # keep the ledger in the repo (mode 1)

[sync]
# remote = "https://quipu.example.org"   # what `sd sync` exchanges with (mode 3)
```

Which combination gives which arrangement is in
[Storage modes](storage-modes.md).

- `store` is a path. Relative paths resolve against the directory that holds
  `.seeds/` (in the project file) or against the user config directory (in the
  user file). `~/` expands to your home directory.
- `url` is an `http://` or `https://` quipu server.
- A file (or the environment) that sets **both** is refused with exit 6.
  Choose one.
- Unknown keys are refused too, so a typo such as `stroe` fails loudly instead
  of being ignored.

## Precedence

Highest first. A layer is consulted only when every layer above it says
nothing about that setting.

| # | layer | location keys | other keys |
|---|---|---|---|
| 1 | flags | `--store <path>`, `--quipu <url>` | `--graph` |
| 2 | environment | `SEEDS_QUIPU_STORE`, `SEEDS_QUIPU_URL` | `SEEDS_GRAPH`, `SEEDS_PREFIX`, `SEEDS_ACTOR`, `SEEDS_PENDANT_DIR`, `SEEDS_SYNC_REMOTE`, `SEEDS_QUIPU_TOKEN` |
| 3 | project file | the nearest `.seeds/config.toml`, walking up from the current directory like git | `[project]`, `[pendant]`, `[sync]`, `token_file` |
| 4 | user file | `$XDG_CONFIG_HOME/seeds/config.toml`, else `~/.config/seeds/config.toml` | the same |
| 5 | default | a local store at `<project>/.seeds/seeds.db` | prefix `sd` |

`<project>` in the default is the directory holding the nearest `.seeds/`
(walking up), or the current directory when there is none.

The store location is **one** choice: the highest layer that names either a
`store` or a `url` decides it. A project file that says `store` wins over a
user file that says `url`; the environment wins over both; a flag wins over
everything.

## A shared server

`url` is a shared, team-wide quipu server, read and written live over HTTP
([mode 2](storage-modes.md#mode-2-a-quipu-server)). Writes send
`Authorization: Bearer <token>` when a token is configured (`SEEDS_QUIPU_TOKEN`
or `[quipu] token_file`); reads send none.

A configured URL that cannot be reached is **exit 7**, "cannot reach quipu at
…". seeds **never** falls back to a local store when a configured server is
down, because the two would then hold different ledgers.

## CI

Point a job at a throwaway store with the environment, which beats any config
file in the checkout:

```bash
export SEEDS_QUIPU_STORE="$RUNNER_TEMP/seeds.db" SEEDS_ACTOR=ci
sd create "smoke" --silent
```

## Where this lives in the code

Configuration is resolved only in the native CLI (`src/native/config.rs`). The
core never reads a file or an environment variable; it is handed a storage
backend that is already open, which is what keeps it
[wasm-clean](wasm.md). The precedence rules above are unit-tested there, and
end to end in `tests/cli.rs`.

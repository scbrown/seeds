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
# token_file = "~/.config/seeds/quipu-token"   # bearer for writes: USER config only
# trusted_hosts = ["quipu.example.org"]       # USER config only: see below
# allow_plain_http_hosts = ["quipu.internal.example"]      # USER config only: see below
# signing_key_file = "~/.config/seeds/keys/<session>.key"  # signed writes: USER config only
# signing_session = "<session>"
# signing_introducer = "<who registered it>"

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
- `graph`, when set, must be a plain absolute IRI; anything that could break
  out of the SPARQL it is written into is refused (exit 6).
- Unknown keys are refused too, so a typo such as `stroe` fails loudly instead
  of being ignored.

## Precedence

Highest first. A layer is consulted only when every layer above it says
nothing about that setting.

| # | layer | location keys | other keys |
|---|---|---|---|
| 1 | flags | `--store <path>`, `--quipu <url>` | `--graph` |
| 2 | environment | `SEEDS_QUIPU_STORE`, `SEEDS_QUIPU_URL` | `SEEDS_GRAPH`, `SEEDS_PREFIX`, `SEEDS_ACTOR`, `SEEDS_PENDANT_DIR`, `SEEDS_SYNC_REMOTE`, `SEEDS_QUIPU_TOKEN`, `SEEDS_MAX_WRITE_BYTES`, `SEEDS_MAX_WRITE_CLAUSES` |
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

### Where the token may go

A cloned repository's `.seeds/config.toml` is not yours, so it is not trusted
with your token:

- `token_file`, `trusted_hosts` and `allow_plain_http_hosts` are honoured
  only in your **user** config (or `SEEDS_QUIPU_TOKEN_FILE` /
  `SEEDS_QUIPU_TOKEN`); a project file that sets any of them is refused.
- A server URL that came from a **project** file gets your token only if its
  host is in your `trusted_hosts`. A URL you chose (flag, environment, user
  file) always does.
- A token is never sent over plain `http://`, except to localhost or to a
  host your user config lists in `allow_plain_http_hosts`. That list is for a
  server on a network you trust that has no TLS. An entry matches exactly: a
  bare host (`quipu.internal.example`) admits any port on that host, `host:port` admits only
  that port, and nothing is matched by suffix.

### Signed writes: no bearer on the wire

A quipu server that accepts signed writes (`POST /knot`, `/update`, `/episode`)
does not need your bearer at all. `sd key init` makes an Ed25519 key that never
leaves your machine; its public half is registered **once** on the server by
someone else (your lead or a human, never you). After that every write carries
an `x-quipu-attestation` header: a signature over that one request (method,
path, content type, body hash) with a single-use nonce. Nothing in it can be
reused, so plain `http://` is no longer an authentication risk, and the server
records the write as yours, not as "whoever holds the token".

```toml
# ~/.config/seeds/config.toml (USER config only; sd key init prints these)
[quipu]
signing_key_file = "~/.config/seeds/keys/seeds-myhost-me.key"
signing_session = "seeds-myhost-me"
signing_introducer = "wu"
```

- A signed write never also sends the bearer.
- A key is used only for a server you would send your token to: a URL from a
  project file needs its host in `trusted_hosts`. The signature does not name
  the server, so a server a cloned project chose could otherwise pass your
  signed write on to yours.
- `/graph/create` is signed too. seeds creates a graph only when the server
  does not already list it (`GET /graphs`). Against a server too old to
  accept a signed `/graph/create`, seeds retries that one idempotent call
  with your token, if you have one.
- The key must be registered **with write granted** (`--allow-write`, or
  `quipu attest allow-write <session>` later). A key registered only to trust
  someone's shares is refused with `scope`.
- A refusal says which check failed: `skew` (fix this machine's clock),
  `unbound` (the key is not registered: `sd key show`), `scope` (registered
  without write), `revoked`/`expired`, `replay`, `badsig`.

See [sd key](verbs/key.md).

## The project id

With no `graph` configured, a project's graph is
`https://seeds.local/project/<prefix>-<id>`, where `<id>` is a random id kept
in `.seeds/project-id`. It is created on the first write (or first use of a
server) and should be **committed**: it is what keeps two repositories that
never set a prefix from sharing one ledger on a shared server. A plain read in
a directory that never wrote creates nothing. A local store that exists
without its project id is refused rather than read as empty.

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

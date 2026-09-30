# The verbs

| verb | writes | page |
|---|---|---|
| `init` | yes (files) | [init](init.md) |
| `create` | yes | [create](create.md) |
| `show` | no | [show](show.md) |
| `list` | no | [list](list.md) |
| `ready` | no | [ready](ready.md) |
| `search` | no | [search](search.md) |
| `stale` | no | [stale](stale.md) |
| `stats` (alias `status`) | no | [stats](stats.md) |
| `q` | yes | [q](q.md) |
| `query`, `upgrade`, `gate`, `scheduler`, `audit`, `robot-docs` | no (exit 21, pointer) | [elsewhere](q.md) |
| `count` | no | [count](count.md) |
| `update` | yes | [update](update.md) |
| `close` | yes | [close](close.md) |
| `reopen`, `defer`, `undefer` | yes | [reopen, defer, undefer](reopen.md) |
| `delete` | yes (tombstone) | [delete](delete.md) |
| `blocked` | no | [blocked](blocked.md) |
| `label add`, `label remove`, `label rename`, `label list`, `label list-all` | add/remove/rename | [label](label.md) |
| `graph` | no | [graph](graph.md) |
| `history` | no | [history](history.md) |
| `changelog` | no | [changelog](changelog.md) |
| `epic status`, `epic close-eligible` | close-eligible | [epic](epic.md) |
| `dep add`, `dep remove`, `dep list` | add/remove | [dep](dep.md) |
| `comments add`, `comments list` | add | [comments](comments.md) |
| `version`, `where`, `info` | no | [version, where, info](about.md) |
| `config list`, `get`, `path` | no | [config](config.md) |
| `completions` | no | [completions](completions.md) |
| `export`, `import`, `sync`, `merge-driver` | import, sync, merge-driver | [export, import, sync](sync.md) |

Every verb accepts the [global options](../reference.md#global-options), and
every read accepts `--at <tx>`. A verb that writes prints the transaction it
wrote as `(tx N)` in text mode and as `tx` in `--json` (on the object, or on
each seed of a bare array); that number is what `--at` takes. A `--dry-run`
writes nothing, so its `tx` is `null`. Against a **quipu server** a write's
`tx` is also `null` today: quipu's `/update` returns no transaction id
(aegis-xajsgn). `--at` reads still work there, and [history](history.md)
recovers each version's transaction from them.

`--json` output is a contract: every documented key is always present (an
absent value is `null`, never a missing key), and `tests/json_schema.rs` pins
the key sets. Errors in `--json` mode print `{"error": {"code", "message"}}`
on stdout and exit with the code from [Reference](../reference.md#exit-codes).

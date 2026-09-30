# sd key

The key that signs your writes to a quipu server, so no bearer goes on the wire
(see [Signed writes](../config.md#signed-writes-no-bearer-on-the-wire)).

```bash
sd key init --introducer wu            # session defaults to seeds-<host>-<user>
sd key init --introducer wu --session seeds-ci --agent urn:ci:seeds
sd key show
```

`sd key init`:

- writes a new Ed25519 key to `~/.config/seeds/keys/<session>.key`, readable
  by you only, and **never overwrites** an existing key;
- prints the three `[quipu]` lines to add to your **user** config;
- prints the one `quipu attest register` command your introducer runs on the
  quipu host. seeds never registers its own key: a key that vouches for itself
  proves nothing.

`sd key show` prints the configured key's public key, `key_id` and the same
register command. It refuses a key file that others can read.

Nothing here touches the ledger. Rotation is a new key plus revoking the old
session on the server.

**`--json`**: `{created, key_file, session, introducer, agent, public_key,
key_id, register_command, config}`.

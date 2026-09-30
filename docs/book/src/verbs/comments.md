# sd comments

```bash
sd comments add sd-a3f "found the off-by-one in the lexer"
sd comments add sd-a3f --file notes.md      # safest for long or quoted text
sd comments list sd-a3f --json
```

`comments add` takes the text one way: positional words (joined with spaces),
`-m/--message` (alias `--content`), or `-f/--file`. The author is `--author`,
else the actor. Comments are append-only and numbered 1, 2, 3 per seed; the
number is the comment's `id`.

**`--json`**: one comment (add) or an array (list), each `{id, issue_id,
author, text, created_at}`; `add` also carries `tx`.

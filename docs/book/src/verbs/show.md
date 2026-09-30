# sd show

Show one or more seeds with their dependencies, dependents and comments.

```bash
sd show sd-a3f
sd show sd-a3f sd-b7c --json
sd show sd-a3f --at 12          # as it stood at transaction 12
```

**`--json`**: an array, one object per id, with every [seed key](list.md)
plus:

- `dependencies`: what this seed depends on, each `{id, title, status,
  priority, dependency_type}`;
- `dependents`: what depends on this seed, same shape;
- `comments`: each `{id, issue_id, author, text, created_at}`.

**Exit codes**: 0; 3 when any id does not exist (nothing is printed for the
others).

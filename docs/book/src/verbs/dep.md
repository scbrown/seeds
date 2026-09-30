# sd dep

```bash
sd dep add sd-a3f sd-b7c              # sd-a3f is blocked by sd-b7c
sd dep add sd-a3f sd-epic -t parent-child
sd dep remove sd-a3f sd-b7c
sd dep list sd-a3f                    # what sd-a3f depends on
sd dep list sd-b7c --direction up     # what depends on sd-b7c
```

`dep add <issue> <depends-on>` records that *issue* depends on *depends-on*.
Types (`-t, --type`):

| type | meaning | blocks `ready`? |
|---|---|---|
| `blocks` (default) | issue cannot proceed until depends-on closes | yes |
| `parent-child` | issue is a child of depends-on (one parent) | no |
| `related` | a link | no |
| `discovered-from` | issue was found while working depends-on | no |

- Both seeds must exist (exit 3). A seed cannot depend on itself, and a
  `blocks` edge that would close a cycle is refused with the cycle printed
  (exit 5).
- Adding an edge that already exists is a no-op (`action: unchanged`).
- `dep remove` of an edge that is not there is exit 3.

**`--json`**: `dep add` and `dep remove` print `{status, issue_id,
depends_on_id, type, action, tx}`; `dep list` prints an array of
`{issue_id, depends_on_id, type, title, status, priority}`, where the title,
status and priority are the other end's.

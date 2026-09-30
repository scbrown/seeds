# sd label

Add, remove, rename and list labels. The argument shapes are br's.

```bash
sd label add sd-a3f sd-b7c infra        # the last positional is the label
sd label add sd-a3f -l infra            # or name it with -l
sd label remove sd-a3f -l infra
sd label rename infra ops               # on every seed that carries it
sd label list sd-a3f                    # one seed's labels
sd label list                           # every label in use
sd label list-all                       # every label, with counts
```

- `add` and `remove` change every named seed in **one transaction**, or none:
  an unknown id is exit 3 and nothing is written.
- Each seed reports br's status: `added` or `exists`, `removed` or
  `not_found`. A seed that needs no change is not written.
- `list` and `list-all` include closed seeds, as br does.
- A label is one non-empty word without a comma (labels are comma-separated
  in `create -l`).

**`--json`**: `add`/`remove` print `[{status, issue_id, label, tx}]`;
`list` prints an array of strings; `list-all` prints `[{label, count}]`;
`rename` prints `{old_name, new_name, affected_issues, tx}`.

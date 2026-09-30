# sd graph

The dependency graph around a seed, br's way.

```bash
sd graph sd-a3f                   # what sd-a3f unblocks
sd graph sd-a3f --dependencies    # what sd-a3f waits on
sd graph --all --json             # every active seed, as connected components
sd graph --all --dot | dot -Tsvg > work.svg
```

- **Edges point from the waiter to what it waits on**: `B -> T` when B is
  blocked by T, and `P -> C` when parent P waits on its child C.
- The default walks **dependents over `blocks` edges only**: what finishing
  this seed unblocks. `--dependencies` walks the other way over both kinds:
  blockers, and a parent's children.
- `--all` takes every open, in-progress or blocked seed and splits it into
  connected components, each rooted at the seeds that wait on nothing.
- Depth is the breadth-first distance from the root(s). Deleted seeds take no
  part.
- `--compact` prints one line per seed; `--dot` prints Graphviz and overrides
  text and `--json`.

**`--json`**: `{root, nodes: [{id, title, status, priority, depth}], edges:
[[from, to]], count}`; with `--all`, `{total_components, total_nodes,
components: [{nodes, edges, roots}]}`.

# Formulas: shuttle, not a second engine

beads carries its own formula engine: molecules (`mol-*`) are workflow
templates that stamp out a chain of dependent issues and drive them. **seeds
will not build one.** Formula support in seeds is integration with
[shuttle](https://github.com/scbrown/shuttle), the quipu stack's workflow
engine, and it is planned as a follow-on to v0.1.

## What shuttle already is

- A workflow is a JSON **definition**: a name, an initial state, terminal
  states, and transitions `{step, from, to}` (shuttle's `examples/triage.json`).
- A **run** is started with a caller-chosen id (`shuttle start <definition>
  <run> --agent A`) and moved one signed transition at a time (`shuttle advance
  <run> --step S --agent A`). Each transition is signed with the agent's
  ed25519 key.
- `shuttle export` writes runs into quipu with camayoc's workflow vocabulary:
  `aegis:WorkflowDefinition`, `aegis:WorkflowRun` (`aegis:runOf` its
  definition, `aegis:currentState`), and an append-only `aegis:TransitionEvent`
  per step. Runs are `urn:shuttle:run:<id>`, definitions
  `urn:shuttle:workflow:<name>`.

## What seeds provides today

Every seed can carry the run that created or drives it:

```bash
sd create "Review the release" --workflow-run release-42      # urn:shuttle:run:release-42
sd update sd-a3f --workflow-run urn:shuttle:run:release-42
```

It is stored as `seeds:workflowRun <urn:shuttle:run:…>` and appears as
`workflow_run` in `--json`. From the run, camayoc's `aegis:runOf` reaches the
definition, so "which seeds did this workflow stamp" and "which workflow drives
this seed" are both one query over the graph.

Neither camayoc nor shuttle has a term linking a WorkItem to a WorkflowRun yet.
`seeds:workflowRun` is a stopgap and a proposal for camayoc; the runs,
definitions and transitions themselves stay in shuttle's `aegis:` terms rather
than a parallel seeds vocabulary.

## The follow-on, sketched

A formula becomes a shuttle definition whose steps name the work items to
create. Starting a run stamps them: shuttle (or a thin driver around it) calls
the seeds library or `sd create --workflow-run <run> --deps …` for each step,
so the chain of dependent seeds exists as ordinary seeds with `blocks` edges,
and `sd ready` surfaces the first one. As each seed closes, the driver
advances the run (`shuttle advance <run> --step <step>`); as the run reaches a
terminal state, the driver closes whatever seeds remain with the run's outcome.
seeds stays a tracker; shuttle stays the engine; the graph joins them.

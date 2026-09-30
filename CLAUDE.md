# seeds - Agent Instructions

## Project Overview

seeds is an experiment: beads-shaped work items stored as facts in a
[quipu](https://github.com/scbrown/quipu) knowledge graph. It answers the
`bd`/`br` verbs agents already type, with `--json` in the same shape, and it
exists to test one design question: **fact-level versus snapshot versioning**.
It is deliberately **not** a competing beads implementation. The command is `sd`;
the project, repository and prose name are "seeds"; the crate is `seeds-ai`. Read
`docs/book/src/experiment.md` before changing its scope.

Sibling repos: scbrown/quipu (governed store), scbrown/camayoc (shapes and
stored queries), scbrown/caboodle (installer and verify),
scbrown/desire-path (the `bd` -> `sd` redirect), scbrown/yupana (code
structure), scbrown/bobbin (retrieval).

## Conventions

- **Map the intent, never copy the SQL surface.** A new verb or flag starts
  from `docs/book/src/intent-map.md`: what the caller is trying to do, and what
  that means on a graph store.
- **Flags follow `br`.** When a verb exists in `br`, match its flag names and
  short forms, and match the `bd` JSON output shape.
- **Exit codes are a contract.** Each verb has its own code (see
  `docs/book/src/reference.md`). Exit 2 is reserved for usage errors. Do not
  reuse or renumber a code; add a test when you add one.
- **Reads before writes.** Reads go to quipu `/query`; writes go to quipu
  `/knot`, into the sandbox named graph unless configured otherwise.
- **Definitions live in camayoc.** "ready", "blocked" and a worker's plate are
  stored queries next to the shapes, not logic hardcoded here.
- **Public-safe.** No internal hostnames, private IP addresses, home paths or
  personal names in code, docs, tests or commit messages. Endpoints are
  configuration (`--quipu`, `SEEDS_QUIPU_URL`), never literals.
- **The book is the documentation.** Anything longer than a README line goes in
  `docs/book/src/`, linked from `SUMMARY.md` and routed on the docs map.

## Build Commands

```bash
just build           # cargo build
just test            # cargo test
just check           # fmt --check, clippy -D warnings, test, mdbook build (what CI runs)
just book            # build the mdBook
```

## Git Workflow — trunk-based, straight to `main`

**Work on `main` and push to `main`.** Do not create feature branches, and do
not open pull requests, unless you are explicitly asked for one.

```bash
git pull --rebase origin main     # before starting, and again before pushing
# ... work, with `just check` green ...
git add -A && git commit && git push origin main
```

There is no review step between a commit and the history everyone else pulls,
so **the quality gates are the only gate**.

- **Run `just check` before every push**, not once at session end.
- **Never force-push `main`.** If a push is rejected, `git pull --rebase` and
  resolve it.
- **Prefer small complete commits.** Each one lands live, so each one has to
  stand on its own.
- **Work that cannot pass the gates does not get pushed.** Finish it, or leave
  it uncommitted and say so at handoff.
- **Conventional commit subjects** (`feat:`, `fix:`, `docs:`, `chore:`, …).

## Releases

Versioning is release-please: conventional commits on `main` feed a release
PR, and merging it tags, builds the `sd` binaries for four targets, publishes
the GitHub release and publishes `seeds-ai` to crates.io through Trusted
Publishing (no registry token in the repo). `crates.yml` is the manual recovery
lane. To rehearse without releasing, dispatch `release.yml` with
`dry_run: true` (the default).

## Before Every Push

Run `just check`. Do not push on failure. Work is not complete until
`git push` succeeds and CI on `main` is green.

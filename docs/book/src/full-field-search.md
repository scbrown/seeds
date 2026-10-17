# Remote full-field search

`sd search TERM` matches titles and descriptions. Explicit `--full` additionally
matches identifiers and valid comments, with the same filters,
closed-item counts, sorting and pagination as the snapshot implementation.

For current remote reads, the server must support the explicit substring-index
contract and have a complete compatible index. A cheap readiness request precedes
text selection. Each subsequent request requires the same contract; a missing
acknowledgement or unavailable index refuses rather than materializing a board
snapshot. No positive or negative readiness result is cached between searches.
Terms shorter than three Unicode characters after lowercasing, or containing NUL,
refuse before query evaluation because this index cannot accelerate them. Both remote search scopes require this minimum; shorter terms refuse explicitly.

The backend projects matching subjects before joining compact metadata, validates
comment parents and integer positions, and deduplicates matching comment parents
before item eligibility checks. Main and ephemeral graphs retain their effective
shadowing rules. Only the selected page's item bodies are hydrated.

An operator must explicitly prepare the optional index using the matching Quipu
maintenance tooling. A source update does not authorize production backfill.
Dropping it makes both remote search scopes refuse; neither falls back to a
whole-board scan. Historical reads retain the existing snapshot read-cost gate.

The public synthetic demo and protocol contract live in Quipu's
`docs/probes/pr-demos/substring-search/` and
`docs/book/src/reference/substring-search.md`.

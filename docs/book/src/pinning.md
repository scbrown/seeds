# Pinning: `--at <tx>`

The pinning demo is the sharpest test of the experiment's question.

## The idea

In a snapshot-versioned store, "what did this look like before?" means
checking out the whole database at an older commit. Everything else rewinds
with it.

In seeds, every fact carries the transaction that asserted it and, once
replaced, the transaction that retracted it. So "what did this one seed look
like at transaction 1042?" is a query over that seed's facts alone. Nothing
else rewinds.

## The demo

```bash
sd create "Ship the parser" -p 1 --json      # -> s-7, written in tx 1042
sd update s-7 --status in_progress           # tx 1043
sd update s-7 --title "Ship the streaming parser" -p 0   # tx 1051

sd show s-7                  # today: P0, "Ship the streaming parser", in_progress
sd show s-7 --at 1042        # the pin: P1, "Ship the parser", open
sd ready --json              # the project has moved on without it
```

The pin still resolves to the old state while the rest of the project reads
current. That is a fact-level pin.

## A pin that crosses teams

When a ledger is shared as a qpack (see [Sharing a ledger](qpack-sharing.md)),
a pinned reference is three things:

- the payload hash of the shared ledger;
- the transaction id;
- the seed's IRI.

Together they name exactly one version of one work item, in another team's
ledger, without either team exposing the rest of its history.

## Status

`--at <tx>` is accepted by the v0 parser on every verb. Resolving it is not
implemented yet.

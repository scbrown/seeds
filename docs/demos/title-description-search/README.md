# Default title-and-description search

Watch the same `sd search 'read process pool'` input change from zero results to one description-only match. The title positive control succeeds on the base. A comment-only term stays absent by default and appears with `--full`.

This is an actual terminal recording against a disposable local fixture, with no network calls. Base source: `62ce6b29448ca6d00970f36a0c44efa027ad8076`. Candidate implementation: `fa1d4356cd055d274e9d98da3318d2afcfd0d3b0`; SDK executable SHA-256: `7d53272617ee41d9cb6db6e4b8d1049dce1c73531edfdee38d94d9272e3cb68c`.

Read [the transcript](output.txt), or download it with [the timing file](timing.txt) and replay from this directory:

```sh
scriptreplay --log-timing timing.txt --log-out output.txt
```

Playback was verified with util-linux scriptreplay. To reproduce with separately built binaries:

```sh
BASE_SD=/path/to/base/sd CANDIDATE_SD=/path/to/candidate/sd python3 demo.py
```

The recording demonstrates local semantics and output scope. It does not demonstrate production installation, remote index readiness or production resource cost. Remote search requires a compatible indexed provider and refuses unsupported readiness rather than scanning or silently reducing its scope. The small recording and caption are retained here for review.

# Owner readiness request demonstration

This is a retained real test transcript, not a terminal recording. A terminal
recorder is unavailable in the author environment. The same synthetic fixture
has 500 closed items and two open ready items assigned to one owner. The before
source is `2b9d96d`; the after source is the pull request head. The independent
unscoped readiness result still contains both returned IDs.

Before:

```text
owner history: 500 closed, 2 ready, 52 query requests
test native::remote::scoped_tests::owner_ready_requests_do_not_grow_with_closed_history ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 94 filtered out; finished in 2.40s
```

After:

```text
owner history: 500 closed, 2 ready, 2 query requests
test native::remote::scoped_tests::owner_ready_requests_do_not_grow_with_closed_history ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 94 filtered out; finished in 2.70s
```

Repeat on an isolated server with the CI-pinned server artifact:

```sh
SEEDS_TEST_QUIPU_SERVER=/path/to/private/quipu-server \
  cargo test --lib owner_ready_requests_do_not_grow_with_closed_history -- --nocapture
```

Removing the open-status discovery restriction makes the request-budget
assertion fail at 52 queries again; restoring it passes at two. This proves the
fixture's request and membership behavior. It does not measure production
traffic, memory, full-board parity, or general SHOW request acceptance. The
transcript is retained in this repository alongside the regression test.

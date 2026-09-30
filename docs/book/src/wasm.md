# WebAssembly

The seeds core builds for `wasm32-unknown-unknown`, and its test suite runs
there.

## What is the core

The crate has two layers:

- **the core**: the work-item model (`model`), the verbs (`engine`), the ready
  computation, the JSON output (`output`), the storage seam (`backend`) and its
  quipu implementation (`quipu_backend`);
- **`native`** (the default feature): the `sd` CLI on top of it: arguments,
  [configuration](config.md), the store file and its write lock, and the
  system clock.

The core never reads a clock, a file, the environment, a process, a thread or
the network. Time arrives in `Ctx::now`; storage arrives as a `Backend`. That
rule is enforced, not just stated: `clippy.toml` disallows those APIs, and
only `src/native/` opts out. The wasm build alone would not catch them,
because most of them compile for wasm32 and fail only at run time.

## Storage on wasm

The backend on wasm is the same `QuipuBackend` the CLI uses, over
`quipu::store::Store::open_in_memory()`. quipu's library builds for wasm32 with
default features off (its CI gates exactly that, with an in-memory SQLite and a
wasm clock shim), so seeds did not need a second, hand-written graph backend.
What is missing on wasm is quipu's SHACL engine, which quipu leaves out of its
own wasm gate: `QuipuBackend::validates()` is false there, and the one
structural rule that matters most (a `blocks` edge must point at a real seed)
is checked by hand.

```rust
use seeds::backend::Ctx;
use seeds::engine::{self, CreateReq, ReadyReq};
use seeds::quipu_backend::QuipuBackend;

let mut b = QuipuBackend::in_memory("https://seeds.local/project/sd")?;
let ctx = Ctx { now: "2026-09-30T00:00:00Z".into(), actor: "me".into(), prefix: "sd".into() };
let (a, _) = engine::create(&mut b, &ctx, &CreateReq { title: "a".into(), ..Default::default() })?;
```

## Build and test

```bash
just wasm        # build + lint the core for wasm32, link the demo, print its size
just wasm-test   # run tests/core.rs on wasm32 under node
```

which are:

```bash
cargo build --target wasm32-unknown-unknown --lib --no-default-features --release
cargo test  --target wasm32-unknown-unknown --no-default-features --test core
```

`.cargo/config.toml` supplies the two wasm32-only settings: the
`getrandom_backend="wasm_js"` cfg quipu's dependencies need, and
`wasm-bindgen-test-runner` as the test runner. The runner must match the
`wasm-bindgen` version in `Cargo.lock`:

```bash
cargo install wasm-bindgen-cli --version "$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')" --locked
```

`tests/core.rs` is one file for both targets: each test is a `#[test]`
natively and a `#[wasm_bindgen_test]` on wasm32, so the verbs, the ready
computation, pinning and the compare-and-set are proved on both.

The `ready_demo` example (create, dep add, ready, close, ready) links the whole
core into one module; its release size is what `just wasm` reports.

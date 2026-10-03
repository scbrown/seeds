//! seeds: beads-shaped work items stored as facts in a quipu knowledge graph.
//!
//! The crate is two layers:
//!
//! - **the core** (everything except [`native`]): the work-item model, the
//!   verbs ([`engine`]), the ready computation, JSON output, the storage seam
//!   ([`backend::Backend`]) with its quipu implementation
//!   ([`quipu_backend::QuipuBackend`]), the ledger as a quipu pendant
//!   ([`pendant`]), and import and three-way sync between stores ([`sync`]). It reads no clock, file, environment,
//!   process or network, so it builds for `wasm32-unknown-unknown` with
//!   `--no-default-features` (`just wasm`). `clippy.toml` enforces that.
//! - **`native`** (the default feature): the `sd` CLI on top: argument
//!   parsing, TOML configuration, the local store file and its write lock,
//!   pendant files on disk, the quipu server client, and the system clock.
//!
//! ```
//! use seeds::backend::Ctx;
//! use seeds::engine::{self, CreateReq, ReadyReq};
//! use seeds::quipu_backend::QuipuBackend;
//!
//! let mut b = QuipuBackend::in_memory("https://seeds.local/project/sd").unwrap();
//! let ctx = Ctx { now: "2026-09-30T00:00:00Z".into(), actor: "me".into(), prefix: "sd".into(), claims: Default::default() };
//! let (a, _) = engine::create(&mut b, &ctx, &CreateReq { title: "a".into(), ..Default::default() }).unwrap();
//! let (blocker, _) = engine::create(&mut b, &ctx, &CreateReq { title: "b".into(), ..Default::default() }).unwrap();
//! engine::dep_add(&mut b, &ctx, &a.id, &blocker.id, "blocks").unwrap();
//! let ready = engine::ready(&b, &ctx, &ReadyReq::default(), None).unwrap();
//! assert_eq!(ready.issues.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec![blocker.id]);
//! ```

pub mod backend;
pub mod beads;
pub mod engine;
pub mod error;
pub mod ids;
pub mod model;
pub mod output;
pub mod pendant;
pub mod quipu_backend;
pub mod schema;
pub mod sync;
pub mod validate;
pub mod vocab;

#[cfg(feature = "native")]
pub mod native;

//! The storage seam.
//!
//! The core (verbs, the ready computation, JSON output) talks to storage only
//! through [`Backend`]. The one implementation today is
//! [`crate::quipu_backend::QuipuBackend`], which runs on a quipu store: a local
//! file in the native `sd` CLI, an in-memory store on wasm32. A backend never
//! reads a clock or the filesystem on its own behalf; time arrives in [`Ctx`].

use crate::error::Result;
use crate::model::{Comment, Seed, Snapshot};

/// Who is acting, and when. Injected by the caller: the core never reads a
/// clock, so it runs unchanged on wasm32.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// The current instant, ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
    pub now: String,
    /// The actor, recorded on every transaction and used by `--claim`.
    pub actor: String,
    /// The id prefix for new seeds.
    pub prefix: String,
}

/// One seed to write, with the revision the writer read it at.
#[derive(Debug, Clone)]
pub struct SeedWrite {
    /// The complete post-state of the seed.
    pub seed: Seed,
    /// The revision the writer based this on: `None` for a new seed (which
    /// must not exist yet), `Some(r)` for an update (which must still be at
    /// revision `r`). A mismatch is a [`crate::error::ErrorKind::Conflict`]
    /// and nothing in the batch is written.
    pub expected_revision: Option<u64>,
}

/// Everything one verb writes. A batch is one transaction: all of it lands, or
/// none of it does.
#[derive(Debug, Clone, Default)]
pub struct WriteBatch {
    /// Seeds to create or replace.
    pub seeds: Vec<SeedWrite>,
    /// New comments (a comment is never edited).
    pub comments: Vec<Comment>,
    /// Seeds to remove entirely, each with the revision the writer read. Only
    /// import and sync remove seeds (to match a ledger that no longer holds
    /// them); no verb deletes.
    pub delete_seeds: Vec<(String, u64)>,
    /// Comments to remove, as (seed id, index). Only sync uses this, to
    /// renumber a comment that collided with one written elsewhere.
    pub delete_comments: Vec<(String, u64)>,
    /// The transaction source tag, e.g. `seeds:update`.
    pub source: String,
}

impl WriteBatch {
    /// A batch that changes nothing.
    pub fn is_empty(&self) -> bool {
        self.seeds.is_empty()
            && self.comments.is_empty()
            && self.delete_seeds.is_empty()
            && self.delete_comments.is_empty()
    }
}

/// Storage for one project.
pub trait Backend {
    /// Every seed and comment, as of transaction `at` (current state when
    /// `None`).
    fn snapshot(&self, at: Option<u64>) -> Result<Snapshot>;

    /// The ids the ready definition ([`crate::vocab::ready_query`]) selects, as
    /// of `at`. Filters and the defer date are applied by the caller.
    fn ready_ids(&self, at: Option<u64>) -> Result<Vec<String>>;

    /// Apply a batch atomically, checking every [`SeedWrite::expected_revision`]
    /// first. Returns the transaction id, or the current head when the batch
    /// changed nothing.
    fn commit(&mut self, batch: &WriteBatch, ctx: &Ctx) -> Result<u64>;
}

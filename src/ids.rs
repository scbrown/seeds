//! Seed ids: `<prefix>-<short hash>`, like br's.
//!
//! The short part is base36 of a SHA-256 over the title, the creation instant
//! and an attempt counter. It starts at 3 characters and grows by one after
//! every few collisions, so ids stay short in small projects and stay unique
//! in large ones. No randomness is involved, which keeps the core free of an
//! entropy source (and so wasm-clean); uniqueness is checked against the
//! project under the write lock.
//!
//! A child created with `--parent P` gets `P.<n>`, the next free integer.

use sha2::{Digest, Sha256};

const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const MIN_LEN: usize = 3;
const MAX_LEN: usize = 12;

/// Mint a new id that `taken` does not already hold.
pub fn mint(prefix: &str, title: &str, now: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut attempt: u64 = 0;
    loop {
        let len = (MIN_LEN + (attempt / 4) as usize).min(MAX_LEN);
        let id = format!("{prefix}-{}", short(title, now, attempt, len));
        if !taken(&id) {
            return id;
        }
        attempt += 1;
    }
}

/// The id of the seed a workflow step creates: `<prefix>-w<8 base36>` from a
/// SHA-256 over (run, step, visit). Deterministic, so a retry, or a second
/// reconcile racing the first, names the SAME seed, and the write's
/// compare-and-set refuses the duplicate atomically. `visit` counts entries
/// into the step, so a step the run revisits gets a new seed rather than the
/// first, already-closed one.
pub fn keyed(prefix: &str, run: &str, step: &str, visit: u32) -> String {
    format!(
        "{prefix}-w{}",
        short(&format!("{run}\n{step}"), &visit.to_string(), 0, 8)
    )
}

/// The next child id under `parent`: `parent.1`, `parent.2`, ...
pub fn child(parent: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut n: u64 = 1;
    loop {
        let id = format!("{parent}.{n}");
        if !taken(&id) {
            return id;
        }
        n += 1;
    }
}

fn short(title: &str, now: &str, attempt: u64, len: usize) -> String {
    let digest = Sha256::digest(format!("{title}\n{now}\n{attempt}").as_bytes());
    let mut n = u128::from_be_bytes(digest[..16].try_into().expect("16 bytes"));
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(ALPHABET[(n % 36) as usize]);
        n /= 36;
    }
    String::from_utf8(out).expect("ascii")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_keyed_id_depends_on_run_step_and_visit_only() {
        let a = keyed("sd", "urn:shuttle:run:r1", "triage", 1);
        assert_eq!(a, keyed("sd", "urn:shuttle:run:r1", "triage", 1));
        assert!(a.starts_with("sd-w") && a.len() == "sd-w".len() + 8, "{a}");
        assert_ne!(a, keyed("sd", "urn:shuttle:run:r1", "triage", 2));
        assert_ne!(a, keyed("sd", "urn:shuttle:run:r1", "review", 1));
        assert_ne!(a, keyed("sd", "urn:shuttle:run:r2", "triage", 1));
        // The separator keeps (run, step) pairs from aliasing.
        assert_ne!(keyed("sd", "a", "bc", 1), keyed("sd", "ab", "c", 1));
    }

    #[test]
    fn ids_have_the_prefix_and_a_short_base36_tail() {
        let id = mint("sd", "a title", "2026-09-30T00:00:00Z", |_| false);
        let tail = id.strip_prefix("sd-").unwrap();
        assert_eq!(tail.len(), 3);
        assert!(tail.bytes().all(|b| ALPHABET.contains(&b)));
    }

    #[test]
    fn collisions_are_resolved_and_ids_grow() {
        let mut taken = HashSet::new();
        for _ in 0..500 {
            let id = mint("sd", "same title", "same instant", |c| taken.contains(c));
            assert!(taken.insert(id));
        }
        assert!(taken.iter().any(|id| id.len() > "sd-".len() + 3));
    }

    #[test]
    fn child_ids_count_up() {
        assert_eq!(child("sd-abc", |_| false), "sd-abc.1");
        assert_eq!(child("sd-abc", |c| c == "sd-abc.1"), "sd-abc.2");
    }
}

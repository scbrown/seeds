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
//!
//! `sd create --slug S` embeds a normalized slug: `<prefix>-<slug>-<short hash>`.

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

/// The longest normalized slug, br's cap.
const SLUG_MAX: usize = 48;

/// br's `--slug` normalization: lowercase ASCII letters and digits, every run
/// of anything else (non-ASCII included) collapsed to one hyphen, leading and
/// trailing hyphens stripped, then capped at 48 characters (stripping a
/// hyphen the cap exposes). `None` when nothing is left, and the id is then
/// minted without a slug, as br does.
pub fn slug(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(SLUG_MAX);
    let out = out.trim_end_matches('-');
    (!out.is_empty()).then(|| out.to_string())
}

/// Like [`mint`], with a normalized slug between the prefix and the hash.
pub fn mint_slugged(
    prefix: &str,
    slug: &str,
    title: &str,
    now: &str,
    taken: impl Fn(&str) -> bool,
) -> String {
    mint(&format!("{prefix}-{slug}"), title, now, taken)
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
    fn slugs_normalize_like_br() {
        // Expected values are br's observed CLI output for the same inputs
        // (aegis-w3k75d.13; br --help and outputs only, per aegis-fur6v8).
        assert_eq!(slug("Survey My Thing!").as_deref(), Some("survey-my-thing"));
        assert_eq!(slug("--weird__  slug--").as_deref(), Some("weird-slug"));
        assert_eq!(slug("a--b").as_deref(), Some("a-b"));
        assert_eq!(slug("ABC-123").as_deref(), Some("abc-123"));
        assert_eq!(slug("Ünïcode café").as_deref(), Some("n-code-caf"));
        assert_eq!(slug(""), None);
        assert_eq!(slug("!!!"), None);
        assert_eq!(slug(&"a".repeat(60)), Some("a".repeat(48)));
        // The cap lands on a hyphen, which is then stripped: 47 characters.
        let capped = slug(&"ab-".repeat(20)).unwrap();
        assert_eq!(capped, ["ab"; 16].join("-"));
        assert_eq!(capped.len(), 47);
    }

    #[test]
    fn slugged_ids_keep_prefix_slug_and_hash() {
        let id = mint_slugged("sd", "survey-my-thing", "t", "2026-10-01T00:00:00Z", |_| {
            false
        });
        let tail = id.strip_prefix("sd-survey-my-thing-").unwrap();
        assert_eq!(tail.len(), 3);
        assert!(tail.bytes().all(|b| ALPHABET.contains(&b)));
    }

    #[test]
    fn child_ids_count_up() {
        assert_eq!(child("sd-abc", |_| false), "sd-abc.1");
        assert_eq!(child("sd-abc", |c| c == "sd-abc.1"), "sd-abc.2");
    }
}

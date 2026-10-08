//! Structured write provenance for every request seeds sends to quipu.
//!
//! Who wrote a fact must be machine-readable. Beside `X-Quipu-Client: seeds`,
//! each request carries up to five headers, filled by machinery from facts the
//! environment already holds and never typed by a user:
//!
//! | header            | first of                                                        |
//! |-------------------|-----------------------------------------------------------------|
//! | `X-Quipu-Agent`   | `QUIPU_AGENT`; `SHANTY_AGENT` inside an agent session; `seeds`  |
//! | `X-Quipu-Harness` | `QUIPU_HARNESS`; `claude` (`CLAUDECODE=1`); `codex` (`CODEX_HOME`); `cli` |
//! | `X-Quipu-Model`   | `QUIPU_MODEL`; `SHANTY_MODEL`                                   |
//! | `X-Quipu-Session` | `QUIPU_SESSION`; the harness's own: `CODEX_SESSION_ID` then `CODEX_THREAD_ID` in a Codex session, else `CLAUDE_CODE_SESSION_ID` |
//! | `X-Quipu-Host`    | `QUIPU_HOST`; the machine's hostname                            |
//!
//! An agent session is `CLAUDECODE=1` or a non-empty `CODEX_HOME`. Outside one,
//! an inherited `SHANTY_AGENT` is ignored, so a cron or service that merely
//! inherited it is never credited to an agent. A cron or service running `sd`
//! names itself with the overrides, e.g. `QUIPU_AGENT=nightly-sync
//! QUIPU_HARNESS=cron`.
//!
//! An empty variable counts as unset. A field that cannot be filled is
//! OMITTED, never guessed. Values keep only printable ASCII (`0x21..=0x7e`
//! and space), are trimmed of surrounding spaces and cut to 128 characters, so
//! a header can never carry a CR/LF; a value left empty by that is omitted.
//!
//! This is the same contract as the stack's other quipu writers. The header
//! set is computed once per process ([`headers`]).

use std::sync::OnceLock;

/// The agent seeds names itself when no override or agent session does.
pub const PRODUCER: &str = "seeds";
/// The harness seeds names itself when no override or agent session does.
pub const PRODUCER_HARNESS: &str = "cli";
/// The longest value a provenance header may carry.
pub const MAX_LEN: usize = 128;

/// The provenance fields, in header order.
pub const FIELDS: [(&str, &str); 5] = [
    ("agent", "X-Quipu-Agent"),
    ("harness", "X-Quipu-Harness"),
    ("model", "X-Quipu-Model"),
    ("session", "X-Quipu-Session"),
    ("host", "X-Quipu-Host"),
];

/// Keep printable ASCII and space only, trim spaces, cut to [`MAX_LEN`].
/// `None` when nothing is left.
pub fn clean(value: &str) -> Option<String> {
    let kept: String = value
        .chars()
        .filter(|c| matches!(c, '\x21'..='\x7e' | ' '))
        .collect();
    let text: String = kept.trim_matches(' ').chars().take(MAX_LEN).collect();
    (!text.is_empty()).then_some(text)
}

/// The provenance headers `(name, value)` for the environment `env` (a lookup
/// by variable name) on a machine named `hostname`, by the precedence in the
/// module doc. Pure: tests pass a map, the process passes its environment.
pub fn headers_from(
    env: impl Fn(&str) -> Option<String>,
    hostname: impl FnOnce() -> Option<String>,
) -> Vec<(&'static str, String)> {
    // An empty variable is unset, as in the other writers.
    let get = |k: &str| env(k).filter(|v| !v.is_empty());
    let claude = get("CLAUDECODE").as_deref() == Some("1");
    let codex = get("CODEX_HOME").is_some();
    let in_session = claude || codex;
    let session_harness = if claude {
        Some("claude".to_string())
    } else if codex {
        Some("codex".to_string())
    } else {
        None
    };
    let raw = [
        get("QUIPU_AGENT")
            .or_else(|| in_session.then(|| get("SHANTY_AGENT")).flatten())
            .or_else(|| Some(PRODUCER.to_string())),
        get("QUIPU_HARNESS")
            .or(session_harness)
            .or_else(|| Some(PRODUCER_HARNESS.to_string())),
        get("QUIPU_MODEL").or_else(|| get("SHANTY_MODEL")),
        // The session id of the harness this IS. Codex exports its own to tool
        // shells; reading only Claude's left every Codex write sessionless, and
        // an inherited Claude id inside Codex would credit the wrong session.
        get("QUIPU_SESSION").or_else(|| {
            if codex && !claude {
                get("CODEX_SESSION_ID").or_else(|| get("CODEX_THREAD_ID"))
            } else {
                get("CLAUDE_CODE_SESSION_ID")
            }
        }),
        get("QUIPU_HOST").or_else(|| hostname().filter(|h| !h.is_empty())),
    ];
    FIELDS
        .iter()
        .zip(raw)
        .filter_map(|((_, header), value)| Some((*header, clean(&value?)?)))
        .collect()
}

/// This process's provenance headers, computed on first use.
pub fn headers() -> &'static [(&'static str, String)] {
    static HEADERS: OnceLock<Vec<(&'static str, String)>> = OnceLock::new();
    HEADERS.get_or_init(|| {
        headers_from(
            |k| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()),
            hostname,
        )
    })
}

/// Set this process's provenance headers on a request.
pub fn apply(mut req: ureq::Request) -> ureq::Request {
    for (name, value) in headers() {
        req = req.set(name, value);
    }
    req
}

/// The machine's hostname: the kernel's on Linux, else `hostname(1)`.
pub(super) fn hostname() -> Option<String> {
    let read = |p: &str| std::fs::read_to_string(p).ok();
    read("/proc/sys/kernel/hostname")
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        })
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn run(vars: &[(&str, &str)], host: Option<&str>) -> BTreeMap<&'static str, String> {
        let env: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        headers_from(|k| env.get(k).cloned(), || host.map(str::to_string))
            .into_iter()
            .collect()
    }

    fn expect(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn a_plain_shell_names_the_producer() {
        assert_eq!(
            run(&[], Some("box")),
            expect(&[
                ("X-Quipu-Agent", "seeds"),
                ("X-Quipu-Harness", "cli"),
                ("X-Quipu-Host", "box"),
            ])
        );
    }

    #[test]
    fn an_inherited_shanty_agent_outside_a_session_is_ignored() {
        let h = run(&[("SHANTY_AGENT", "x"), ("CLAUDECODE", "0")], Some("box"));
        assert_eq!(h["X-Quipu-Agent"], "seeds");
        assert_eq!(h["X-Quipu-Harness"], "cli");
    }

    #[test]
    fn a_claude_session_names_its_agent_model_and_session() {
        assert_eq!(
            run(
                &[
                    ("CLAUDECODE", "1"),
                    ("SHANTY_AGENT", "x"),
                    ("SHANTY_MODEL", "m-1"),
                    ("CLAUDE_CODE_SESSION_ID", "s-1"),
                ],
                Some("box"),
            ),
            expect(&[
                ("X-Quipu-Agent", "x"),
                ("X-Quipu-Harness", "claude"),
                ("X-Quipu-Model", "m-1"),
                ("X-Quipu-Session", "s-1"),
                ("X-Quipu-Host", "box"),
            ])
        );
    }

    #[test]
    fn a_codex_session_is_a_session_too() {
        let h = run(&[("CODEX_HOME", "/c"), ("SHANTY_AGENT", "x")], Some("box"));
        assert_eq!(h["X-Quipu-Agent"], "x");
        assert_eq!(h["X-Quipu-Harness"], "codex");
        // An empty CODEX_HOME is no session.
        let h = run(&[("CODEX_HOME", ""), ("SHANTY_AGENT", "x")], Some("box"));
        assert_eq!(h["X-Quipu-Agent"], "seeds");
        assert_eq!(h["X-Quipu-Harness"], "cli");
    }

    #[test]
    fn a_codex_session_names_its_own_session() {
        let h = run(
            &[
                ("CODEX_HOME", "/c"),
                ("SHANTY_AGENT", "x"),
                ("CODEX_SESSION_ID", "cs-1"),
                ("CODEX_THREAD_ID", "ct-1"),
                // Inherited from a Claude shell that launched Codex: not ours.
                ("CLAUDE_CODE_SESSION_ID", "s-1"),
            ],
            Some("box"),
        );
        assert_eq!(h["X-Quipu-Session"], "cs-1");
        // The thread id is the fallback when only it is exported.
        let h = run(
            &[("CODEX_HOME", "/c"), ("CODEX_THREAD_ID", "ct-1")],
            Some("box"),
        );
        assert_eq!(h["X-Quipu-Session"], "ct-1");
        // Outside a Codex session its ids are ignored, like SHANTY_AGENT.
        let h = run(&[("CODEX_SESSION_ID", "cs-1")], Some("box"));
        assert!(!h.contains_key("X-Quipu-Session"));
    }

    #[test]
    fn a_session_without_shanty_agent_falls_back_to_the_producer() {
        let h = run(&[("CLAUDECODE", "1")], Some("box"));
        assert_eq!(h["X-Quipu-Agent"], "seeds");
        assert_eq!(h["X-Quipu-Harness"], "claude");
    }

    #[test]
    fn explicit_overrides_win_over_everything() {
        assert_eq!(
            run(
                &[
                    ("CLAUDECODE", "1"),
                    ("SHANTY_AGENT", "x"),
                    ("SHANTY_MODEL", "m-1"),
                    ("CLAUDE_CODE_SESSION_ID", "s-1"),
                    ("QUIPU_AGENT", "nightly"),
                    ("QUIPU_HARNESS", "cron"),
                    ("QUIPU_MODEL", "m-2"),
                    ("QUIPU_SESSION", "s-2"),
                    ("QUIPU_HOST", "named"),
                ],
                Some("box"),
            ),
            expect(&[
                ("X-Quipu-Agent", "nightly"),
                ("X-Quipu-Harness", "cron"),
                ("X-Quipu-Model", "m-2"),
                ("X-Quipu-Session", "s-2"),
                ("X-Quipu-Host", "named"),
            ])
        );
    }

    #[test]
    fn an_empty_override_counts_as_unset() {
        let h = run(
            &[
                ("QUIPU_AGENT", ""),
                ("QUIPU_HARNESS", ""),
                ("QUIPU_HOST", ""),
            ],
            Some("box"),
        );
        assert_eq!(h["X-Quipu-Agent"], "seeds");
        assert_eq!(h["X-Quipu-Harness"], "cli");
        assert_eq!(h["X-Quipu-Host"], "box");
    }

    #[test]
    fn a_field_that_cannot_be_filled_is_omitted() {
        let h = run(&[], None);
        assert!(!h.contains_key("X-Quipu-Host"), "{h:?}");
        assert!(!h.contains_key("X-Quipu-Model"), "{h:?}");
        assert!(!h.contains_key("X-Quipu-Session"), "{h:?}");
        assert!(!run(&[], Some("")).contains_key("X-Quipu-Host"));
    }

    #[test]
    fn a_value_sanitized_to_nothing_is_omitted_not_replaced() {
        // As in the other writers: the override was chosen, then cleaned.
        let h = run(
            &[("QUIPU_AGENT", "\r\n\t"), ("QUIPU_MODEL", "  ")],
            Some("box"),
        );
        assert!(!h.contains_key("X-Quipu-Agent"), "{h:?}");
        assert!(!h.contains_key("X-Quipu-Model"), "{h:?}");
    }

    #[test]
    fn the_sanitizer_drops_control_and_non_ascii_and_caps_length() {
        assert_eq!(clean("a\r\nX-Evil: 1").as_deref(), Some("aX-Evil: 1"));
        assert_eq!(clean("\x00t\x7fa\tb\x1b").as_deref(), Some("tab"));
        assert_eq!(clean("caf\u{e9} \u{2603}x").as_deref(), Some("caf x"));
        assert_eq!(clean("  padded  ").as_deref(), Some("padded"));
        assert_eq!(clean("\u{e9}\n"), None);
        assert_eq!(clean(""), None);
        let long = "z".repeat(300);
        assert_eq!(clean(&long).unwrap().len(), MAX_LEN);
        // Trimmed before the cut, as in the other writers.
        let padded = format!("   {}", "y".repeat(200));
        assert_eq!(clean(&padded).unwrap(), "y".repeat(MAX_LEN));
        // Every character that survives is header-safe.
        let all: String = (0u32..0x3000).filter_map(char::from_u32).collect();
        let kept = clean(&all).unwrap();
        assert!(kept.bytes().all(|b| (0x20..=0x7e).contains(&b)), "{kept:?}");
    }

    #[test]
    fn injection_through_the_environment_cannot_reach_a_header() {
        let h = run(
            &[("QUIPU_SESSION", "s\r\nAuthorization: Bearer x")],
            Some("box\n"),
        );
        assert_eq!(h["X-Quipu-Session"], "sAuthorization: Bearer x");
        assert_eq!(h["X-Quipu-Host"], "box");
    }

    #[test]
    fn the_process_set_is_computed_once_and_names_the_producer_or_agent() {
        let a = headers();
        let b = headers();
        assert!(std::ptr::eq(a, b));
        let names: Vec<&str> = a.iter().map(|(n, _)| *n).collect();
        assert!(names.iter().all(|n| FIELDS.iter().any(|(_, h)| h == n)));
    }
}

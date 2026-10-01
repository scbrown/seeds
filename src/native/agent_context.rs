//! br's `--agent-context` value: inline JSON, `@file` (JSON), or
//! `@file.yaml` / `@file.yml` (YAML 1.2, normalized to JSON).
//!
//! Stored as compact JSON with key order kept and a repeated key resolved to
//! its last value, the form br stores (taken from br's --help and observed
//! CLI output only, aegis-w3k75d.13 / aegis-fur6v8). The core's serde_json
//! map sorts keys, so this carries its own ordered value.

use crate::error::{Result, SdError};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::fmt;
use std::path::Path;
use yaml_rust2::{Yaml, YamlLoader};

/// `None` when the flag was not given; `Some("")` to clear (update) or leave
/// unset (create); otherwise the compact JSON to store.
pub fn resolve(value: &Option<String>) -> Result<Option<String>> {
    let Some(v) = value else { return Ok(None) };
    if v.is_empty() {
        return Ok(Some(String::new()));
    }
    let j = match v.strip_prefix('@') {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| SdError::usage(format!("--agent-context: cannot read {path}: {e}")))?;
            let yaml = Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
            if yaml {
                from_yaml(&text, path)?
            } else {
                from_json(&text, path)?
            }
        }
        None => from_json(v, "inline argument")?,
    };
    let mut out = String::new();
    write(&j, &mut out);
    Ok(Some(out))
}

enum J {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    /// Insert or, for a repeated key, replace in place (the last value wins).
    fn put(fields: &mut Vec<(String, J)>, key: String, value: J) {
        match fields.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => fields.push((key, value)),
        }
    }
}

fn from_json(text: &str, what: &str) -> Result<J> {
    serde_json::from_str(text)
        .map_err(|e| SdError::usage(format!("--agent-context: {what} is not valid JSON: {e}")))
}

fn from_yaml(text: &str, path: &str) -> Result<J> {
    let bad = |m: String| SdError::usage(format!("--agent-context: {path}: {m}"));
    let docs = YamlLoader::load_from_str(text).map_err(|e| bad(format!("not valid YAML: {e}")))?;
    match docs.into_iter().next() {
        Some(doc) => yaml(doc).map_err(bad),
        None => Err(bad("holds no YAML document".into())),
    }
}

fn yaml(y: Yaml) -> std::result::Result<J, String> {
    Ok(match y {
        Yaml::Null => J::Null,
        Yaml::Boolean(b) => J::Bool(b),
        Yaml::Integer(i) => J::Num(i.into()),
        Yaml::Real(r) => r
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(J::Num)
            .ok_or_else(|| format!("{r} has no JSON number form"))?,
        Yaml::String(s) => J::Str(s),
        Yaml::Array(a) => J::Arr(
            a.into_iter()
                .map(yaml)
                .collect::<std::result::Result<_, _>>()?,
        ),
        Yaml::Hash(h) => {
            let mut fields = Vec::new();
            for (k, v) in h {
                let key = match k {
                    Yaml::String(s) => s,
                    Yaml::Integer(i) => i.to_string(),
                    Yaml::Boolean(b) => b.to_string(),
                    Yaml::Real(r) => r,
                    other => return Err(format!("a mapping key must be a scalar, not {other:?}")),
                };
                J::put(&mut fields, key, yaml(v)?);
            }
            J::Obj(fields)
        }
        Yaml::Alias(_) | Yaml::BadValue => return Err("unsupported YAML value".into()),
    })
}

fn write(j: &J, out: &mut String) {
    match j {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Num(n) => out.push_str(&n.to_string()),
        J::Str(s) => out.push_str(&serde_json::to_string(s).expect("a string serializes")),
        J::Arr(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out);
            }
            out.push(']');
        }
        J::Obj(fields) => {
            out.push('{');
            for (i, (k, v)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("a string serializes"));
                out.push(':');
                write(v, out);
            }
            out.push('}');
        }
    }
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        d.deserialize_any(JVisitor)
    }
}

struct JVisitor;

impl<'de> Visitor<'de> for JVisitor {
    type Value = J;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> std::result::Result<J, E> {
        Ok(J::Null)
    }
    fn visit_bool<E>(self, b: bool) -> std::result::Result<J, E> {
        Ok(J::Bool(b))
    }
    fn visit_i64<E>(self, n: i64) -> std::result::Result<J, E> {
        Ok(J::Num(n.into()))
    }
    fn visit_u64<E>(self, n: u64) -> std::result::Result<J, E> {
        Ok(J::Num(n.into()))
    }
    fn visit_f64<E: de::Error>(self, n: f64) -> std::result::Result<J, E> {
        serde_json::Number::from_f64(n)
            .map(J::Num)
            .ok_or_else(|| E::custom("not a finite number"))
    }
    fn visit_str<E>(self, s: &str) -> std::result::Result<J, E> {
        Ok(J::Str(s.to_owned()))
    }
    fn visit_string<E>(self, s: String) -> std::result::Result<J, E> {
        Ok(J::Str(s))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<J, A::Error> {
        let mut items = Vec::new();
        while let Some(x) = a.next_element()? {
            items.push(x);
        }
        Ok(J::Arr(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<J, A::Error> {
        let mut fields = Vec::new();
        while let Some((k, v)) = a.next_entry::<String, J>()? {
            J::put(&mut fields, k, v);
        }
        Ok(J::Obj(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(v: &str) -> String {
        resolve(&Some(v.to_string())).unwrap().unwrap()
    }

    #[test]
    fn json_is_stored_compact_like_br() {
        // Expected values are br's observed CLI output for the same inputs.
        assert_eq!(r(r#"{"b":2, "a":1}"#), r#"{"b":2,"a":1}"#);
        assert_eq!(
            r(r#"{"n": 1.50, "e": 1e2, "big": 12345678901234567890}"#),
            r#"{"n":1.5,"e":100.0,"big":12345678901234567890}"#
        );
        assert_eq!(r(r#"{"s": "café \"q\" \/"}"#), r#"{"s":"café \"q\" /"}"#);
        assert_eq!(r(r#"{"a":1,"a":2}"#), r#"{"a":2}"#);
        assert_eq!(r(" 7 "), "7");
        assert_eq!(r(r#""str""#), r#""str""#);
        assert_eq!(r("null"), "null");
        assert_eq!(r("[1]"), "[1]");
        assert_eq!(r(""), "");
        assert_eq!(resolve(&None).unwrap(), None);
        assert!(resolve(&Some("{bad".into())).is_err());
    }

    #[test]
    fn files_are_read_and_yaml_is_normalized_like_br() {
        let dir = std::env::temp_dir().join(format!("sd-agent-context-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let at = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            format!("@{}", p.display())
        };
        assert_eq!(
            r(&at("c.yaml", "x: 1\nlist: [a, b]\n")),
            r#"{"x":1,"list":["a","b"]}"#
        );
        assert_eq!(
            r(&at("t.yml", "b: 1.50\na: yes\nc: ~\nd: \"x\"\ne: 0x10\n")),
            r#"{"b":1.5,"a":"yes","c":null,"d":"x","e":16}"#
        );
        // Any other extension is read as JSON, as br does.
        assert_eq!(r(&at("c.txt", r#"{"a":1}"#)), r#"{"a":1}"#);
        assert!(resolve(&Some(at("bad.yaml", "a: [unclosed\n"))).is_err());
        assert!(resolve(&Some(format!("@{}", dir.join("nope.json").display()))).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

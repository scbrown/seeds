//! A project's ledger as a **pendant**: quipu's share artifact, as files.
//!
//! seeds does not invent a file format. A pendant is exactly what quipu's
//! `share` produces for one named graph ([`quipu::share::share_payload`]):
//!
//! | file | what it is |
//! |---|---|
//! | `export.nt` | the ledger: every current fact of the project graph, as RDFC-1.0 canonical N-Triples, one fact per line, sorted |
//! | `shapes.ttl` | the shapes the ledger conforms to (the seeds shapes) |
//! | `manifest.json` | the share manifest: hashes of the two files above, the producing store and transaction, and the share id |
//! | `manifest.ttl` | the same manifest as RDF (DCAT + PROV) |
//!
//! `export.nt` is deterministic: the same ledger state always produces the same
//! bytes, sorted line by line, so a pendant committed to git diffs as one line
//! per changed fact. The manifest names the producing store, so two clones that
//! export the same ledger write the same `export.nt` and slightly different
//! manifests; seeds treats `export.nt` as the data and the manifest as a seal
//! it re-checks.
//!
//! Everything here is pure (no filesystem): the native CLI reads and writes
//! the files, and the wasm build can round-trip a pendant held in memory.

use std::collections::BTreeMap;

use quipu::share::{ShareDestination, ShareManifest, ShareOptions, ShareScope};
use sha2::{Digest, Sha256};

use crate::error::{Result, SdError};
use crate::model::{Fact, Obj, Snapshot};
use crate::quipu_backend::{QuipuBackend, SHAPES_NAME};
use crate::validate;

/// The ledger file.
pub const EXPORT_NT: &str = "export.nt";
/// The shapes file.
pub const SHAPES_TTL: &str = "shapes.ttl";
/// The manifest, JSON.
pub const MANIFEST_JSON: &str = "manifest.json";
/// The manifest, Turtle.
pub const MANIFEST_TTL: &str = "manifest.ttl";
/// Every file a pendant consists of.
pub const FILES: [&str; 4] = [EXPORT_NT, SHAPES_TTL, MANIFEST_JSON, MANIFEST_TTL];

/// A pendant's files, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pendant {
    /// File name to exact contents.
    pub files: BTreeMap<String, String>,
}

/// Whether a pendant's files still match its manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seal {
    /// Every hash in the manifest matches the files, and `export.nt` is canonical.
    Intact,
    /// The files were changed after the manifest was written (a hand edit, a
    /// git merge). The reason says which check failed. The data may still be
    /// good; [`read`] validates it on its own terms.
    Broken(String),
}

/// A pendant read back into seeds' model.
#[derive(Debug, Clone)]
pub struct Ledger {
    /// The seeds and comments.
    pub snapshot: Snapshot,
    /// Whether the manifest still vouches for the data.
    pub seal: Seal,
    /// `sha256:<hex>` of the exact `export.nt` bytes.
    pub export_hash: String,
}

/// `sha256:<hex>` of some bytes, the form quipu's manifests use.
pub fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

impl Pendant {
    /// The ledger text, if present.
    pub fn export_nt(&self) -> Option<&str> {
        self.files.get(EXPORT_NT).map(String::as_str)
    }

    /// `sha256:<hex>` of `export.nt`, if present.
    pub fn export_hash(&self) -> Option<String> {
        self.export_nt().map(|t| sha256(t.as_bytes()))
    }
}

/// Export the project graph of `b` as a pendant.
pub fn export(b: &QuipuBackend) -> Result<Pendant> {
    let opts = ShareOptions {
        scope: ShareScope::Graph(b.graph_iri().to_string()),
        shapes: vec![SHAPES_NAME.to_string()],
        // quipu's outward scrub refuses to run without an identifier-policy
        // catalogue, which a seeds store does not carry. So the pendant is
        // stamped `internal`: quipu's own honest marker that no outward scrub
        // ran on it. It is bound into the share id, so it cannot be edited out.
        destination: ShareDestination::Internal,
        ..ShareOptions::default()
    };
    let payload = quipu::share::share_payload(b.store(), &opts, usize::MAX)?;
    Ok(Pendant {
        files: payload.files,
    })
}

/// Check a pendant's files against its manifest.
pub fn seal(p: &Pendant) -> Result<Seal> {
    let nt = p.export_nt().ok_or_else(|| {
        SdError::refused(format!(
            "the pendant has no {EXPORT_NT}; it is not a seeds ledger"
        ))
    })?;
    let Some(manifest_text) = p.files.get(MANIFEST_JSON) else {
        return Ok(Seal::Broken(format!("{MANIFEST_JSON} is missing")));
    };
    let manifest: ShareManifest = match serde_json::from_str(manifest_text) {
        Ok(m) => m,
        Err(e) => return Ok(Seal::Broken(format!("{MANIFEST_JSON} does not parse: {e}"))),
    };
    if manifest.files.graph != EXPORT_NT || manifest.files.shapes != SHAPES_TTL {
        return Ok(Seal::Broken(format!(
            "{MANIFEST_JSON} names payload files seeds does not use"
        )));
    }
    let canonical = match quipu::share::canonicalize_ntriples(nt.as_bytes()) {
        Ok(c) => c,
        Err(e) => {
            return Ok(Seal::Broken(format!(
                "{EXPORT_NT} is not valid N-Triples: {e}"
            )))
        }
    };
    if canonical != nt.as_bytes() {
        return Ok(Seal::Broken(format!(
            "{EXPORT_NT} is not in canonical order (edited or merged outside sd)"
        )));
    }
    if sha256(nt.as_bytes()) != manifest.graph_hash {
        return Ok(Seal::Broken(format!(
            "{EXPORT_NT} does not match the hash in {MANIFEST_JSON}"
        )));
    }
    let shapes = p.files.get(SHAPES_TTL).map(String::as_str).unwrap_or("");
    if sha256(shapes.as_bytes()) != manifest.shapes_hash {
        return Ok(Seal::Broken(format!(
            "{SHAPES_TTL} does not match the hash in {MANIFEST_JSON}"
        )));
    }
    let mut value =
        serde_json::to_value(&manifest).map_err(|e| SdError::failed(format!("manifest: {e}")))?;
    if let Some(o) = value.as_object_mut() {
        o.remove("share_id");
        o.remove("attestation");
    }
    let bytes =
        serde_json::to_vec(&value).map_err(|e| SdError::failed(format!("manifest: {e}")))?;
    if sha256(&bytes) != manifest.share_id {
        return Ok(Seal::Broken(format!(
            "{MANIFEST_JSON} does not hash to its own share id"
        )));
    }
    Ok(Seal::Intact)
}

/// Read a pendant into seeds' model, validating the ledger itself (never just
/// the seal): a git merge can leave a sealed-looking pendant broken, or a
/// broken-seal pendant perfectly good. Every problem is reported; none is
/// resolved by picking a side.
pub fn read(p: &Pendant) -> Result<Ledger> {
    let nt = p.export_nt().ok_or_else(|| {
        SdError::refused(format!(
            "the pendant has no {EXPORT_NT}; it is not a seeds ledger"
        ))
    })?;
    let by_subject = parse_ntriples(nt)?;
    let seal = seal(p)?;
    let problems = validate::validate_ledger(nt, &by_subject);
    if !problems.is_empty() {
        return Err(SdError::conflict(format!(
            "the pendant's ledger has {} problem(s); nothing was imported. Fix {EXPORT_NT} \
             (for example, keep one side of a merge) and run sd again:\n  {}",
            problems.len(),
            problems.join("\n  ")
        )));
    }
    Ok(Ledger {
        snapshot: Snapshot::from_subjects(&by_subject),
        seal,
        export_hash: sha256(nt.as_bytes()),
    })
}

/// Parse N-Triples into facts grouped by subject. Accepts IRIs, plain and
/// language-tagged string literals, and typed literals (`xsd:integer` becomes
/// an integer; any other datatype keeps its lexical form). Blank nodes and
/// anything else are refused with the line number, so a leftover merge
/// marker is reported where it is.
pub fn parse_ntriples(text: &str) -> Result<BTreeMap<String, Vec<Fact>>> {
    let mut out: BTreeMap<String, Vec<Fact>> = BTreeMap::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let err =
            |why: &str| SdError::refused(format!("{EXPORT_NT} line {}: {why}: {line}", n + 1));
        let mut rest = line;
        let s = take_iri(&mut rest).ok_or_else(|| err("expected a subject IRI"))?;
        let p = take_iri(&mut rest).ok_or_else(|| err("expected a predicate IRI"))?;
        rest = rest.trim_start();
        let o = if rest.starts_with('<') {
            Obj::Iri(take_iri(&mut rest).ok_or_else(|| err("bad object IRI"))?)
        } else if rest.starts_with('"') {
            take_literal(&mut rest).ok_or_else(|| err("bad literal"))?
        } else {
            return Err(err("expected an IRI or a literal object"));
        };
        if rest.trim() != "." {
            return Err(err("expected ' .' at the end of the triple"));
        }
        out.entry(s).or_default().push((p, o));
    }
    Ok(out)
}

fn take_iri(rest: &mut &str) -> Option<String> {
    let t = rest.trim_start();
    let t = t.strip_prefix('<')?;
    let end = t.find('>')?;
    let iri = unescape(&t[..end])?;
    *rest = &t[end + 1..];
    Some(iri)
}

fn take_literal(rest: &mut &str) -> Option<Obj> {
    let t = rest.trim_start().strip_prefix('"')?;
    let mut end = None;
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => {
                end = Some(i);
                break;
            }
            _ => i += 1,
        }
    }
    let end = end?;
    let lexical = unescape(&t[..end])?;
    let mut after = &t[end + 1..];
    if let Some(dt) = after.strip_prefix("^^") {
        let mut r = dt;
        let datatype = take_iri(&mut r)?;
        *rest = r;
        if datatype == "http://www.w3.org/2001/XMLSchema#integer" {
            return lexical.parse::<i64>().ok().map(Obj::Int);
        }
        return Some(Obj::Str(lexical));
    }
    if let Some(lang) = after.strip_prefix('@') {
        let len = lang
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(lang.len());
        after = &lang[len..];
    }
    *rest = after;
    Some(Obj::Str(lexical))
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            't' => out.push('\t'),
            'b' => out.push('\u{8}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\u{c}'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            '\\' => out.push('\\'),
            'u' => {
                let h: String = chars.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            'U' => {
                let h: String = chars.by_ref().take(8).collect();
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_seeds_writes() {
        let nt = "<https://seeds.local/item/sd-a> <urn:p> \"a \\\"q\\\"\\nline\" .\n\
                  <https://seeds.local/item/sd-a> <urn:n> \"3\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n\
                  <https://seeds.local/item/sd-a> <urn:r> <https://seeds.local/item/sd-b> .\n";
        let m = parse_ntriples(nt).unwrap();
        let f = &m["https://seeds.local/item/sd-a"];
        assert!(f.contains(&("urn:p".into(), Obj::Str("a \"q\"\nline".into()))));
        assert!(f.contains(&("urn:n".into(), Obj::Int(3))));
        assert!(f.contains(&(
            "urn:r".into(),
            Obj::Iri("https://seeds.local/item/sd-b".into())
        )));
    }

    #[test]
    fn a_merge_marker_is_refused_with_its_line_number() {
        let nt = "<urn:s> <urn:p> \"x\" .\n<<<<<<< HEAD\n";
        let e = parse_ntriples(nt).unwrap_err();
        assert!(e.message.contains("line 2"), "{}", e.message);
    }
}

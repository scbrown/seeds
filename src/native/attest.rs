//! Signed writes to a quipu server (aegis-bys8d1): a write carries an
//! `x-quipu-attestation` header signed with a key registered once, out of band,
//! instead of a bearer token. Nothing reusable crosses the wire: each header
//! binds one request (method, path, content type, body hash) and one nonce.
//!
//! The byte format is quipu's `quipu-write-v1`, pinned by the test vector quipu
//! publishes (`tests/vectors/write-attestation-v1.json`, copied here). seeds does
//! not depend on quipu for this; the vector is the contract.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest, Sha256};

use crate::error::{ErrorKind, Result, SdError};

pub const HEADER: &str = "x-quipu-attestation";
pub const VERSION: &str = "quipu-write-v1";

/// The paths quipu accepts a signed write on. Anything else still needs a bearer.
pub const SIGNED_PATHS: [&str; 4] = ["/knot", "/update", "/episode", "/graph/create"];

/// A registered signing identity: the private key and the binding it was
/// registered under (`quipu attest register --session S --introducer I`).
pub struct Signer {
    key: SigningKey,
    pub session: String,
    pub introducer: String,
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Lowercase hex SHA-256 of `bytes`, with no prefix.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn key_error(message: String) -> SdError {
    SdError::new(ErrorKind::Config, message)
}

/// Whether a field is safe in the newline-delimited canonical message.
fn clean(field: &str) -> bool {
    !field.chars().any(char::is_control)
}

impl Signer {
    pub fn from_seed(seed: [u8; 32], session: &str, introducer: &str) -> Result<Self> {
        if session.is_empty() || !clean(session) || !clean(introducer) {
            return Err(key_error(
                "[quipu] signing session and introducer must be non-empty and contain no \
                 control characters"
                    .into(),
            ));
        }
        Ok(Self {
            key: SigningKey::from_bytes(&seed),
            session: session.to_string(),
            introducer: introducer.to_string(),
        })
    }

    /// Load the key file: 64 hex characters (the Ed25519 seed), readable by the
    /// owner only.
    pub fn load(path: &Path, session: &str, introducer: &str) -> Result<Self> {
        check_private(path)?;
        let text = std::fs::read_to_string(path)
            .map_err(|e| key_error(format!("reading signing key {}: {e}", path.display())))?;
        let seed: [u8; 32] = unhex(text.trim())
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| {
                key_error(format!(
                    "{} is not a signing key (expected 64 hex characters)",
                    path.display()
                ))
            })?;
        Self::from_seed(seed, session, introducer)
    }

    /// Lowercase hex of the raw 32-byte public key.
    pub fn public_key_hex(&self) -> String {
        hex(self.key.verifying_key().as_bytes())
    }

    /// `sha256:` + hex SHA-256 of the raw public key (quipu's key_id has the prefix).
    pub fn key_id(&self) -> String {
        format!("sha256:{}", sha256_hex(self.key.verifying_key().as_bytes()))
    }

    /// The exact bytes signed for one write.
    pub fn canonical_message(
        &self,
        issued_at: u64,
        nonce: &str,
        method: &str,
        path: &str,
        content_type: &str,
        body_sha256: &str,
    ) -> String {
        format!(
            "{VERSION}\nkey_id={}\nsession={}\nintroducer={}\nissued_at={issued_at}\nnonce={nonce}\n\
             method={method}\npath={path}\ncontent_type={content_type}\nbody_sha256={body_sha256}\n",
            self.key_id(),
            self.session,
            self.introducer,
        )
    }

    /// The header value for one write: base64url (unpadded) of the envelope JSON.
    pub fn header(
        &self,
        issued_at: u64,
        nonce: &str,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<String> {
        if path.contains('?') || ![method, path, content_type].iter().all(|f| clean(f)) {
            return Err(SdError::failed(format!(
                "cannot sign {method} {path}: a signed write has no query string or control \
                 characters"
            )));
        }
        let message = self.canonical_message(
            issued_at,
            nonce,
            method,
            path,
            content_type,
            &sha256_hex(body),
        );
        let signature = hex(&self.key.sign(message.as_bytes()).to_bytes());
        // Field order matches quipu's envelope; the server parses JSON, so order
        // is not load-bearing, but it keeps the vector byte-identical.
        let envelope = format!(
            "{{\"version\":{},\"key_id\":{},\"session\":{},\"introducer\":{},\
             \"issued_at_epoch\":{issued_at},\"nonce\":{},\"signature\":{}}}",
            json_str(VERSION),
            json_str(&self.key_id()),
            json_str(&self.session),
            json_str(&self.introducer),
            json_str(nonce),
            json_str(&signature),
        );
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope))
    }

    /// A header for a write sent now, with a fresh nonce.
    pub fn header_now(
        &self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| SdError::failed(format!("system clock before 1970: {e}")))?
            .as_secs();
        self.header(now, &fresh_nonce()?, method, path, content_type, body)
    }

    /// The command the introducer runs, once, on the quipu host.
    pub fn register_command(&self, agent: &str, issued_at: u64, expires_at: u64) -> String {
        format!(
            "quipu attest register --agent {agent} --session {} --public-key {} \
             --introducer {} --issued-at {issued_at} --expires-at {expires_at} --allow-write",
            self.session,
            self.public_key_hex(),
            self.introducer
        )
    }
}

fn json_str(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

/// 128 random bits as 32 lowercase hex characters.
fn fresh_nonce() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b)
        .map_err(|e| SdError::failed(format!("no randomness for a nonce: {e}")))?;
    Ok(hex(&b))
}

/// Refuse a key file anyone but its owner can read.
fn check_private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let meta = std::fs::metadata(path)
            .map_err(|e| key_error(format!("signing key {}: {e}", path.display())))?;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(key_error(format!(
                "signing key {} is readable by others; run: chmod 600 {}",
                path.display(),
                path.display()
            )));
        }
    }
    Ok(())
}

/// Generate a new key file at `path` (owner-only), refusing to overwrite one.
pub fn generate(path: &Path) -> Result<PathBuf> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed)
        .map_err(|e| SdError::failed(format!("no randomness for a key: {e}")))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| key_error(format!("creating {}: {e}", dir.display())))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|e| {
        key_error(if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "{} already exists; seeds never overwrites a key",
                path.display()
            )
        } else {
            format!("creating {}: {e}", path.display())
        })
    })?;
    std::io::Write::write_all(&mut f, format!("{}\n", hex(&seed)).as_bytes())
        .map_err(|e| key_error(format!("writing {}: {e}", path.display())))?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTOR: &str = include_str!("../../tests/vectors/write-attestation-v1.json");

    #[test]
    fn reproduces_quipus_published_vector_byte_for_byte() {
        let v: serde_json::Value = serde_json::from_str(VECTOR).unwrap();
        let (i, d) = (&v["inputs"], &v["derived"]);
        let s = |x: &serde_json::Value| x.as_str().unwrap().to_string();
        let seed: [u8; 32] = unhex(&s(&i["ed25519_seed_hex"]))
            .unwrap()
            .try_into()
            .unwrap();
        let signer = Signer::from_seed(seed, &s(&i["session"]), &s(&i["introducer"])).unwrap();
        let body = s(&i["body"]);
        let issued = i["issued_at_epoch"].as_u64().unwrap();
        let (method, path, ct, nonce) = (
            s(&i["method"]),
            s(&i["path"]),
            s(&i["content_type"]),
            s(&i["nonce"]),
        );

        assert_eq!(signer.public_key_hex(), s(&d["public_key_hex"]));
        assert_eq!(signer.key_id(), s(&d["key_id"]));
        assert_eq!(sha256_hex(body.as_bytes()), s(&d["body_sha256"]));
        assert_eq!(
            signer.canonical_message(issued, &nonce, &method, &path, &ct, &s(&d["body_sha256"])),
            s(&d["canonical_message"])
        );
        let header = signer
            .header(issued, &nonce, &method, &path, &ct, body.as_bytes())
            .unwrap();
        assert_eq!(header, s(&d["header_value"]));
        let envelope = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&header)
            .unwrap();
        assert_eq!(String::from_utf8(envelope).unwrap(), s(&d["envelope_json"]));
    }

    #[test]
    fn a_query_string_or_control_character_is_refused() {
        let signer = Signer::from_seed([7; 32], "s", "i").unwrap();
        assert!(signer
            .header(1, "00", "POST", "/update?x=1", "a/b", b"")
            .is_err());
        assert!(signer
            .header(1, "00", "POST", "/update", "a/b\n", b"")
            .is_err());
        assert!(Signer::from_seed([7; 32], "s\nintroducer=x", "i").is_err());
    }

    #[test]
    fn fresh_headers_never_reuse_a_nonce() {
        let signer = Signer::from_seed([7; 32], "s", "i").unwrap();
        let a = signer.header_now("POST", "/update", "a/b", b"x").unwrap();
        let b = signer.header_now("POST", "/update", "a/b", b"x").unwrap();
        assert_ne!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn a_generated_key_is_owner_only_never_overwritten_and_loads() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("sd-attest-{}", std::process::id()));
        let path = dir.join("keys/k.key");
        let _ = std::fs::remove_dir_all(&dir);
        generate(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(generate(&path).is_err(), "must never overwrite a key");
        assert!(Signer::load(&path, "s", "i").is_ok());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            Signer::load(&path, "s", "i").is_err(),
            "a world-readable key is refused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

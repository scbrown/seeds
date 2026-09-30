//! Opening the local store file, and the write lock that makes concurrent
//! `sd` processes safe.
//!
//! Every write takes an exclusive lock on `<store>.lock` for the whole verb:
//! read the snapshot, check the revisions, commit. A second `sd` writing the
//! same store waits for the first, then reads the first's result, so no update
//! is lost.
//!
//! The lock is load-bearing, and the tests prove it: with it removed,
//! `tests/cli.rs` sees two claimers both win and two writers both stamp the
//! same revision. The backend's revision check is atomic only within ONE store
//! handle, because quipu's library API offers no way to hold SQLite's write
//! lock across a read and a `transact`. So a program that writes the same
//! file WITHOUT this lock can still race `sd`. Closing that needs an
//! expected-value precondition on quipu's write path (see
//! `docs/book/src/storage.md`).
//!
//! Reads take no lock. quipu keeps the store in SQLite WAL mode, so a reader
//! sees the last committed transaction while a write is in flight.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use quipu::store::Store;

use crate::error::{Result, SdError};
use crate::quipu_backend::QuipuBackend;

/// A writable store, holding the write lock until dropped.
pub struct WriteHandle {
    /// The backend.
    pub backend: QuipuBackend,
    _lock: File,
}

fn lock_path(store: &Path) -> PathBuf {
    let mut name = store.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    store.with_file_name(name)
}

/// Open (creating if needed) the store at `path` for writing, under the lock.
pub fn open_for_write(path: &Path, graph: &str) -> Result<WriteHandle> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)
            .map_err(|e| SdError::failed(format!("cannot create {}: {e}", dir.display())))?;
    }
    let lp = lock_path(path);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lp)
        .map_err(|e| SdError::failed(format!("cannot open lock {}: {e}", lp.display())))?;
    lock.lock()
        .map_err(|e| SdError::failed(format!("cannot lock {}: {e}", lp.display())))?;
    let store = Store::open(&path.to_string_lossy())
        .map_err(|e| SdError::failed(format!("cannot open store {}: {e}", path.display())))?;
    Ok(WriteHandle {
        backend: QuipuBackend::writable(store, graph)?,
        _lock: lock,
    })
}

/// Open the store at `path` for reading. `Ok(None)` when it does not exist
/// yet (nothing has been written).
pub fn open_for_read(path: &Path, graph: &str) -> Result<Option<QuipuBackend>> {
    if !path.exists() {
        return Ok(None);
    }
    let store = Store::open_read_only(&path.to_string_lossy())
        .map_err(|e| SdError::failed(format!("cannot open store {}: {e}", path.display())))?;
    Ok(Some(QuipuBackend::read_only(store, graph)))
}

/// Try to reach a quipu server, so a configured URL that is down is reported
/// as down. Returns the error text when it cannot be reached.
pub fn probe(url: &str) -> std::result::Result<(), String> {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;
    let (default_port, rest) = if let Some(r) = url.strip_prefix("https://") {
        (443, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (80, r)
    } else {
        return Err(format!("not an http(s) URL: {url}"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host_port = if authority
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
        && !authority.ends_with(']')
    {
        authority.to_string()
    } else {
        format!("{authority}:{default_port}")
    };
    let addrs: Vec<_> = host_port
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host_port}: {e}"))?
        .collect();
    let mut last = format!("no addresses for {host_port}");
    for a in addrs {
        match TcpStream::connect_timeout(&a, Duration::from_secs(3)) {
            Ok(_) => return Ok(()),
            Err(e) => last = format!("{a}: {e}"),
        }
    }
    Err(last)
}

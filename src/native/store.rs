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

/// Full-text board reads share one user-wide lock across projects and graphs.
/// Keep the inode after release: unlinking a locked file would let two readers
/// acquire locks on different inodes at the same path.
pub(super) fn lock_full_search() -> Result<File> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| SdError::refused("full search needs HOME for its host-wide lock"))?;
    let dir = PathBuf::from(home).join(".config/seeds");
    fs::create_dir_all(&dir)
        .map_err(|e| SdError::failed(format!("cannot create full-search lock directory: {e}")))?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(dir.join("full-search.lock"))
        .map_err(|e| SdError::failed(format!("cannot open full-search lock: {e}")))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(SdError::refused("full search is already running on this host; retry later or search titles without --full")),
        Err(std::fs::TryLockError::Error(e)) => Err(SdError::failed(format!("cannot lock full search: {e}"))),
    }
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

// ---------------------------------------------------------------- pendants on disk

use crate::backend::Ctx;
use crate::pendant::{self, Pendant, Seal};
use crate::sync;

/// Read a pendant directory. `Ok(None)` when it (or its `export.nt`) does
/// not exist yet.
pub fn read_pendant_dir(dir: &Path) -> Result<Option<Pendant>> {
    if !dir.join(pendant::EXPORT_NT).is_file() {
        return Ok(None);
    }
    let mut p = Pendant::default();
    for name in pendant::FILES {
        match fs::read_to_string(dir.join(name)) {
            Ok(text) => {
                p.files.insert(name.to_string(), text);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(SdError::failed(format!(
                    "cannot read {}: {e}",
                    dir.join(name).display()
                )))
            }
        }
    }
    Ok(Some(p))
}

/// The `.gitattributes` a pendant directory carries. The manifests change on
/// every export and record the producing store, so two branches always touch
/// them; `merge=union` lets git merge them without a conflict. The result is
/// not a valid manifest, which is fine: seeds validates `export.nt` on its
/// own terms, then rewrites the manifest.
pub const PENDANT_GITATTRIBUTES: &str =
    "# written by sd: see https://github.com/scbrown/seeds (docs/book/src/storage-modes.md)\n\
     # export.nt merges field by field once the driver is registered:\n\
     #   git config merge.seeds.driver \"sd merge-driver %O %A %B\"\n\
     # Without it, git falls back to a line merge and sd validates the result.\n\
     export.nt merge=seeds\n\
     manifest.json merge=union\n\
     manifest.ttl merge=union\n";

/// Write a pendant into `dir`, file by file, each atomically (write, then
/// rename). Returns whether any file changed.
pub fn write_pendant_dir(dir: &Path, p: &Pendant) -> Result<bool> {
    fs::create_dir_all(dir)
        .map_err(|e| SdError::failed(format!("cannot create {}: {e}", dir.display())))?;
    let mut changed = false;
    let mut files: Vec<(&str, &str)> = p
        .files
        .iter()
        .filter(|(n, _)| pendant::FILES.contains(&n.as_str()))
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let attrs = dir.join(".gitattributes");
    if !attrs.exists() {
        files.push((".gitattributes", PENDANT_GITATTRIBUTES));
    }
    for (name, text) in files {
        let path = dir.join(name);
        if fs::read_to_string(&path).ok().as_deref() == Some(text) {
            continue;
        }
        let tmp = dir.join(format!(".{name}.tmp"));
        fs::write(&tmp, text)
            .and_then(|()| fs::rename(&tmp, &path))
            .map_err(|e| SdError::failed(format!("cannot write {}: {e}", path.display())))?;
        changed = true;
    }
    Ok(changed)
}

fn sidecar(store: &Path, suffix: &str) -> PathBuf {
    let mut name = store.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    store.with_file_name(name)
}

/// The file recording which `export.nt` the working store last matched.
pub fn marker_path(store: &Path) -> PathBuf {
    sidecar(store, ".pendant")
}

/// The file holding the ledger as of the last `sd sync` with ONE remote and
/// graph (the merge base). Keyed by the pair, because a base read against a
/// different remote turns everything that remote lacks into deletions.
pub fn sync_base_path(store: &Path, remote: &str, graph: &str) -> PathBuf {
    let key = pendant::sha256(format!("{}\n{graph}", normalize_url(remote)).as_bytes());
    let short = &key["sha256:".len().."sha256:".len() + 16];
    let mut name = store.file_name().unwrap_or_default().to_os_string();
    name.push(".sync");
    store.with_file_name(name).join(format!("{short}.nt"))
}

/// A remote URL in the form used for keys: lower-case scheme and host, no
/// trailing slash.
pub fn normalize_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    match u.split_once("://") {
        Some((scheme, rest)) => {
            let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
            let mut out = format!(
                "{}://{}",
                scheme.to_ascii_lowercase(),
                host.to_ascii_lowercase()
            );
            if !path.is_empty() {
                out.push('/');
                out.push_str(path);
            }
            out
        }
        None => u.to_string(),
    }
}

/// Write a sync base, with a note of which remote and graph it belongs to.
pub fn write_sync_base(path: &Path, remote: &str, graph: &str, nt: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|e| SdError::failed(format!("cannot create {}: {e}", dir.display())))?;
    }
    let tmp = path.with_extension("nt.tmp");
    fs::write(&tmp, nt)
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| SdError::failed(format!("cannot write the sync base: {e}")))?;
    let _ = fs::write(
        path.with_extension("remote"),
        format!("{}\n{graph}\n", normalize_url(remote)),
    );
    Ok(())
}

fn read_marker(store: &Path) -> Option<String> {
    fs::read_to_string(marker_path(store))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn write_marker(store: &Path, hash: &str) -> Result<()> {
    fs::write(marker_path(store), format!("{hash}\n"))
        .map_err(|e| SdError::failed(format!("cannot write the pendant marker: {e}")))
}

/// When the store lives in a `.seeds/` directory, keep its working files out
/// of git (the pendant is what gets committed). Never overwrites.
pub fn ensure_gitignore(store: &Path) {
    let Some(dir) = store.parent() else { return };
    if dir.file_name().and_then(|n| n.to_str()) != Some(crate::native::config::PROJECT_DIR) {
        return;
    }
    let gi = dir.join(".gitignore");
    if gi.exists() {
        return;
    }
    let name = store
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("seeds.db");
    let _ = fs::write(
        gi,
        format!(
            "# written by sd: the working store and its sidecars stay local;\n\
             # the pendant directory is the part to commit.\n\
             {name}\n{name}-*\n{name}.*\n"
        ),
    );
}

/// Export the working store to the pendant directory and record the match.
/// Returns whether the pendant files changed.
///
/// When the pendant on disk already holds this exact ledger under an intact
/// seal, it is left alone: the manifest names the producing store, so
/// rewriting it would dirty a fresh clone's working tree for no change in data.
pub fn export_to_pendant(b: &QuipuBackend, store: &Path, dir: &Path) -> Result<bool> {
    let p = pendant::export(b)?;
    let same_data = |d: &Pendant| {
        d.export_nt() == p.export_nt()
            && d.files.get(pendant::SHAPES_TTL) == p.files.get(pendant::SHAPES_TTL)
    };
    let changed = match read_pendant_dir(dir)? {
        Some(d) if same_data(&d) && pendant::seal(&d)? == Seal::Intact => false,
        _ => write_pendant_dir(dir, &p)?,
    };
    if let Some(h) = p.export_hash() {
        write_marker(store, &h)?;
    }
    ensure_gitignore(store);
    Ok(changed)
}

/// Bring the working store and the repo-local pendant into agreement before a
/// command runs (mode 1). The marker file names the `export.nt` both last
/// agreed on, which makes this a three-way decision:
///
/// | working store | pendant | action |
/// |---|---|---|
/// | = pendant | | nothing (re-seal the manifest if it was edited) |
/// | unchanged since the marker (or empty) | changed (a pull, a checkout, a merge) | load the pendant into the store |
/// | changed | unchanged since the marker | export the store to the pendant |
/// | changed | changed | refuse, and say how to choose |
///
/// Returns notes for stderr.
pub fn hydrate(b: &mut QuipuBackend, store: &Path, dir: &Path, ctx: &Ctx) -> Result<Vec<String>> {
    use crate::backend::Backend;
    let mut notes = Vec::new();
    let mine = pendant::export(b)?;
    let mine_hash = mine.export_hash().unwrap_or_default();
    let Some(disk) = read_pendant_dir(dir)? else {
        let snap = b.snapshot(None)?.shared();
        if !snap.seeds.is_empty() || !snap.comments.is_empty() {
            export_to_pendant(b, store, dir)?;
            notes.push(format!("wrote the pendant at {}", dir.display()));
        }
        return Ok(notes);
    };
    let disk_hash = disk.export_hash().unwrap_or_default();
    let marker = read_marker(store);
    if disk_hash == mine_hash {
        if pendant::seal(&disk)? != Seal::Intact {
            write_pendant_dir(dir, &mine)?;
            notes.push(format!(
                "resealed the pendant manifest at {}",
                dir.display()
            ));
        }
        if marker.as_deref() != Some(mine_hash.as_str()) {
            write_marker(store, &mine_hash)?;
        }
        return Ok(notes);
    }
    let snap = b.snapshot(None)?.shared();
    let store_empty = snap.seeds.is_empty() && snap.comments.is_empty();
    let store_clean = store_empty || marker.as_deref() == Some(mine_hash.as_str());
    let disk_clean = marker.as_deref() == Some(disk_hash.as_str());
    if store_clean {
        let ledger = pendant::read(&disk)?;
        let r = sync::import(b, ctx, &ledger.snapshot, None, true)?;
        export_to_pendant(b, store, dir)?;
        notes.push(format!(
            "loaded the pendant at {} ({} created, {} updated, {} removed{})",
            dir.display(),
            r.created.len(),
            r.updated.len(),
            r.removed.len(),
            match ledger.seal {
                Seal::Intact => String::new(),
                Seal::Broken(why) =>
                    format!("; it had been edited outside sd ({why}), and was resealed"),
            }
        ));
        return Ok(notes);
    }
    if disk_clean {
        export_to_pendant(b, store, dir)?;
        notes.push(format!(
            "the pendant at {} was behind the working store; re-exported",
            dir.display()
        ));
        return Ok(notes);
    }
    Err(SdError::conflict(format!(
        "both the working store ({}) and the pendant ({}) changed since they last matched; \
         nothing was changed. Choose one: `sd import {} --prefer pendant` (or --prefer store, \
         or --replace) merges the pendant into the store, or `sd export` overwrites the \
         pendant with the store.",
        store.display(),
        dir.display(),
        dir.display()
    )))
}

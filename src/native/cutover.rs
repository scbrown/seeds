//! Filesystem orchestration for the beads bridge. Never opens a br database.
use super::{
    cli::{Cli, CutoverArgs},
    config::{Location, Resolved},
    store, Outcome,
};
use crate::{
    backend::{Backend, Ctx},
    beads::{self, Records},
    error::{Result, SdError},
    model::Snapshot,
    quipu_backend::QuipuBackend,
    sync,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map_err(|e| SdError::failed(format!("cannot read {}: {e}", path.display())))
}
fn atomic(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("cutover-{}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| SdError::failed(format!("cannot stage {}: {e}", tmp.display())))?;
    let result = (|| {
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e: std::io::Error| {
        SdError::failed(format!("cannot publish {}: {e}", path.display()))
    })
}
fn outcome(v: Value, code: i32) -> Outcome {
    Outcome {
        code,
        stdout: format!("{}\n", serde_json::to_string_pretty(&v).unwrap()),
        stderr: String::new(),
    }
}
fn snapshot(path: &Path, graph: &str) -> Result<Snapshot> {
    match store::open_for_read(path, graph)? {
        Some(b) => b.snapshot(None),
        None => Ok(Snapshot::default()),
    }
}
fn prepared(current: &Snapshot, records: &Records) -> Result<Snapshot> {
    let mut desired = beads::decode(records)?;
    let exported = beads::encode(current)?;
    for (id, s) in &mut desired.seeds {
        if let Some(old) = current.seeds.get(id) {
            if exported.get(id) == records.get(id) {
                *s = old.clone();
            } else {
                s.revision = old.revision + 1; // Keep unrelated facts written by newer tools.
                for fact in &old.extra {
                    if fact.0 != crate::vocab::seeds("beadsJson") {
                        s.extra.insert(fact.clone());
                    }
                }
            }
        }
    }
    // Stable comment slots: decoding uses source array order, which export preserves.
    let previous: std::collections::BTreeMap<_, _> = current
        .comments
        .iter()
        .map(|c| ((&c.seed, c.index), c))
        .collect();
    for c in &mut desired.comments {
        if let Some(old) = previous.get(&(&c.seed, c.index)) {
            c.extra = old.extra.clone();
        }
    }
    Ok(desired)
}

fn sibling(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
fn lock(path: &Path) -> Result<fs::File> {
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| SdError::failed(format!("cannot lock {}: {e}", path.display())))?;
    f.lock().map_err(|e| SdError::failed(e.to_string()))?;
    Ok(f)
}
fn absolute(path: &Path) -> Result<std::path::PathBuf> {
    if path.is_symlink() {
        return Err(SdError::refused("cutover paths must not be symlinks"));
    }
    let abs = std::path::absolute(path).map_err(|e| SdError::failed(e.to_string()))?;
    let parent = abs
        .parent()
        .ok_or_else(|| SdError::usage("file path needs a parent"))?
        .canonicalize()
        .map_err(|e| SdError::failed(e.to_string()))?;
    Ok(parent.join(
        abs.file_name()
            .ok_or_else(|| SdError::usage("expected file path"))?,
    ))
}

fn comment_ids(path: &Path, binding: &Value) -> Result<beads::CommentIds> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text)
                .map_err(|e| SdError::failed(format!("invalid comment identity map: {e}")))?;
            if value["format"] != "seeds-br-comment-ids-v1" || value["binding"] != *binding {
                return Err(SdError::refused("comment identity map binding differs"));
            }
            serde_json::from_value(value["ids"].clone())
                .map_err(|e| SdError::failed(format!("invalid comment identity map: {e}")))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(SdError::failed(format!(
            "cannot read comment identity map: {e}"
        ))),
    }
}

fn save_comment_ids(path: &Path, binding: &Value, ids: &beads::CommentIds) -> Result<()> {
    let text = json!({"format":"seeds-br-comment-ids-v1","binding":binding,"ids":ids}).to_string();
    if fs::read_to_string(path).ok().as_deref() != Some(&text) {
        atomic(path, &text)?;
    }
    Ok(())
}

pub(super) fn run(cli: &Cli, cfg: &Resolved, ctx: &Ctx, a: &CutoverArgs) -> Result<Outcome> {
    // Requiring explicit destination prevents a cwd or user config from turning
    // a copied-board rehearsal into a production import.
    if cli.store.is_none()
        || cli.graph.is_none()
        || cfg.pendant.is_some()
        || !matches!(cfg.location, Location::Store(_))
    {
        return Err(SdError::usage("cutover requires explicit --store and --graph, with no pendant or remote; use a copied board"));
    }
    if cli.at.is_some() {
        return Err(SdError::usage("cutover does not accept --at"));
    }
    let Location::Store(path) = &cfg.location else {
        unreachable!()
    };
    let file = absolute(Path::new(&a.file))?;
    let store_path = absolute(path)?;
    let graph = cfg.graph();
    let graph_hash = format!("{:x}", Sha256::digest(graph.as_bytes()));
    let ids_path = sibling(&store_path, &format!(".cutover-{graph_hash}.comments.json"));
    let ids_lock_path = sibling(&ids_path, ".lock");
    let ids_binding = json!({"store":store_path,"graph":graph});
    let base_owned = a.base.as_deref().map(Path::new).map(absolute).transpose()?;
    let base_path = base_owned.as_deref();
    let mut paths = vec![
        store_path.clone(),
        file.clone(),
        sibling(&file, ".cutover.lock"),
        ids_path.clone(),
        ids_lock_path.clone(),
    ];
    paths.extend(
        ["-wal", "-shm", ".lock"]
            .iter()
            .map(|suffix| sibling(&store_path, suffix)),
    );
    if let Some(p) = base_path {
        paths.extend([p.to_path_buf(), sibling(p, ".lock"), sibling(p, ".pending")]);
    }
    if paths
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != paths.len()
    {
        return Err(SdError::usage(
            "store, JSONL, cursor and all sidecars must be distinct files",
        ));
    }
    if paths.iter().any(|p| p.is_symlink()) {
        return Err(SdError::refused("cutover sidecars must not be symlinks"));
    }
    let _ids_lock = if !a.dry_run && a.operation != "verify" {
        Some(lock(&ids_lock_path)?)
    } else {
        None
    };
    let _cursor_lock = if !a.dry_run && a.operation == "sync" {
        base_path.map(|p| lock(&sibling(p, ".lock"))).transpose()?
    } else {
        None
    };
    let _peer_lock = if !a.dry_run && a.operation != "verify" {
        Some(lock(&sibling(&file, ".cutover.lock"))?)
    } else {
        None
    };
    if a.operation == "verify" {
        let input = beads::parse(&read(&file)?)?;
        let decoded = beads::decode(&input)?;
        let mut b = QuipuBackend::in_memory(graph)?;
        let (batch, _) = sync::plan(&Snapshot::default(), &decoded, "beads-sync");
        if !batch.is_empty() {
            b.commit(&batch, ctx)?;
        }
        let output = beads::encode(&b.snapshot(None)?)?;
        let differences = beads::diff(&input, &output);
        return Ok(outcome(
            json!({"records":input.len(),"returned":output.len(),"losses":differences.len(),"differences":differences}),
            if differences.is_empty() { 0 } else { 1 },
        ));
    }
    let mut ids = comment_ids(&ids_path, &ids_binding)?;
    if a.operation == "export" {
        let records = beads::encode_mapped(&snapshot(path, graph)?, &mut ids)?;
        if !a.dry_run {
            // Reserve first: a failed output publication must not reuse an ID.
            save_comment_ids(&ids_path, &ids_binding, &ids)?;
            atomic(&file, &beads::render(&records))?;
        }
        return Ok(outcome(
            json!({"records":records.len(),"dry_run":a.dry_run,"file":file}),
            0,
        ));
    }
    let input_text = read(&file)?;
    let input = beads::parse(&input_text)?;
    // All writes in this process hold the normal sd store lock. Dry-run only
    // opens a read handle: it does not create even an empty database or lock.
    let mut handle = if a.dry_run {
        None
    } else {
        Some(store::open_for_write(path, graph)?)
    };
    let current = match &handle {
        Some(h) => h.backend.snapshot(None)?,
        None => snapshot(path, graph)?,
    };
    if current.seeds.values().any(|s| s.ephemeral) {
        return Err(SdError::refused(
            "cutover requires a ledger without ephemeral seeds",
        ));
    }
    let local = beads::encode_mapped(&current, &mut ids)?;
    let binding = json!({"store":store_path,"graph":graph,"file":file});

    let mut base = Records::new();
    if a.operation == "sync" {
        let p = base_path
            .ok_or_else(|| SdError::usage("cutover sync requires --base <cursor-file>"))?;
        match fs::read_to_string(p) {
            Ok(t) => {
                let v: Value = serde_json::from_str(&t)
                    .map_err(|e| SdError::failed(format!("invalid sync cursor: {e}")))?;
                if v["binding"] != binding {
                    return Err(SdError::refused(
                        "sync cursor belongs to a different store, graph or file",
                    ));
                }
                base = serde_json::from_value(v["records"].clone())
                    .map_err(|e| SdError::failed(format!("invalid sync cursor records: {e}")))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(SdError::failed(format!("cannot read sync cursor: {e}"))),
        }
    }
    let pending_path = base_path.map(|p| sibling(p, ".pending"));
    let pending = if a.operation == "sync" {
        match fs::read_to_string(pending_path.as_ref().unwrap()) {
            Ok(t) => Some(
                serde_json::from_str::<Value>(&t)
                    .map_err(|e| SdError::failed(format!("invalid recovery journal: {e}")))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(SdError::failed(format!(
                    "cannot read recovery journal: {e}"
                )))
            }
        }
    } else {
        None
    };
    let merged = if let Some(j) = &pending {
        if j["binding"] != binding {
            return Err(SdError::refused("recovery journal binding differs"));
        }
        let decode = |key: &str| {
            serde_json::from_value::<Records>(j[key].clone())
                .map_err(|e| SdError::failed(format!("invalid recovery {key}: {e}")))
        };
        let desired = decode("desired")?;
        if (local != decode("local")? && local != desired)
            || (input != decode("peer")? && input != desired)
        {
            return Err(SdError::conflict("unfinished sync has newer edits; preserve the journal and reconcile before retrying"));
        }
        desired
    } else if a.operation == "sync" {
        beads::merge(&base, &local, &input)?
    } else {
        // Import is additive and refuses changes to existing IDs. Updates
        // need the common base of cutover sync, never a silent prefer-side.
        let mut m = local.clone();
        for (id, v) in &input {
            if let Some(old) = m.get(id) {
                if old != v {
                    return Err(SdError::conflict(format!(
                        "{id}: import differs; use cutover sync with a common base"
                    )));
                }
            }
            m.insert(id.clone(), v.clone());
        }
        m
    };
    let desired = prepared(&current, &merged)?;
    let merged = beads::encode_mapped(&desired, &mut ids)?;
    let (batch, report) = sync::plan(&current, &desired, "beads-sync");
    let peer_diff = beads::diff(&input, &merged);
    let local_diff = beads::diff(&local, &merged);
    if !a.allow_deletes
        && (!report.removed.is_empty() || input.keys().any(|id| !merged.contains_key(id)))
    {
        return Err(SdError::refused(
            "sync would remove records; inspect both copies and explicitly pass --allow-deletes",
        ));
    }
    if a.dry_run {
        crate::validate::validate_batch(&batch, &mut |id| {
            Ok(current.seeds.get(id).map(|s| s.facts()))
        })?;
    }
    if !a.dry_run {
        // Detect a peer changing after the read. Cooperating JSONL producers
        // must serialize with this invocation; this is not a live br writer.
        if read(&file)? != input_text {
            return Err(SdError::conflict(
                "JSONL changed during sync; nothing written",
            ));
        }
        save_comment_ids(&ids_path, &ids_binding, &ids)?;
        if a.operation == "sync"
            && pending.is_none()
            && (!batch.is_empty() || !peer_diff.is_empty())
        {
            // Persist both pre-images and the intended common result before
            // either participant changes. A retry can finish only this plan.
            atomic(
                pending_path.as_ref().unwrap(),
                &json!({"binding":binding,"local":local,"peer":input,"desired":merged}).to_string(),
            )?;
        }
        if !batch.is_empty() {
            handle.as_mut().unwrap().backend.commit(&batch, ctx)?;
        }
        // Cursor is last. If publishing fails, a retry merges the same facts
        // against the previous cursor, so an interrupted write is replayable.
        if a.operation == "sync" {
            if !peer_diff.is_empty() {
                atomic(&file, &beads::render(&merged))?;
            }
            let state = json!({"binding":binding,"records":merged}).to_string();
            let p = base_path.unwrap();
            if fs::read_to_string(p).ok().as_deref() != Some(&state) {
                atomic(p, &state)?;
            }
            if pending_path.as_ref().unwrap().exists() {
                fs::remove_file(pending_path.as_ref().unwrap()).map_err(|e| {
                    SdError::failed(format!("cursor committed but recovery cleanup failed: {e}"))
                })?;
            }
        }
    }
    let counts = |diff: &[Value]| {
        let mut fields = std::collections::BTreeMap::<String, usize>::new();
        for d in diff {
            let field = d["field"]
                .as_str()
                .unwrap_or("$record")
                .trim_start_matches('/')
                .split('/')
                .next()
                .unwrap_or("$record");
            *fields.entry(field.into()).or_default() += 1;
        }
        fields
    };
    Ok(outcome(
        json!({"dry_run":a.dry_run,"created":report.created,"updated":report.updated,"removed":report.removed,
            "store_difference_count":local_diff.len(),"file_difference_count":peer_diff.len(),
            "store_fields":counts(&local_diff),"file_fields":counts(&peer_diff),
            "store_differences":local_diff.iter().take(100).collect::<Vec<_>>(),"file_differences":peer_diff.iter().take(100).collect::<Vec<_>>(),
            "differences_truncated":local_diff.len()>100 || peer_diff.len()>100,
            "wrote":!a.dry_run&&!batch.is_empty()}),
        0,
    ))
}

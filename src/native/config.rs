//! Where the graph lives: TOML configuration for the native CLI.
//!
//! Resolution, highest precedence first (each layer is consulted only when
//! every layer above it says nothing):
//!
//! 1. flags: `--store <path>` or `--quipu <url>` (and `--graph`)
//! 2. environment: `SEEDS_QUIPU_STORE` or `SEEDS_QUIPU_URL` (and
//!    `SEEDS_GRAPH`, `SEEDS_PREFIX`)
//! 3. the project file: the nearest `.seeds/config.toml`, walking up from the
//!    current directory like git does
//! 4. the user file: `$XDG_CONFIG_HOME/seeds/config.toml`, else
//!    `~/.config/seeds/config.toml`
//! 5. the default: a LOCAL store at `<project>/.seeds/seeds.db`, where
//!    `<project>` is the directory holding the nearest `.seeds/`, or the
//!    current directory when there is none. It is created on first write.
//!
//! The store location is ONE choice: the highest layer that names either a
//! `store` or a `url` decides it, and a single layer (file or environment)
//! that names both is refused. A configured `url` that cannot be reached is an
//! error; seeds never falls back to a local store, which would fork the ledger.
//!
//! This module lives in the native layer on purpose: the wasm core never reads
//! a file or an environment variable, it is handed a storage backend.
//!
//! ```toml
//! [quipu]
//! store = ".seeds/seeds.db"          # a local store file (the default), or
//! # url = "https://quipu.example.org" # a shared quipu server
//!
//! [project]
//! prefix = "sd"                      # id prefix for new seeds
//! # graph = "https://seeds.local/project/sd"  # the project's named graph
//!
//! [pendant]
//! # dir = ".seeds/pendant"           # mode 1: keep the ledger in the repo
//!
//! [sync]
//! # remote = "https://quipu.example.org"  # mode 3: what `sd sync` exchanges with
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{ErrorKind, Result, SdError};
use crate::vocab;

/// The project directory name and the config file inside it.
pub const PROJECT_DIR: &str = ".seeds";
/// The config file name, in `.seeds/` and in the user config dir.
pub const CONFIG_FILE: &str = "config.toml";
/// The default store file name, in `.seeds/`.
pub const DEFAULT_STORE_FILE: &str = "seeds.db";
/// The default id prefix.
pub const DEFAULT_PREFIX: &str = "sd";

/// One config file's contents.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// `[quipu]`: where the graph lives.
    #[serde(default)]
    pub quipu: QuipuSection,
    /// `[project]`: ids and the named graph.
    #[serde(default)]
    pub project: ProjectSection,
    /// `[pendant]`: a repo-local copy of the ledger (mode 1).
    #[serde(default)]
    pub pendant: PendantSection,
    /// `[sync]`: the remote `sd sync` exchanges with (mode 3).
    #[serde(default)]
    pub sync: SyncSection,
}

/// `[pendant]`.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendantSection {
    /// The pendant directory, e.g. `.seeds/pendant`. Relative paths resolve
    /// like `[quipu] store`.
    pub dir: Option<String>,
}

/// `[sync]`.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SyncSection {
    /// The quipu server `sd sync` exchanges the ledger with.
    pub remote: Option<String>,
}

/// `[quipu]`.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuipuSection {
    /// A local store file. Relative paths resolve against the directory that
    /// holds `.seeds/` (project file) or the user config directory (user file).
    pub store: Option<String>,
    /// A shared quipu server, `http://` or `https://`.
    pub url: Option<String>,
    /// A file holding the bearer token quipu wants for writes. USER-LEVEL
    /// config only: a project file that sets it is refused, because a cloned
    /// repository could otherwise send your token to a server of its choosing.
    pub token_file: Option<String>,
    /// Hosts the token may be sent to when the server URL came from a
    /// PROJECT file (user-level config only). A URL you set yourself (flag,
    /// environment, user file) is trusted without this.
    pub trusted_hosts: Option<Vec<String>>,
    /// Hosts the token may be sent to over plain `http://` (user-level config
    /// only). By default a token goes only over https or to localhost; this
    /// is the opt-in for a server on a trusted network that has no TLS.
    /// Matched exactly (host, or host:port), never by suffix.
    pub allow_plain_http_hosts: Option<Vec<String>>,
    /// A file holding this user's Ed25519 signing key (`sd key init` writes
    /// it). When set, writes quipu accepts signed carry an attestation instead
    /// of the bearer (aegis-bys8d1). USER-LEVEL config only, like token_file.
    pub signing_key_file: Option<String>,
    /// The session the key is registered under (`quipu attest register --session`).
    pub signing_session: Option<String>,
    /// The introducer that registered it (`quipu attest register --introducer`).
    pub signing_introducer: Option<String>,
}

/// `[project]`.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    /// The id prefix for new seeds.
    pub prefix: Option<String>,
    /// The project's named graph IRI.
    pub graph: Option<String>,
}

/// Where the graph lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A local quipu store file.
    Store(PathBuf),
    /// A shared quipu server.
    Url(String),
}

/// The resolved configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Where the graph lives.
    pub location: Location,
    /// Which layer decided the location, for messages and `--json` errors.
    pub location_source: String,
    /// The id prefix.
    pub prefix: String,
    /// The project's named graph IRI, once known: configured, or derived
    /// from the project id (see [`project_graph`]).
    pub graph: Option<String>,
    /// Where this project's id lives (`<project>/.seeds/project-id`).
    pub project_id_file: PathBuf,
    /// Whether the server URL in `location` came from the project file.
    pub location_from_project: bool,
    /// Whether `sync_remote` came from the project file.
    pub sync_remote_from_project: bool,
    /// Hosts the user allows the token to go to for project-chosen URLs.
    pub trusted_hosts: Vec<String>,
    /// Hosts the user allows the token to reach over plain http.
    pub allow_plain_http_hosts: Vec<String>,
    /// The repo-local pendant directory, when one is configured (mode 1).
    pub pendant: Option<PathBuf>,
    /// The remote `sd sync` exchanges with (mode 3).
    pub sync_remote: Option<String>,
    /// The bearer token for writes to a quipu server, if configured.
    pub token: Option<String>,
    /// A file to read the bearer token from, if configured.
    pub token_file: Option<PathBuf>,
    /// The signing key file, session and introducer, if configured.
    pub signing: Option<Signing>,
    /// The largest write request to send a quipu server, in bytes
    /// (`SEEDS_MAX_WRITE_BYTES`, default [`DEFAULT_MAX_WRITE_BYTES`]). A larger
    /// sync is pushed in batches; a larger single write is refused unsent.
    pub max_write_bytes: usize,
    /// The most guard clauses one write request may nest
    /// (`SEEDS_MAX_WRITE_CLAUSES`, default [`DEFAULT_MAX_WRITE_CLAUSES`]).
    pub max_write_clauses: usize,
}

/// The default write request cap: under quipu's 64 MiB `/update` body
/// limit, with room for a proxy's own headers.
pub const DEFAULT_MAX_WRITE_BYTES: usize = 48 * 1024 * 1024;

/// The default guard-clause cap per write: a quarter of the smallest nesting
/// measured to abort quipu-server's `/update` (2,000 `UNION` branches).
pub const DEFAULT_MAX_WRITE_CLAUSES: usize = 500;

/// A configured signing identity (user-level only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signing {
    pub key_file: PathBuf,
    pub session: String,
    pub introducer: String,
}

/// Everything resolution reads, passed in so tests control all of it.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    /// The current directory.
    pub cwd: PathBuf,
    /// The user config directory (`$XDG_CONFIG_HOME` or `~/.config`), if known.
    pub user_config_dir: Option<PathBuf>,
    /// The home directory, for `~/` in paths.
    pub home: Option<PathBuf>,
    /// `SEEDS_QUIPU_STORE`.
    pub env_store: Option<String>,
    /// `SEEDS_QUIPU_URL`.
    pub env_url: Option<String>,
    /// `SEEDS_GRAPH`.
    pub env_graph: Option<String>,
    /// `SEEDS_PREFIX`.
    pub env_prefix: Option<String>,
    /// `SEEDS_PENDANT_DIR`.
    pub env_pendant: Option<String>,
    /// `SEEDS_SYNC_REMOTE`.
    pub env_sync_remote: Option<String>,
    /// `SEEDS_QUIPU_TOKEN`.
    pub env_token: Option<String>,
    /// `SEEDS_QUIPU_TOKEN_FILE`.
    pub env_token_file: Option<String>,
    /// `SEEDS_MAX_WRITE_BYTES`.
    pub env_max_write_bytes: Option<String>,
    /// `SEEDS_MAX_WRITE_CLAUSES`.
    pub env_max_write_clauses: Option<String>,
    /// `--store`.
    pub flag_store: Option<String>,
    /// `--quipu`.
    pub flag_url: Option<String>,
    /// `--graph`.
    pub flag_graph: Option<String>,
}

impl Inputs {
    /// Read the process environment.
    pub fn from_env(
        flag_store: Option<String>,
        flag_url: Option<String>,
        flag_graph: Option<String>,
    ) -> Result<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let home = var("HOME").map(PathBuf::from);
        let user_config_dir = var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".config")));
        Ok(Self {
            cwd: std::env::current_dir()
                .map_err(|e| SdError::failed(format!("cannot read the current directory: {e}")))?,
            user_config_dir,
            home,
            env_store: var("SEEDS_QUIPU_STORE"),
            env_url: var("SEEDS_QUIPU_URL"),
            env_graph: var("SEEDS_GRAPH"),
            env_prefix: var("SEEDS_PREFIX"),
            env_pendant: var("SEEDS_PENDANT_DIR"),
            env_sync_remote: var("SEEDS_SYNC_REMOTE"),
            env_token: var("SEEDS_QUIPU_TOKEN"),
            env_token_file: var("SEEDS_QUIPU_TOKEN_FILE"),
            env_max_write_bytes: var("SEEDS_MAX_WRITE_BYTES"),
            env_max_write_clauses: var("SEEDS_MAX_WRITE_CLAUSES"),
            flag_store,
            flag_url,
            flag_graph,
        })
    }
}

fn config_error(message: String) -> SdError {
    SdError::new(ErrorKind::Config, message)
}

/// Parse one config file's text. `origin` names it in errors.
pub fn parse_file(text: &str, origin: &str) -> Result<FileConfig> {
    let cfg: FileConfig = toml::from_str(text)
        .map_err(|e| config_error(format!("{origin}: not a valid seeds config: {e}")))?;
    if cfg.quipu.store.is_some() && cfg.quipu.url.is_some() {
        return Err(config_error(format!(
            "{origin} sets both [quipu] store and [quipu] url; choose one \
             (a local store file OR a shared server)"
        )));
    }
    Ok(cfg)
}

fn read_file(path: &Path) -> Result<Option<FileConfig>> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_file(&text, &path.display().to_string()).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(config_error(format!("cannot read {}: {e}", path.display()))),
    }
}

/// The nearest directory at or above `cwd` holding a `.seeds/` directory.
pub fn find_project_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|d| d.join(PROJECT_DIR).is_dir())
        .map(Path::to_path_buf)
}

fn expand(path: &str, base: &Path, home: Option<&Path>) -> PathBuf {
    if let (Some(rest), Some(h)) = (path.strip_prefix("~/"), home) {
        return h.join(rest);
    }
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

fn check_url(url: &str, origin: &str) -> Result<String> {
    let u = url.trim();
    if u.starts_with("http://") || u.starts_with("https://") {
        Ok(u.trim_end_matches('/').to_string())
    } else {
        Err(config_error(format!(
            "{origin}: quipu url must start with http:// or https://, got {u:?}"
        )))
    }
}

/// The project and user config files resolution would read, whether or not
/// they exist: (project, user).
pub fn config_paths(inputs: &Inputs) -> (Option<PathBuf>, Option<PathBuf>) {
    let project = find_project_root(&inputs.cwd).map(|r| r.join(PROJECT_DIR).join(CONFIG_FILE));
    let user = inputs
        .user_config_dir
        .as_ref()
        .map(|d| d.join("seeds").join(CONFIG_FILE));
    (project, user)
}

/// Resolve the configuration from `inputs`. Reads the two config files.
pub fn resolve(inputs: &Inputs) -> Result<Resolved> {
    let project_root = find_project_root(&inputs.cwd);
    let project_file = project_root
        .as_ref()
        .map(|r| r.join(PROJECT_DIR).join(CONFIG_FILE));
    let project = match &project_file {
        Some(p) => read_file(p)?,
        None => None,
    };
    let user_file = inputs
        .user_config_dir
        .as_ref()
        .map(|d| d.join("seeds").join(CONFIG_FILE));
    let user = match &user_file {
        Some(p) => read_file(p)?,
        None => None,
    };
    if inputs.env_store.is_some() && inputs.env_url.is_some() {
        return Err(config_error(
            "both SEEDS_QUIPU_STORE and SEEDS_QUIPU_URL are set; unset one".into(),
        ));
    }
    if inputs.flag_store.is_some() && inputs.flag_url.is_some() {
        return Err(config_error("pass --store or --quipu, not both".into()));
    }
    let home = inputs.home.as_deref();
    let root = project_root.clone().unwrap_or_else(|| inputs.cwd.clone());

    let (location, location_source) = if let Some(s) = &inputs.flag_store {
        (
            Location::Store(expand(s, &inputs.cwd, home)),
            "--store".to_string(),
        )
    } else if let Some(u) = &inputs.flag_url {
        (Location::Url(check_url(u, "--quipu")?), "--quipu".into())
    } else if let Some(s) = &inputs.env_store {
        (
            Location::Store(expand(s, &inputs.cwd, home)),
            "SEEDS_QUIPU_STORE".into(),
        )
    } else if let Some(u) = &inputs.env_url {
        (
            Location::Url(check_url(u, "SEEDS_QUIPU_URL")?),
            "SEEDS_QUIPU_URL".into(),
        )
    } else if let Some((cfg, path)) = project
        .as_ref()
        .zip(project_file.as_ref())
        .filter(|(c, _)| c.quipu.store.is_some() || c.quipu.url.is_some())
    {
        let origin = path.display().to_string();
        match (&cfg.quipu.store, &cfg.quipu.url) {
            (Some(s), _) => (Location::Store(expand(s, &root, home)), origin),
            (None, Some(u)) => (Location::Url(check_url(u, &origin)?), origin),
            (None, None) => unreachable!("filtered above"),
        }
    } else if let Some((cfg, path)) = user
        .as_ref()
        .zip(user_file.as_ref())
        .filter(|(c, _)| c.quipu.store.is_some() || c.quipu.url.is_some())
    {
        let origin = path.display().to_string();
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        match (&cfg.quipu.store, &cfg.quipu.url) {
            (Some(s), _) => (Location::Store(expand(s, &base, home)), origin),
            (None, Some(u)) => (Location::Url(check_url(u, &origin)?), origin),
            (None, None) => unreachable!("filtered above"),
        }
    } else {
        (
            Location::Store(root.join(PROJECT_DIR).join(DEFAULT_STORE_FILE)),
            "default".into(),
        )
    };

    let pick = |env: &Option<String>, f: fn(&FileConfig) -> &Option<String>| {
        env.clone()
            .or_else(|| project.as_ref().and_then(|c| f(c).clone()))
            .or_else(|| user.as_ref().and_then(|c| f(c).clone()))
    };
    let prefix = pick(&inputs.env_prefix, |c| &c.project.prefix)
        .unwrap_or_else(|| DEFAULT_PREFIX.to_string());
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(config_error(format!(
            "id prefix {prefix:?} must be non-empty ASCII letters, digits, '-' or '_'"
        )));
    }
    if let Some(p) = &project {
        if p.quipu.token_file.is_some()
            || p.quipu.trusted_hosts.is_some()
            || p.quipu.allow_plain_http_hosts.is_some()
            || p.quipu.signing_key_file.is_some()
            || p.quipu.signing_session.is_some()
            || p.quipu.signing_introducer.is_some()
        {
            return Err(config_error(format!(
                "{} sets [quipu] token_file, trusted_hosts, allow_plain_http_hosts or signing_*; only your \
                 user config \
(~/.config/seeds/config.toml) or SEEDS_QUIPU_TOKEN_FILE may, because a \
                 cloned repository could otherwise send your token to a server it chose",
                project_file
                    .as_ref()
                    .map_or_else(String::new, |f| f.display().to_string())
            )));
        }
    }
    let graph = inputs
        .flag_graph
        .clone()
        .or_else(|| pick(&inputs.env_graph, |c| &c.project.graph));
    if let Some(g) = &graph {
        check_iri(g)?;
    }
    // Paths from a file resolve against that file's base; from env, the cwd.
    let user_base = user_file
        .as_ref()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let path_setting = |env: &Option<String>, f: fn(&FileConfig) -> &Option<String>| {
        env.as_ref()
            .map(|v| expand(v, &inputs.cwd, home))
            .or_else(|| {
                project
                    .as_ref()
                    .and_then(|c| f(c).as_ref())
                    .map(|v| expand(v, &root, home))
            })
            .or_else(|| {
                user.as_ref()
                    .and_then(|c| f(c).as_ref())
                    .map(|v| expand(v, &user_base, home))
            })
    };
    let pendant = path_setting(&inputs.env_pendant, |c| &c.pendant.dir);
    let token_file = inputs
        .env_token_file
        .as_ref()
        .map(|v| expand(v, &inputs.cwd, home))
        .or_else(|| {
            user.as_ref()
                .and_then(|c| c.quipu.token_file.as_ref())
                .map(|v| expand(v, &user_base, home))
        });
    let sync_remote_from_project = inputs.env_sync_remote.is_none()
        && project.as_ref().is_some_and(|c| c.sync.remote.is_some());
    let sync_remote = match pick(&inputs.env_sync_remote, |c| &c.sync.remote) {
        Some(u) => Some(check_url(&u, "[sync] remote")?),
        None => None,
    };
    let location_from_project = project_file
        .as_ref()
        .is_some_and(|f| location_source == f.display().to_string());
    let trusted_hosts = user
        .as_ref()
        .and_then(|c| c.quipu.trusted_hosts.clone())
        .unwrap_or_default();
    let allow_plain_http_hosts = user
        .as_ref()
        .and_then(|c| c.quipu.allow_plain_http_hosts.clone())
        .unwrap_or_default();
    let signing = match user.as_ref().map(|c| &c.quipu) {
        Some(q) => match (
            &q.signing_key_file,
            &q.signing_session,
            &q.signing_introducer,
        ) {
            (None, None, None) => None,
            (Some(f), Some(s), Some(i)) => Some(Signing {
                key_file: expand(f, &user_base, home),
                session: s.clone(),
                introducer: i.clone(),
            }),
            _ => {
                return Err(config_error(
                    "[quipu] signing_key_file, signing_session and signing_introducer go \
                     together; set all three (sd key init prints them) or none"
                        .into(),
                ));
            }
        },
        None => None,
    };
    let positive = |v: &Option<String>, name: &str, default: usize| -> Result<usize> {
        match v {
            None => Ok(default),
            Some(v) => match v.trim().parse::<usize>() {
                Ok(n) if n > 0 => Ok(n),
                _ => Err(config_error(format!(
                    "{name} must be a positive whole number, not {v:?}"
                ))),
            },
        }
    };
    let max_write_bytes = positive(
        &inputs.env_max_write_bytes,
        "SEEDS_MAX_WRITE_BYTES",
        DEFAULT_MAX_WRITE_BYTES,
    )?;
    let max_write_clauses = positive(
        &inputs.env_max_write_clauses,
        "SEEDS_MAX_WRITE_CLAUSES",
        DEFAULT_MAX_WRITE_CLAUSES,
    )?;
    Ok(Resolved {
        location,
        location_source,
        prefix,
        graph,
        project_id_file: root.join(PROJECT_DIR).join(PROJECT_ID_FILE),
        location_from_project,
        sync_remote_from_project,
        trusted_hosts,
        allow_plain_http_hosts,
        pendant,
        sync_remote,
        token: inputs.env_token.clone(),
        token_file,
        signing,
        max_write_bytes,
        max_write_clauses,
    })
}

/// The file holding a project's generated id, committed with the project.
pub const PROJECT_ID_FILE: &str = "project-id";

/// Refuse a graph name that is not a plain absolute IRI. It is written into
/// SPARQL between angle brackets, so anything that could close them (or a
/// string, or a group) is refused rather than escaped.
pub fn check_iri(iri: &str) -> Result<()> {
    let scheme_ok = iri.split_once(':').is_some_and(|(s, rest)| {
        !rest.is_empty()
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    let chars_ok = iri
        .chars()
        .all(|c| !c.is_whitespace() && !c.is_control() && !"<>\"{}|\\^`".contains(c));
    if scheme_ok && chars_ok {
        Ok(())
    } else {
        Err(config_error(format!(
            "graph {iri:?} is not an absolute IRI (scheme:rest, no spaces or any of <>\"{{}}|\\^`)"
        )))
    }
}

impl Resolved {
    /// The graph IRI. Call [`project_graph`] first to fill it in.
    pub fn graph(&self) -> &str {
        self.graph.as_deref().unwrap_or("")
    }

    /// Whether the token may be sent to `url`, given where that URL came from.
    pub fn token_allowed(&self, url: &str, from_project: bool) -> bool {
        if !from_project {
            return true;
        }
        let host = url
            .split_once("://")
            .map(|(_, r)| r.split(['/', '?', '#']).next().unwrap_or_default())
            .unwrap_or_default()
            .rsplit('@')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let bare = host.split(':').next().unwrap_or_default().to_string();
        self.trusted_hosts
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&host) || t.eq_ignore_ascii_case(&bare))
    }
}

/// The project's graph: the configured one, else
/// `https://seeds.local/project/<prefix>-<id>`, where `<id>` is read from
/// `.seeds/project-id`. A project with no id gets one generated when `create`
/// is true (first write, or first use of a server), so two repositories that
/// never set a prefix still get separate ledgers on a shared server.
/// Returns the graph and, when an id was just created, a note saying so.
pub fn project_graph(cfg: &Resolved, create: bool) -> Result<(Option<String>, Option<String>)> {
    if let Some(g) = &cfg.graph {
        return Ok((Some(g.clone()), None));
    }
    let derive = |id: &str| vocab::project_graph_iri(&format!("{}-{id}", cfg.prefix));
    match std::fs::read_to_string(&cfg.project_id_file) {
        Ok(id) => {
            let id = id.trim();
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(config_error(format!(
                    "{} does not hold a project id (letters and digits)",
                    cfg.project_id_file.display()
                )));
            }
            Ok((Some(derive(id)), None))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok((None, None));
            }
            let id = new_project_id();
            if let Some(dir) = cfg.project_id_file.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| config_error(format!("cannot create {}: {e}", dir.display())))?;
            }
            std::fs::write(&cfg.project_id_file, format!("{id}\n")).map_err(|e| {
                config_error(format!(
                    "cannot write {}: {e}",
                    cfg.project_id_file.display()
                ))
            })?;
            Ok((
                Some(derive(&id)),
                Some(format!(
                    "created project id {id} in {} (commit it: it names this project's ledger)",
                    cfg.project_id_file.display()
                )),
            ))
        }
        Err(e) => Err(config_error(format!(
            "cannot read {}: {e}",
            cfg.project_id_file.display()
        ))),
    }
}

/// Ten base36 characters from the clock, the process and the hasher's
/// per-process random keys: unique enough to name a project.
/// What `sd init` did in one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    /// The `.seeds/` directory.
    pub dir: PathBuf,
    /// The prefix the project uses.
    pub prefix: String,
    /// The project id (new, or the one already there).
    pub project_id: String,
    /// Files written, relative to `dir`.
    pub created: Vec<String>,
    /// Files that were already there and were left untouched.
    pub kept: Vec<String>,
}

/// `sd init`: make `<cwd>/.seeds/` a project: `config.toml` (with the prefix),
/// `project-id` and `.gitignore`.
///
/// The graph a project writes to is derived from its prefix AND its id, so
/// changing either orphans every seed already written. So an initialized
/// project is refused, and `force` only restores missing files: it never
/// replaces the id or changes the prefix.
pub fn init_project(
    cwd: &Path,
    prefix: Option<&str>,
    default_prefix: &str,
    force: bool,
) -> Result<InitReport> {
    let dir = cwd.join(PROJECT_DIR);
    let (config, id_file) = (dir.join(CONFIG_FILE), dir.join("project-id"));
    let existing_id = std::fs::read_to_string(&id_file)
        .ok()
        .map(|s| s.trim().to_string());
    if existing_id.is_some() && !force {
        return Err(SdError::conflict(format!(
            "{} is already a seeds project (project id in {}); nothing was written. \
             --force restores missing files but never changes the id or prefix",
            dir.display(),
            id_file.display()
        )));
    }
    let configured = std::fs::read_to_string(&config)
        .ok()
        .map(|t| {
            toml::from_str::<FileConfig>(&t)
                .map_err(|e| config_error(format!("{}: {e}", config.display())))
        })
        .transpose()?
        .map(|c| c.project.prefix);
    let prefix = match (prefix, &configured) {
        (Some(p), Some(Some(c))) if p != c => {
            return Err(SdError::refused(format!(
                "{} already sets prefix {c:?}; changing it to {p:?} would move this project to a \
                 different ledger. Nothing was written",
                config.display()
            )))
        }
        (Some(p), _) => p.to_string(),
        (None, Some(Some(c))) => c.clone(),
        (None, _) => default_prefix.to_string(),
    };
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(SdError::usage(format!(
            "id prefix {prefix:?} must be non-empty ASCII letters, digits, '-' or '_'"
        )));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| config_error(format!("cannot create {}: {e}", dir.display())))?;
    let (mut created, mut kept) = (Vec::new(), Vec::new());
    let write = |path: &Path, text: String| {
        std::fs::write(path, text)
            .map_err(|e| config_error(format!("cannot write {}: {e}", path.display())))
    };
    if config.exists() {
        kept.push(CONFIG_FILE.to_string());
    } else {
        write(&config, format!("[project]\nprefix = {prefix:?}\n"))?;
        created.push(CONFIG_FILE.to_string());
    }
    let project_id = match existing_id {
        Some(id) => {
            kept.push("project-id".into());
            id
        }
        None => {
            let id = new_project_id();
            write(&id_file, format!("{id}\n"))?;
            created.push("project-id".into());
            id
        }
    };
    let gitignore = dir.join(".gitignore");
    if gitignore.exists() {
        kept.push(".gitignore".into());
    } else {
        crate::native::store::ensure_gitignore(&dir.join(DEFAULT_STORE_FILE));
        created.push(".gitignore".into());
    }
    Ok(InitReport {
        dir,
        prefix,
        project_id,
        created,
        kept,
    })
}

fn new_project_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(std::process::id());
    let mut n = h.finish();
    let alphabet = b"0123456789abcdefghijklmnopqrstuvwxyz";
    (0..10)
        .map(|_| {
            let c = alphabet[(n % 36) as usize] as char;
            n /= 36;
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods, clippy::disallowed_types)]
    use super::*;

    struct Sandbox {
        root: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("seeds-config-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("home/.config")).unwrap();
            std::fs::create_dir_all(root.join("proj/sub/dir")).unwrap();
            Self { root }
        }
        fn inputs(&self) -> Inputs {
            Inputs {
                cwd: self.root.join("proj/sub/dir"),
                user_config_dir: Some(self.root.join("home/.config")),
                home: Some(self.root.join("home")),
                ..Inputs::default()
            }
        }
        fn project_toml(&self, text: &str) {
            std::fs::create_dir_all(self.root.join("proj/.seeds")).unwrap();
            std::fs::write(self.root.join("proj/.seeds/config.toml"), text).unwrap();
        }
        fn user_toml(&self, text: &str) {
            std::fs::create_dir_all(self.root.join("home/.config/seeds")).unwrap();
            std::fs::write(self.root.join("home/.config/seeds/config.toml"), text).unwrap();
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn with_no_config_at_all_the_default_is_a_local_store_in_cwd() {
        let sb = Sandbox::new("default");
        let r = resolve(&sb.inputs()).unwrap();
        assert_eq!(
            r.location,
            Location::Store(sb.root.join("proj/sub/dir/.seeds/seeds.db"))
        );
        assert_eq!(r.location_source, "default");
        assert_eq!(r.prefix, "sd");
        assert_eq!(
            r.graph, None,
            "derived from the project id, created on first use"
        );
        assert_eq!(
            r.project_id_file,
            sb.root.join("proj/sub/dir/.seeds/project-id")
        );
    }

    #[test]
    fn the_default_store_sits_in_the_nearest_seeds_dir_walking_up() {
        let sb = Sandbox::new("walkup");
        std::fs::create_dir_all(sb.root.join("proj/.seeds")).unwrap();
        let r = resolve(&sb.inputs()).unwrap();
        assert_eq!(
            r.location,
            Location::Store(sb.root.join("proj/.seeds/seeds.db"))
        );
    }

    #[test]
    fn project_toml_beats_user_toml() {
        let sb = Sandbox::new("project-beats-user");
        sb.user_toml("[quipu]\nurl = \"https://user.example.org\"\n[project]\nprefix = \"usr\"\n");
        sb.project_toml("[quipu]\nstore = \"data/p.db\"\n[project]\nprefix = \"prj\"\n");
        let r = resolve(&sb.inputs()).unwrap();
        assert_eq!(r.location, Location::Store(sb.root.join("proj/data/p.db")));
        assert!(r.location_source.ends_with(".seeds/config.toml"));
        assert_eq!(r.prefix, "prj");
    }

    #[test]
    fn user_toml_applies_when_the_project_says_nothing() {
        let sb = Sandbox::new("user-only");
        sb.user_toml("[quipu]\nurl = \"https://user.example.org/\"\n");
        let r = resolve(&sb.inputs()).unwrap();
        assert_eq!(r.location, Location::Url("https://user.example.org".into()));
    }

    #[test]
    fn env_overrides_both_files_and_flags_override_env() {
        let sb = Sandbox::new("env");
        sb.user_toml("[quipu]\nurl = \"https://user.example.org\"\n");
        sb.project_toml("[quipu]\nstore = \"p.db\"\n");
        let mut i = sb.inputs();
        i.env_url = Some("http://env.example.org:7070".into());
        i.env_prefix = Some("env".into());
        let r = resolve(&i).unwrap();
        assert_eq!(
            r.location,
            Location::Url("http://env.example.org:7070".into())
        );
        assert_eq!(r.location_source, "SEEDS_QUIPU_URL");
        assert_eq!(r.prefix, "env");
        i.flag_store = Some("/abs/flag.db".into());
        let r = resolve(&i).unwrap();
        assert_eq!(r.location, Location::Store(PathBuf::from("/abs/flag.db")));
    }

    #[test]
    fn a_file_that_sets_both_store_and_url_is_refused() {
        let sb = Sandbox::new("both-file");
        sb.project_toml("[quipu]\nstore = \"a.db\"\nurl = \"https://x.example.org\"\n");
        let e = resolve(&sb.inputs()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Config);
        assert!(e.message.contains("both"), "{}", e.message);
    }

    #[test]
    fn env_that_sets_both_is_refused() {
        let sb = Sandbox::new("both-env");
        let mut i = sb.inputs();
        i.env_store = Some("a.db".into());
        i.env_url = Some("https://x.example.org".into());
        assert_eq!(resolve(&i).unwrap_err().kind, ErrorKind::Config);
    }

    #[test]
    fn unknown_keys_and_bad_urls_are_refused() {
        assert!(parse_file("[quipu]\nstroe = \"x\"\n", "t").is_err());
        let sb = Sandbox::new("bad-url");
        sb.project_toml("[quipu]\nurl = \"quipu.example.org\"\n");
        assert_eq!(resolve(&sb.inputs()).unwrap_err().kind, ErrorKind::Config);
    }

    #[test]
    fn graph_follows_flag_then_env_then_file_then_the_project_id() {
        let sb = Sandbox::new("graph");
        sb.project_toml("[project]\nprefix = \"abc\"\n");
        let mut i = sb.inputs();
        let r = resolve(&i).unwrap();
        assert_eq!(r.graph, None);
        // First use creates the id; the graph is derived from prefix + id.
        let (g, note) = project_graph(&r, true).unwrap();
        let g = g.unwrap();
        assert!(g.starts_with("https://seeds.local/project/abc-"), "{g}");
        assert!(note.unwrap().contains("project-id"));
        // It is stable: read back, not regenerated.
        assert_eq!(project_graph(&r, true).unwrap().0.unwrap(), g);
        i.env_graph = Some("urn:g:env".into());
        assert_eq!(resolve(&i).unwrap().graph.as_deref(), Some("urn:g:env"));
        i.flag_graph = Some("urn:g:flag".into());
        assert_eq!(resolve(&i).unwrap().graph.as_deref(), Some("urn:g:flag"));
    }

    #[test]
    fn two_projects_with_no_prefix_get_different_graphs() {
        let a = Sandbox::new("proj-a");
        let b = Sandbox::new("proj-b");
        let ga = project_graph(&resolve(&a.inputs()).unwrap(), true)
            .unwrap()
            .0
            .unwrap();
        let gb = project_graph(&resolve(&b.inputs()).unwrap(), true)
            .unwrap()
            .0
            .unwrap();
        assert_ne!(ga, gb);
    }

    #[test]
    fn a_plain_read_does_not_create_a_project_id() {
        let sb = Sandbox::new("no-create");
        let r = resolve(&sb.inputs()).unwrap();
        assert_eq!(project_graph(&r, false).unwrap(), (None, None));
        assert!(!r.project_id_file.exists());
    }

    #[test]
    fn a_graph_that_could_break_out_of_sparql_is_refused() {
        let sb = Sandbox::new("graph-inject");
        for bad in [
            "https://x> } ; DROP ALL ; { <urn:y",
            "no-scheme",
            "urn:has space",
            "urn:quote\"",
            "",
        ] {
            let mut i = sb.inputs();
            i.flag_graph = Some(bad.into());
            assert_eq!(resolve(&i).unwrap_err().kind, ErrorKind::Config, "{bad:?}");
        }
        let mut i = sb.inputs();
        i.flag_graph = Some("https://seeds.local/project/ok-1".into());
        assert!(resolve(&i).is_ok());
    }

    #[test]
    fn a_project_file_may_not_name_a_token_file() {
        let sb = Sandbox::new("token-project");
        sb.project_toml(
            "[quipu]\nurl = \"https://evil.example.org\"\ntoken_file = \"~/.config/seeds/token\"\n",
        );
        let e = resolve(&sb.inputs()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Config);
        assert!(e.message.contains("token"), "{}", e.message);
        let sb = Sandbox::new("trusted-project");
        sb.project_toml("[quipu]\ntrusted_hosts = [\"evil.example.org\"]\n");
        assert_eq!(resolve(&sb.inputs()).unwrap_err().kind, ErrorKind::Config);
        let sb = Sandbox::new("plain-http-project");
        sb.project_toml("[quipu]\nallow_plain_http_hosts = [\"evil.example.org\"]\n");
        let e = resolve(&sb.inputs()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Config);
        assert!(
            e.message.contains("allow_plain_http_hosts"),
            "{}",
            e.message
        );
    }

    #[test]
    fn plain_http_hosts_come_only_from_the_user_config() {
        let sb = Sandbox::new("plain-http-user");
        sb.user_toml("[quipu]\nallow_plain_http_hosts = [\"quipu.internal.example\"]\n");
        let r = resolve(&sb.inputs()).unwrap();
        assert_eq!(
            r.allow_plain_http_hosts,
            vec!["quipu.internal.example".to_string()]
        );
        let sb = Sandbox::new("plain-http-none");
        assert!(resolve(&sb.inputs())
            .unwrap()
            .allow_plain_http_hosts
            .is_empty());
    }

    #[test]
    fn the_user_token_goes_only_to_urls_the_user_chose_or_trusts() {
        let sb = Sandbox::new("token-user");
        sb.user_toml("[quipu]\ntoken_file = \"~/tok\"\ntrusted_hosts = [\"quipu.example.org\"]\n");
        sb.project_toml("[quipu]\nurl = \"https://evil.example.org\"\n");
        let r = resolve(&sb.inputs()).unwrap();
        assert_eq!(r.token_file, Some(sb.root.join("home/tok")));
        assert!(r.location_from_project);
        assert!(!r.token_allowed("https://evil.example.org", true));
        assert!(r.token_allowed("https://quipu.example.org:8443/x", true));
        assert!(r.token_allowed("https://anything.example.org", false));
        // The same URL chosen by the user (env) is trusted.
        let mut i = sb.inputs();
        i.env_url = Some("https://evil.example.org".into());
        assert!(!resolve(&i).unwrap().location_from_project);
    }
}

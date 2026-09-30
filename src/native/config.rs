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
    /// The project's named graph IRI.
    pub graph: String,
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
    let graph = inputs
        .flag_graph
        .clone()
        .or_else(|| pick(&inputs.env_graph, |c| &c.project.graph))
        .unwrap_or_else(|| vocab::project_graph_iri(&prefix));
    Ok(Resolved {
        location,
        location_source,
        prefix,
        graph,
    })
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
        assert_eq!(r.graph, "https://seeds.local/project/sd");
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
    fn graph_follows_flag_then_env_then_file_then_prefix() {
        let sb = Sandbox::new("graph");
        sb.project_toml("[project]\nprefix = \"abc\"\n");
        let mut i = sb.inputs();
        assert_eq!(
            resolve(&i).unwrap().graph,
            "https://seeds.local/project/abc"
        );
        i.env_graph = Some("urn:g:env".into());
        assert_eq!(resolve(&i).unwrap().graph, "urn:g:env");
        i.flag_graph = Some("urn:g:flag".into());
        assert_eq!(resolve(&i).unwrap().graph, "urn:g:flag");
    }
}

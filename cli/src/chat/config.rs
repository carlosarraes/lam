use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::types::Limits;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_LIMITS: Limits = Limits {
    inline_bytes: 4 * 1024,
    batch_bytes: 8 * 1024,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub machine: String,
    pub inline_bytes: usize,
    pub batch_bytes: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectMapping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<PeerConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMapping {
    pub id: String,
    pub roots: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    pub machine: String,
    pub ssh_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_binary: Option<PathBuf>,
    pub initiator: bool,
    pub projects: Vec<String>,
}

impl Config {
    pub fn load_existing(path: &Path) -> Result<Self> {
        Self::load(path)
    }

    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let config = Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            machine: uuid::Uuid::new_v4().to_string(),
            inline_bytes: DEFAULT_LIMITS.inline_bytes,
            batch_bytes: DEFAULT_LIMITS.batch_bytes,
            projects: Vec::new(),
            peers: Vec::new(),
        };
        config.create_or_load(path)
    }

    pub fn limits(&self) -> Limits {
        Limits {
            inline_bytes: self.inline_bytes,
            batch_bytes: self.batch_bytes,
        }
    }

    /// Match a repository/workspace root exactly. Callers discover that root;
    /// ancestor matching here would silently merge nested worktrees.
    pub fn project_for_root(&self, root: &Path) -> Result<String> {
        let mappings = self.project_roots()?;
        let root = normalize_project_root(root)?;
        if let Some(project) = mappings.get(&root) {
            return Ok(project.clone());
        }
        let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
        Ok(format!("local:{}:{digest:x}", self.machine))
    }

    fn project_roots(&self) -> Result<std::collections::BTreeMap<PathBuf, String>> {
        let mut roots = std::collections::BTreeMap::new();
        for project in &self.projects {
            let id = uuid::Uuid::parse_str(&project.id)
                .context("Chat project ID is not a UUID")?
                .to_string();
            ensure!(
                !project.roots.is_empty(),
                "Chat project {id} needs at least one local root"
            );
            for root in &project.roots {
                ensure!(
                    root.is_absolute(),
                    "Chat project root must be absolute: {}",
                    root.display()
                );
                let root = normalize_project_root(root)?;
                if let Some(previous) = roots.insert(root.clone(), id.clone()) {
                    ensure!(
                        previous == id,
                        "Chat root {} maps to multiple project IDs",
                        root.display()
                    );
                }
            }
        }
        Ok(roots)
    }

    pub fn peer(&self, machine: &str) -> Option<&PeerConfig> {
        self.peers.iter().find(|peer| peer.machine == machine)
    }

    fn validate_peers(&self) -> Result<()> {
        ensure!(self.peers.len() <= 8, "too many Chat peers");
        let projects: std::collections::BTreeSet<_> = self
            .projects
            .iter()
            .map(|project| project.id.as_str())
            .collect();
        let mut machines = std::collections::BTreeSet::new();
        for peer in &self.peers {
            super::protocol::validate_uuid(&peer.machine)?;
            ensure!(
                peer.machine != self.machine,
                "Chat peer cannot be the local machine"
            );
            ensure!(
                machines.insert(&peer.machine),
                "duplicate Chat peer machine"
            );
            ensure!(
                !peer.projects.is_empty(),
                "Chat peer needs a shared project"
            );
            let mut allowed = std::collections::BTreeSet::new();
            for project in &peer.projects {
                super::protocol::validate_uuid(project)?;
                ensure!(
                    projects.contains(project.as_str()),
                    "Chat peer project has no local mapping"
                );
                ensure!(allowed.insert(project), "duplicate Chat peer project");
            }
            ensure!(
                !peer.ssh_host.is_empty()
                    && peer.ssh_host.len() <= 255
                    && peer
                        .ssh_host
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.@".contains(&byte))
                    && !peer.ssh_host.starts_with('-'),
                "invalid Chat SSH host alias",
            );
            if let Some(binary) = &peer.remote_binary {
                let path = binary
                    .to_str()
                    .context("Chat remote binary path is not UTF-8")?;
                ensure!(
                    binary.is_absolute() && !path.chars().any(char::is_control),
                    "Chat remote binary must be an absolute path without control characters"
                );
            }
        }
        Ok(())
    }

    fn load(path: &Path) -> Result<Self> {
        validate_private_file(path, "Chat config")?;
        let mut raw = String::new();
        open_read_no_follow(path)?
            .read_to_string(&mut raw)
            .with_context(|| format!("cannot read Chat config {}", path.display()))?;
        let config: Self = toml::from_str(&raw).context("invalid Chat config")?;
        ensure!(
            config.schema_version == CONFIG_SCHEMA_VERSION,
            "Chat config schema {} is not supported by version {}",
            config.schema_version,
            CONFIG_SCHEMA_VERSION
        );
        uuid::Uuid::parse_str(&config.machine).context("Chat machine ID is not a UUID")?;
        validate_limits(config.limits())?;
        config.project_roots()?;
        config.validate_peers()?;
        Ok(config)
    }

    fn create_or_load(self, path: &Path) -> Result<Self> {
        validate_limits(self.limits())?;
        self.validate_peers()?;
        let parent = path
            .parent()
            .context("Chat config path has no parent directory")?;
        ensure_private_dir(parent, "Chat config directory")?;
        let raw = toml::to_string_pretty(&self)?;
        match open_create_new(path) {
            Ok(mut file) => {
                file.write_all(raw.as_bytes())?;
                file.sync_all()?;
                validate_private_file(path, "Chat config")?;
                Ok(self)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Self::load(path),
            Err(error) => Err(error).with_context(|| format!("cannot create {}", path.display())),
        }
    }
}

fn normalize_project_root(root: &Path) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot resolve Chat project root {}", root.display()))?;
    ensure!(
        root.is_dir(),
        "Chat project root is not a directory: {}",
        root.display()
    );
    Ok(root)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub database: PathBuf,
    pub socket: PathBuf,
    pub observer_socket: PathBuf,
    pub peer_socket: PathBuf,
    pub lock: PathBuf,
}

impl Paths {
    pub fn validate(&self) -> Result<()> {
        for path in [
            &self.config,
            &self.database,
            &self.socket,
            &self.observer_socket,
            &self.peer_socket,
            &self.lock,
        ] {
            validate_path_components(path)?;
            ensure_private_dir(
                path.parent().context("Chat path has no parent")?,
                "Chat managed directory",
            )?;
        }
        validate_socket_path(&self.socket)?;
        validate_socket_path(&self.observer_socket)?;
        validate_socket_path(&self.peer_socket)
    }

    pub fn discover() -> Result<Self> {
        let config_override = std::env::var_os("LAM_CHAT_CONFIG");
        let data_override = std::env::var_os("LAM_CHAT_DATA_DIR");
        let current_dir = std::env::current_dir().context("cannot resolve current directory")?;
        let config_is_override = config_override.is_some();
        let data_is_override = data_override.is_some();
        let mut config = choose_path(config_override, || {
            Ok(dirs::config_dir()
                .context("no platform config directory")?
                .join("lam-chat")
                .join("chat.toml"))
        })?;
        let mut data = choose_path(data_override, || {
            Ok(dirs::data_local_dir()
                .context("no platform data directory")?
                .join("lam-chat"))
        })?;
        if config_is_override {
            config = anchor_override(config, &current_dir);
        }
        if data_is_override {
            data = anchor_override(data, &current_dir);
        }

        build_paths(config, data, runtime_base()?)
    }
}

fn choose_path<F>(override_value: Option<OsString>, fallback: F) -> Result<PathBuf>
where
    F: FnOnce() -> Result<PathBuf>,
{
    match override_value {
        Some(path) => Ok(PathBuf::from(path)),
        None => fallback(),
    }
}

fn anchor_override(path: PathBuf, current_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        current_dir.join(path)
    }
}

fn build_paths(config: PathBuf, data: PathBuf, runtime_base: PathBuf) -> Result<Paths> {
    validate_path_components(&config)?;
    validate_path_components(&data)?;
    validate_path_components(&runtime_base)?;

    let config_directory = config
        .parent()
        .context("LAM_CHAT_CONFIG must name a file in a directory")?;
    ensure_private_dir(config_directory, "Chat config directory")?;
    ensure_private_dir(&data, "Chat data directory")?;
    if config.exists() {
        validate_private_file(&config, "Chat config")?;
    }

    let canonical_data = data
        .canonicalize()
        .with_context(|| format!("cannot resolve Chat data directory {}", data.display()))?;
    let runtime_root = runtime_base.join(runtime_root_name());
    validate_path_components(&runtime_root)?;
    ensure_private_dir(&runtime_root, "Chat runtime directory")?;
    let namespace = hex_prefix(&Sha256::digest(
        canonical_data.as_os_str().as_encoded_bytes(),
    ));
    let runtime = runtime_root.join(namespace);
    validate_path_components(&runtime)?;
    ensure_private_dir(&runtime, "Chat runtime namespace")?;
    let socket = runtime.join("chat.sock");
    let observer_socket = runtime.join("observer.sock");
    let peer_socket = runtime.join("peer.sock");
    validate_socket_path(&socket)?;
    validate_socket_path(&observer_socket)?;
    validate_socket_path(&peer_socket)?;

    Ok(Paths {
        config,
        database: canonical_data.join("chat.sqlite3"),
        socket,
        observer_socket,
        peer_socket,
        lock: canonical_data.join("chat.lock"),
    })
}

fn validate_socket_path(path: &Path) -> Result<()> {
    const PORTABLE_SUN_PATH_CAPACITY: usize = 104;

    ensure!(
        path.as_os_str().as_encoded_bytes().len() < PORTABLE_SUN_PATH_CAPACITY,
        "Chat socket path exceeds the portable 103-byte Unix socket pathname limit: {}",
        path.display()
    );
    Ok(())
}

pub fn validate_limits(limits: Limits) -> Result<()> {
    ensure!(
        limits.inline_bytes > 0,
        "Chat inline byte limit must be positive"
    );
    ensure!(
        limits.batch_bytes > 0,
        "Chat batch byte limit must be positive"
    );
    ensure!(
        limits.inline_bytes <= limits.batch_bytes,
        "Chat inline byte limit cannot exceed the batch byte limit"
    );
    ensure!(
        limits.batch_bytes <= 8192,
        "Chat batch byte limit cannot exceed 8 KiB"
    );
    ensure!(
        limits.batch_bytes >= super::render::minimum_batch_bytes()?,
        "Chat batch byte limit cannot fit the encoded minimum envelope and overflow hint"
    );
    Ok(())
}

fn hex_prefix(digest: &[u8]) -> String {
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn validate_path_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir
            | Component::Normal(_) => current.push(component.as_os_str()),
        }
        match current.symlink_metadata() {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "Chat path cannot traverse symlink {}; choose a direct path",
                    current.display()
                );
                validate_component_permissions(&current, &metadata)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_component_permissions(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.file_type().is_dir() {
        let effective_uid = unsafe { libc::geteuid() };
        ensure!(
            metadata.uid() == 0 || metadata.uid() == effective_uid,
            "Chat path has ancestor {} owned by uid {}; use a path owned by root or the current user",
            path.display(),
            metadata.uid()
        );
        ensure!(
            ancestor_is_trusted(metadata.uid(), metadata.mode(), effective_uid),
            "Chat path has writable ancestor {}; remove group/world write access or use a sticky directory owned by root or the current user",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_component_permissions(_path: &Path, _metadata: &std::fs::Metadata) -> Result<()> {
    Ok(())
}

fn ancestor_is_trusted(owner: u32, mode: u32, effective_uid: u32) -> bool {
    let trusted_owner = owner == 0 || owner == effective_uid;
    let not_replaceable = mode & 0o022 == 0 || mode & 0o1000 != 0;
    trusted_owner && not_replaceable
}

#[cfg(unix)]
fn runtime_base() -> Result<PathBuf> {
    let path = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    path.canonicalize()
        .with_context(|| format!("cannot resolve runtime directory {}", path.display()))
}

#[cfg(not(unix))]
fn runtime_base() -> Result<PathBuf> {
    Ok(std::env::temp_dir())
}

#[cfg(unix)]
fn runtime_root_name() -> String {
    format!("lam-chat-{}", unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
fn runtime_root_name() -> String {
    "lam-chat".into()
}

#[cfg(unix)]
pub(super) fn ensure_private_dir(path: &Path, label: &str) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    if !path.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(path)
            .with_context(|| format!("cannot create {label} {}", path.display()))?;
    }
    let metadata = path.symlink_metadata()?;
    ensure!(metadata.file_type().is_dir(), "{label} must be a directory");
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "{label} has another owner"
    );
    ensure!(
        metadata.mode() & 0o777 == 0o700,
        "{label} permissions must be 0700"
    );
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn ensure_private_dir(path: &Path, label: &str) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("cannot create {label} {}", path.display()))?;
    ensure!(path.is_dir(), "{label} must be a directory");
    Ok(())
}

#[cfg(unix)]
fn validate_private_file(path: &Path, label: &str) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = path.symlink_metadata()?;
    ensure!(
        metadata.file_type().is_file(),
        "{label} must be a regular file"
    );
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() },
        "{label} has another owner"
    );
    ensure!(
        metadata.mode() & 0o777 == 0o600,
        "{label} permissions must be 0600"
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_file(path: &Path, label: &str) -> Result<()> {
    ensure!(path.is_file(), "{label} must be a regular file");
    Ok(())
}

#[cfg(unix)]
fn open_create_new(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_create_new(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn open_read_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_read_no_follow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn one_data_directory_keeps_one_lock_across_runtime_directories() {
        let dir = tempfile::tempdir().unwrap();
        let first = super::build_paths(
            dir.path().join("config/chat.toml"),
            dir.path().join("data"),
            dir.path().join("runtime-a"),
        )
        .unwrap();
        let second = super::build_paths(
            dir.path().join("config/chat.toml"),
            dir.path().join("data"),
            dir.path().join("runtime-b"),
        )
        .unwrap();
        assert_eq!(first.lock, second.lock);
        assert_ne!(first.socket, second.socket);
    }
    #[test]
    fn project_roots_are_machine_local_until_explicitly_mapped() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left/repo");
        let right = dir.path().join("right/repo");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        let config_path = dir.path().join("config/chat.toml");
        let mut config = super::Config::load_or_create(&config_path).unwrap();
        let left_id = config.project_for_root(&left).unwrap();
        assert_ne!(left_id, config.project_for_root(&right).unwrap());
        assert_eq!(left_id, config.project_for_root(&left.join(".")).unwrap());
        let mut another_machine = config.clone();
        another_machine.machine = uuid::Uuid::new_v4().to_string();
        assert_ne!(left_id, another_machine.project_for_root(&left).unwrap());

        let shared = "a6eebdad-52bb-4f4b-aa7b-1c041efa9091";
        config.projects = vec![super::ProjectMapping {
            id: shared.into(),
            roots: vec![left.clone(), right.clone()],
        }];
        let raw = toml::to_string(&config).unwrap();
        std::fs::write(&config_path, raw).unwrap();
        let loaded = super::Config::load_or_create(&config_path).unwrap();
        assert_eq!(loaded.project_for_root(&left).unwrap(), shared);
        assert_eq!(loaded.project_for_root(&right).unwrap(), shared);
    }

    #[test]
    fn project_mapping_rejects_ambiguous_roots_and_invalid_shared_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let mut config =
            super::Config::load_or_create(&dir.path().join("config/chat.toml")).unwrap();
        let mapping = super::ProjectMapping {
            id: "a6eebdad-52bb-4f4b-aa7b-1c041efa9091".into(),
            roots: vec![root.clone()],
        };
        config.projects = vec![
            mapping.clone(),
            super::ProjectMapping {
                id: "b6eebdad-52bb-4f4b-aa7b-1c041efa9091".into(),
                roots: vec![root.join(".")],
            },
        ];
        assert!(config.project_for_root(&root).is_err());
        config.projects = vec![super::ProjectMapping {
            id: "repo".into(),
            ..mapping.clone()
        }];
        assert!(config.project_for_root(&root).is_err());
        config.projects = vec![super::ProjectMapping {
            roots: vec![std::path::PathBuf::from("repo")],
            ..mapping
        }];
        assert!(config.project_for_root(&root).is_err());
    }

    #[test]
    fn unmapped_worktrees_stay_separate_and_physical_aliases_match() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("repo");
        let worktree = main.join(".worktrees/feature");
        std::fs::create_dir_all(&worktree).unwrap();
        let config_path = dir.path().join("config/chat.toml");
        let mut config = super::Config::load_or_create(&config_path).unwrap();
        assert_ne!(
            config.project_for_root(&main).unwrap(),
            config.project_for_root(&worktree).unwrap()
        );
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&main, &alias).unwrap();
        assert_eq!(
            config.project_for_root(&main).unwrap(),
            config.project_for_root(&alias).unwrap()
        );
        config.projects = vec![super::ProjectMapping {
            id: "a6eebdad-52bb-4f4b-aa7b-1c041efa9091".into(),
            roots: vec![main.clone()],
        }];
        assert_ne!(
            config.project_for_root(&main).unwrap(),
            config.project_for_root(&worktree).unwrap()
        );
        config.projects[0].roots.push(worktree.clone());
        assert_eq!(
            config.project_for_root(&main).unwrap(),
            config.project_for_root(&worktree).unwrap()
        );
    }

    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::sync::Mutex;

    use super::{
        ancestor_is_trusted, anchor_override, build_paths, choose_path, validate_limits,
        validate_socket_path, Config, Paths, DEFAULT_LIMITS,
    };
    use crate::chat::types::Limits;

    static ENVIRONMENT: Mutex<()> = Mutex::new(());

    struct Environment {
        config: Option<std::ffi::OsString>,
        data: Option<std::ffi::OsString>,
    }

    impl Environment {
        fn set(config: &std::path::Path, data: &std::path::Path) -> Self {
            let previous = Self {
                config: std::env::var_os("LAM_CHAT_CONFIG"),
                data: std::env::var_os("LAM_CHAT_DATA_DIR"),
            };
            std::env::set_var("LAM_CHAT_CONFIG", config);
            std::env::set_var("LAM_CHAT_DATA_DIR", data);
            previous
        }
    }

    impl Drop for Environment {
        fn drop(&mut self) {
            match self.config.take() {
                Some(value) => std::env::set_var("LAM_CHAT_CONFIG", value),
                None => std::env::remove_var("LAM_CHAT_CONFIG"),
            }
            match self.data.take() {
                Some(value) => std::env::set_var("LAM_CHAT_DATA_DIR", value),
                None => std::env::remove_var("LAM_CHAT_DATA_DIR"),
            }
        }
    }

    #[test]
    fn limits_require_positive_feasible_budgets() {
        validate_limits(DEFAULT_LIMITS).unwrap();
        assert!(validate_limits(Limits {
            inline_bytes: 0,
            batch_bytes: 8192,
        })
        .is_err());
        assert!(validate_limits(Limits {
            inline_bytes: 4096,
            batch_bytes: 0,
        })
        .is_err());
        assert!(validate_limits(Limits {
            inline_bytes: 8193,
            batch_bytes: 8192,
        })
        .is_err());
        assert!(validate_limits(Limits {
            inline_bytes: 1,
            batch_bytes: 1,
        })
        .is_err());
    }

    #[test]
    fn overrides_create_private_independent_paths_and_stable_config() {
        let _lock = ENVIRONMENT.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("settings").join("chat.toml");
        let data_path = dir.path().join("state");
        let _environment = Environment::set(&config_path, &data_path);

        let paths = Paths::discover().unwrap();
        assert_eq!(paths.config, config_path);
        assert_eq!(paths.database, data_path.join("chat.sqlite3"));
        assert!(paths.socket.as_os_str().as_bytes().len() < 108);
        assert_eq!(paths.database.parent(), paths.lock.parent());

        let first = Config::load_or_create(&paths.config).unwrap();
        let second = Config::load_or_create(&paths.config).unwrap();
        assert_eq!(first, second);
        uuid::Uuid::parse_str(&first.machine).unwrap();
        assert_eq!(first.schema_version, 1);
        assert_eq!(first.limits().inline_bytes, DEFAULT_LIMITS.inline_bytes);
        assert_eq!(first.limits().batch_bytes, DEFAULT_LIMITS.batch_bytes);

        for directory in [
            paths.config.parent().unwrap(),
            data_path.as_path(),
            paths.socket.parent().unwrap(),
        ] {
            let metadata = directory.symlink_metadata().unwrap();
            assert_eq!(metadata.mode() & 0o777, 0o700);
            assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        }
        let metadata = paths.config.symlink_metadata().unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        let raw = std::fs::read_to_string(&paths.config).unwrap();
        assert!(raw.contains("schema_version = 1"));
        assert!(raw.contains("machine ="));
        assert!(!raw.contains("token"));
        assert!(!raw.contains("server"));

        std::fs::remove_dir(paths.socket.parent().unwrap()).unwrap();
    }

    #[test]
    fn unsafe_symlinked_paths_are_rejected() {
        let _lock = ENVIRONMENT.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let actual = dir.path().join("actual");
        std::fs::create_dir(&actual).unwrap();
        let linked = dir.path().join("linked");
        symlink(&actual, &linked).unwrap();
        let _environment = Environment::set(&dir.path().join("chat.toml"), &linked);

        assert!(Paths::discover().is_err());
    }

    #[test]
    fn unsafe_existing_config_permissions_are_rejected() {
        let _lock = ENVIRONMENT.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("chat.toml");
        std::fs::write(&config_path, "schema_version = 1\n").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _environment = Environment::set(&config_path, &dir.path().join("data"));

        assert!(Paths::discover().is_err());
    }

    #[test]
    fn non_sticky_world_writable_override_ancestor_is_rejected() {
        let _lock = ENVIRONMENT.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let unsafe_parent = dir.path().join("replaceable");
        std::fs::create_dir(&unsafe_parent).unwrap();
        std::fs::set_permissions(&unsafe_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let _environment = Environment::set(
            &unsafe_parent.join("config").join("chat.toml"),
            &unsafe_parent.join("data"),
        );

        let error = Paths::discover().unwrap_err();
        assert!(format!("{error:#}").contains("writable ancestor"));
    }

    #[test]
    fn ancestor_policy_rejects_third_party_owners_including_sticky_directories() {
        let current_uid = 1000;
        assert!(ancestor_is_trusted(0, 0o040755, current_uid));
        assert!(ancestor_is_trusted(current_uid, 0o040700, current_uid));
        assert!(ancestor_is_trusted(0, 0o041777, current_uid));
        assert!(!ancestor_is_trusted(2000, 0o040755, current_uid));
        assert!(!ancestor_is_trusted(2000, 0o041777, current_uid));
        assert!(!ancestor_is_trusted(current_uid, 0o040777, current_uid));
    }

    #[test]
    fn default_config_and_data_paths_receive_complete_ancestry_checks() {
        let dir = tempfile::tempdir().unwrap();
        let unsafe_parent = dir.path().join("replaceable-default");
        std::fs::create_dir(&unsafe_parent).unwrap();
        std::fs::set_permissions(&unsafe_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let runtime = dir.path().join("runtime");

        let error = build_paths(
            unsafe_parent.join("config").join("chat.toml"),
            unsafe_parent.join("data"),
            runtime,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("writable ancestor"));
    }

    #[test]
    fn runtime_path_receives_complete_ancestry_checks() {
        let dir = tempfile::tempdir().unwrap();
        let unsafe_runtime = dir.path().join("replaceable-runtime");
        std::fs::create_dir(&unsafe_runtime).unwrap();
        std::fs::set_permissions(&unsafe_runtime, std::fs::Permissions::from_mode(0o777)).unwrap();

        let error = build_paths(
            dir.path().join("config").join("chat.toml"),
            dir.path().join("data"),
            unsafe_runtime,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("writable ancestor"));
    }

    #[test]
    fn portable_socket_boundary_reserves_the_nul_terminator() {
        let maximum = std::path::PathBuf::from("x".repeat(103));
        let too_long = std::path::PathBuf::from("x".repeat(104));

        validate_socket_path(&maximum).unwrap();
        assert!(validate_socket_path(&too_long).is_err());
    }

    #[test]
    fn equivalent_data_path_spellings_share_one_runtime_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let physical = dir.path().join("state");
        let direct = build_paths(
            dir.path().join("config-a").join("chat.toml"),
            physical.clone(),
            runtime.clone(),
        )
        .unwrap();
        let aliased = build_paths(
            dir.path().join("config-b").join("chat.toml"),
            physical.join("."),
            runtime,
        )
        .unwrap();

        assert_eq!(direct.database, aliased.database);
        assert_eq!(direct.socket, aliased.socket);
        assert_eq!(direct.lock, aliased.lock);
    }

    #[test]
    fn same_relative_override_from_distinct_roots_gets_distinct_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let first_root = dir.path().join("first");
        let second_root = dir.path().join("second");
        std::fs::create_dir(&first_root).unwrap();
        std::fs::create_dir(&second_root).unwrap();
        let runtime = dir.path().join("runtime");
        let first_data = anchor_override(std::path::PathBuf::from("state"), &first_root);
        let second_data = anchor_override(std::path::PathBuf::from("state"), &second_root);
        let first = build_paths(
            first_root.join("config").join("chat.toml"),
            first_data,
            runtime.clone(),
        )
        .unwrap();
        let second = build_paths(
            second_root.join("config").join("chat.toml"),
            second_data,
            runtime,
        )
        .unwrap();

        assert_ne!(first.database, second.database);
        assert_ne!(first.socket, second.socket);
        assert_ne!(first.lock, second.lock);
    }

    #[test]
    fn relative_override_validates_the_current_directory_ancestry() {
        let dir = tempfile::tempdir().unwrap();
        let unsafe_root = dir.path().join("replaceable-cwd");
        std::fs::create_dir(&unsafe_root).unwrap();
        std::fs::set_permissions(&unsafe_root, std::fs::Permissions::from_mode(0o777)).unwrap();
        let data = anchor_override(std::path::PathBuf::from("state"), &unsafe_root);

        let error = build_paths(
            dir.path().join("config").join("chat.toml"),
            data,
            dir.path().join("runtime"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("writable ancestor"));
    }

    #[test]
    fn explicit_path_does_not_evaluate_a_failing_platform_fallback() {
        let explicit = std::ffi::OsString::from("isolated/chat.toml");
        let selected = choose_path(Some(explicit.clone()), || {
            anyhow::bail!("platform directory unavailable")
        })
        .unwrap();

        assert_eq!(selected, std::path::PathBuf::from(explicit));
    }
}

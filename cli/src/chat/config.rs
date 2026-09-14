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
}

impl Config {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let config = Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            machine: uuid::Uuid::new_v4().to_string(),
            inline_bytes: DEFAULT_LIMITS.inline_bytes,
            batch_bytes: DEFAULT_LIMITS.batch_bytes,
        };
        config.create_or_load(path)
    }

    pub fn limits(&self) -> Limits {
        Limits {
            inline_bytes: self.inline_bytes,
            batch_bytes: self.batch_bytes,
        }
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
        Ok(config)
    }

    fn create_or_load(self, path: &Path) -> Result<Self> {
        validate_limits(self.limits())?;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub database: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let config_override = std::env::var_os("LAM_CHAT_CONFIG");
        let data_override = std::env::var_os("LAM_CHAT_DATA_DIR");
        let config = config_override.as_ref().map(PathBuf::from).unwrap_or(
            dirs::config_dir()
                .context("no platform config directory")?
                .join("lam-chat")
                .join("chat.toml"),
        );
        let data = data_override.as_ref().map(PathBuf::from).unwrap_or(
            dirs::data_local_dir()
                .context("no platform data directory")?
                .join("lam-chat"),
        );

        if config_override.is_some() {
            validate_override_components(&config)?;
        }
        if data_override.is_some() {
            validate_override_components(&data)?;
        }
        let config_directory = config
            .parent()
            .context("LAM_CHAT_CONFIG must name a file in a directory")?;
        ensure_private_dir(config_directory, "Chat config directory")?;
        ensure_private_dir(&data, "Chat data directory")?;
        if config.exists() {
            validate_private_file(&config, "Chat config")?;
        }

        let runtime_base = runtime_base()?;
        let runtime_root = runtime_base.join(runtime_root_name());
        ensure_private_dir(&runtime_root, "Chat runtime directory")?;
        let namespace = hex_prefix(&Sha256::digest(data.as_os_str().as_encoded_bytes()));
        let runtime = runtime_root.join(namespace);
        ensure_private_dir(&runtime, "Chat runtime namespace")?;
        let socket = runtime.join("chat.sock");
        ensure!(
            socket.as_os_str().as_encoded_bytes().len() < 108,
            "Chat socket path is too long for a Unix socket: {}",
            socket.display()
        );

        Ok(Self {
            config,
            database: data.join("chat.sqlite3"),
            socket,
            lock: runtime.join("chat.lock"),
        })
    }
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
    Ok(())
}

fn hex_prefix(digest: &[u8]) -> String {
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_override_components(path: &Path) -> Result<()> {
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
                    "Chat override cannot traverse symlink {}; choose a direct path",
                    current.display()
                );
                validate_ancestor_permissions(&current, &metadata)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_ancestor_permissions(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.file_type().is_dir()
        && metadata.mode() & 0o022 != 0
        && metadata.mode() & 0o1000 == 0
    {
        anyhow::bail!(
            "Chat override has writable ancestor {}; remove group/world write access or use a sticky temporary directory",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_ancestor_permissions(_path: &Path, _metadata: &std::fs::Metadata) -> Result<()> {
    Ok(())
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
fn ensure_private_dir(path: &Path, label: &str) -> Result<()> {
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
fn ensure_private_dir(path: &Path, label: &str) -> Result<()> {
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
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::sync::Mutex;

    use super::{validate_limits, Config, Paths, DEFAULT_LIMITS};
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
        validate_limits(Limits {
            inline_bytes: 1,
            batch_bytes: 1,
        })
        .unwrap();
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
        assert_eq!(paths.socket.parent(), paths.lock.parent());

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
}

use std::collections::HashSet;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::client::{
    ArticleAsset, ArticleAssetDisposition, ArticleDraft, ArticlePublishError, ArticleReadResult,
    Client,
};
use crate::config::Config;

const MAX_ENTRY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 20 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ASSETS: usize = 50;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ReadFilter {
    All,
    Read,
    Unread,
}

impl ReadFilter {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Read => "read",
            Self::Unread => "unread",
        }
    }
}

pub struct PublishArgs {
    pub file: PathBuf,
    pub title: String,
    pub summary: String,
    pub assets: Vec<String>,
    pub silent: bool,
}

struct UploadAsset {
    metadata: ArticleAsset,
    bytes: Vec<u8>,
}

fn client() -> Result<Client> {
    Client::new(&Config::load()?)
}

fn validate_text(args: &PublishArgs) -> Result<()> {
    if args.title.trim().is_empty() {
        bail!("--title must not be blank");
    }
    if args.title.chars().count() > 200 {
        bail!("--title must be at most 200 characters");
    }
    if args.summary.chars().count() > 2_000 {
        bail!("--summary must be at most 2000 characters");
    }
    if args.assets.len() > MAX_ASSETS {
        bail!("at most 50 additional assets are allowed");
    }
    Ok(())
}

fn normalize_asset_path(path: &str) -> Result<String> {
    if path.is_empty()
        || path.contains('\\')
        || path.contains('%')
        || path.contains("//")
        || path
            .chars()
            .any(|character| character.is_control() || matches!(character, ':' | '?' | '#'))
    {
        bail!("invalid asset path {path:?}");
    }
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("asset paths must be relative and cannot contain traversal: {path:?}");
    }
    if path.encode_utf16().count() > 512 {
        bail!("asset path is longer than 512 UTF-16 code units");
    }
    let normalized = path.nfc().collect::<String>();
    if normalized == "index.html" {
        bail!("--asset index.html conflicts with the normalized entry path");
    }
    if normalized.encode_utf16().count() > 512 {
        bail!("asset path is longer than 512 UTF-16 code units");
    }
    Ok(normalized)
}

fn asset_type(path: &str) -> Result<(&'static str, ArticleAssetDisposition)> {
    let extension = Path::new(path)
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => Ok(("image/png", ArticleAssetDisposition::Inline)),
        "jpg" | "jpeg" => Ok(("image/jpeg", ArticleAssetDisposition::Inline)),
        "webp" => Ok(("image/webp", ArticleAssetDisposition::Inline)),
        "pdf" => Ok(("application/pdf", ArticleAssetDisposition::Attachment)),
        "txt" => Ok(("text/plain", ArticleAssetDisposition::Attachment)),
        _ => bail!("unsupported asset type for {path:?}; use PNG, JPEG, WebP, PDF, or text"),
    }
}

#[cfg(unix)]
struct SecureDirectory {
    file: File,
}

#[cfg(unix)]
impl SecureDirectory {
    fn for_entry(path: &Path) -> Result<(Self, OsStringLeaf)> {
        use std::os::fd::AsRawFd;

        let leaf = path
            .file_name()
            .filter(|name| !name.is_empty())
            .map(OsStringLeaf::new)
            .context("--file must name an HTML file")?;
        let mut directory = if path.is_absolute() {
            File::open("/")?
        } else {
            File::open(".")?
        };
        if let Some(parent) = path.parent() {
            for component in parent.components() {
                match component {
                    Component::RootDir | Component::CurDir => continue,
                    Component::ParentDir => {
                        directory = openat_directory(directory.as_raw_fd(), OsStr::new(".."))?
                    }
                    Component::Normal(name) => {
                        directory = openat_directory(directory.as_raw_fd(), name)?
                    }
                    Component::Prefix(_) => bail!("unsupported --file path"),
                }
            }
        }
        Ok((Self { file: directory }, leaf))
    }

    fn open(&self, path: &Path) -> Result<File> {
        use std::os::fd::AsRawFd;

        let mut directory = self.file.try_clone()?;
        let mut components = path.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                bail!("asset paths must be relative and cannot contain traversal");
            };
            if components.peek().is_some() {
                directory = openat_directory(directory.as_raw_fd(), name)?;
            } else {
                return openat_file(directory.as_raw_fd(), name);
            }
        }
        bail!("path does not name a file")
    }
}

struct OsStringLeaf(std::ffi::OsString);

impl OsStringLeaf {
    fn new(value: &OsStr) -> Self {
        Self(value.to_os_string())
    }

    fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

#[cfg(not(unix))]
struct SecureDirectory;

#[cfg(not(unix))]
impl SecureDirectory {
    fn for_entry(_path: &Path) -> Result<(Self, OsStringLeaf)> {
        bail!("secure article publishing is not supported on this operating system")
    }

    fn open(&self, _path: &Path) -> Result<File> {
        bail!("secure article publishing is not supported on this operating system")
    }
}

#[cfg(unix)]
fn c_path(path: &OsStr) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt;

    CString::new(path.as_bytes()).context("path contains a NUL byte")
}

#[cfg(unix)]
fn openat_directory(parent: std::os::fd::RawFd, name: &OsStr) -> Result<File> {
    use std::os::fd::FromRawFd;

    let name = c_path(name)?;
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ELOOP) || error.raw_os_error() == Some(libc::ENOTDIR)
        {
            bail!("refusing symlink in article path");
        }
        return Err(error).context("cannot open article directory");
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn openat_file(parent: std::os::fd::RawFd, name: &OsStr) -> Result<File> {
    use std::os::fd::FromRawFd;

    let name = c_path(name)?;
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ELOOP) {
            bail!("refusing symlink in article path");
        }
        return Err(error).context("cannot open article file");
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if !file.metadata()?.file_type().is_file() {
        bail!("article paths must name regular files");
    }
    Ok(file)
}

fn snapshot(mut file: File, path: &str, limit: u64, label: &str) -> Result<UploadAssetBytes> {
    let declared = file.metadata()?.len();
    if declared > limit {
        bail!(
            "{label} {path:?} exceeds {limit_label}",
            limit_label = size_label(limit)
        );
    }
    let mut bytes = Vec::with_capacity(declared as usize);
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if bytes.len() as u64 + read as u64 > limit {
            bail!(
                "{label} {path:?} exceeds {limit_label}",
                limit_label = size_label(limit)
            );
        }
        hash.update(&buffer[..read]);
        bytes.extend_from_slice(&buffer[..read]);
    }
    let digest = hash.finalize();
    let sha256 = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(UploadAssetBytes { bytes, sha256 })
}

struct UploadAssetBytes {
    bytes: Vec<u8>,
    sha256: String,
}

fn size_label(limit: u64) -> &'static str {
    match limit {
        MAX_ENTRY_BYTES => "2 MiB",
        MAX_ASSET_BYTES => "20 MiB",
        _ => "the byte limit",
    }
}

fn build_assets(args: &PublishArgs) -> Result<Vec<UploadAsset>> {
    let (directory, entry_leaf) = SecureDirectory::for_entry(&args.file)?;
    let entry = snapshot(
        directory.open(entry_leaf.as_path())?,
        &args.file.display().to_string(),
        MAX_ENTRY_BYTES,
        "entry",
    )?;
    let mut total = entry.bytes.len() as u64;
    let mut uploads = vec![UploadAsset {
        metadata: ArticleAsset {
            path: "index.html".into(),
            media_type: "text/html".into(),
            size: entry.bytes.len() as u64,
            sha256: entry.sha256,
            disposition: ArticleAssetDisposition::Inline,
        },
        bytes: entry.bytes,
    }];
    let mut paths = HashSet::new();
    for raw_path in &args.assets {
        let path = normalize_asset_path(raw_path)?;
        if !paths.insert(path.clone()) {
            bail!("duplicate asset path after normalization: {path:?}");
        }
        let (media_type, disposition) = asset_type(&path)?;
        let snapshot = snapshot(
            directory.open(Path::new(raw_path))?,
            raw_path,
            MAX_ASSET_BYTES,
            "asset",
        )?;
        total += snapshot.bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            bail!("article bundle exceeds 50 MiB total");
        }
        uploads.push(UploadAsset {
            metadata: ArticleAsset {
                path,
                media_type: media_type.into(),
                size: snapshot.bytes.len() as u64,
                sha256: snapshot.sha256,
                disposition,
            },
            bytes: snapshot.bytes,
        });
    }
    Ok(uploads)
}

fn project_name() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    let root = cwd
        .ancestors()
        .find(|path| path.join(".git").exists())
        .map(Path::to_path_buf)
        .unwrap_or(cwd);
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn publish(args: PublishArgs) -> Result<i32> {
    validate_text(&args)?;
    let uploads = build_assets(&args)?;
    let draft = ArticleDraft {
        title: args.title,
        summary: args.summary,
        name: crate::name::resolve(None)?,
        source_host: hostname::get()?.to_string_lossy().into_owned(),
        source_project: project_name(),
        silent: args.silent,
        assets: uploads.iter().map(|asset| asset.metadata.clone()).collect(),
    };
    let client = client()?;
    let idempotency_key = uuid::Uuid::new_v4().to_string();
    let id = client.create_article_upload(&draft, &idempotency_key)?;
    for (index, asset) in uploads.iter().enumerate() {
        if let Err(error) = client
            .upload_article_asset(&id, index, &asset.bytes)
            .with_context(|| format!("asset upload {index} failed"))
        {
            return Err(unpublished_error(&id, error));
        }
    }
    match client.publish_article(&id) {
        Ok(article) => {
            println!("{}", article.id);
            Ok(0)
        }
        Err(ArticlePublishError::Rejected(error)) => Err(unpublished_error(&id, error)),
        Err(ArticlePublishError::OutcomeUnknown) => Err(anyhow!(
            "article draft {id} publication outcome is unknown; the server may have published it; check `lam article list` before starting another publication"
        )),
    }
}

fn unpublished_error(id: &str, error: anyhow::Error) -> anyhow::Error {
    let detail = format!("{error:#}").replace(id, "[draft]");
    anyhow!("article draft {id} was not published: {detail}")
}

pub fn list(read: ReadFilter, query: Option<&str>) -> Result<i32> {
    let client = client()?;
    let mut cursor = None;
    loop {
        let page = client.article_page(read.as_str(), query, cursor.as_deref())?;
        for article in page.items {
            println!(
                "{} {:<6} {:<24} {}",
                article.id,
                if article.read_at.is_some() {
                    "read"
                } else {
                    "unread"
                },
                article.name,
                article.title
            );
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(0),
        }
    }
}

pub fn set_read(id: &str, read: bool) -> Result<i32> {
    let client = client()?;
    let current = client.article(id)?;
    match client.set_article_read(id, read, current.version)? {
        ArticleReadResult::Updated(article) => {
            println!("{}", serde_json::to_string_pretty(&article)?);
            Ok(0)
        }
        ArticleReadResult::Conflict => {
            let canonical = client.article(id).context(
                "article read update conflicted and canonical state could not be fetched",
            )?;
            bail!(
                "article read update conflicted; canonical state:\n{}",
                serde_json::to_string_pretty(&canonical)?
            )
        }
    }
}

fn validated_viewer_url(raw: &str) -> Result<String> {
    if raw.contains('\\')
        || raw
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        bail!("server returned an invalid article viewer URL");
    }
    let url = reqwest::Url::parse(raw)
        .map_err(|_| anyhow!("server returned an invalid article viewer URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("server returned an invalid article viewer URL");
    }
    Ok(url.to_string())
}

pub fn open(id: &str) -> Result<i32> {
    open_with_client(&client()?, id)?;
    Ok(0)
}

pub(crate) fn open_with_client(client: &Client, id: &str) -> Result<()> {
    let session = client.create_article_view_session(id)?;
    let _ = &session.expires_at;
    let viewer_url = validated_viewer_url(&session.url)?;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(viewer_url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("could not launch the article viewer")?;
    Ok(())
}

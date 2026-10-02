//! Optional Windows x64 engine updates. Checking never downloads executable code.
//! Only publisher HTTPS endpoints are accepted; an update is activated after all checks pass.

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const ORIGIN: &str = "https://www.gyan.dev";
const VERSION_URL: &str = "https://www.gyan.dev/ffmpeg/builds/release-version";
const MAX_DOWNLOAD: u64 = 350 * 1024 * 1024;
const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;
const MAX_ENTRY: u64 = 300 * 1024 * 1024;
const MAX_ENTRIES: usize = 8192;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    archive_url: String,
    sha256: String,
}

#[cfg(test)]
pub(crate) fn test_release(version: &str) -> Release {
    Release {
        version: version.to_string(),
        archive_url: archive_url(version),
        sha256: "0".repeat(64),
    }
}

#[derive(Serialize, Deserialize)]
struct ActiveEngine {
    directory: String,
    version: String,
    sha256: String,
}

pub fn supported() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

fn require_supported() -> Result<(), String> {
    supported()
        .then_some(())
        .ok_or_else(|| "自动更新目前支持 Windows x64。可在设置中选择自己的 FFmpeg。".into())
}

fn version_parts(value: &str) -> Option<Vec<u64>> {
    let value = value.trim();
    let value = value
        .strip_prefix("ffmpeg version ")
        .or_else(|| value.strip_prefix("ffprobe version "))
        .unwrap_or(value);
    let token = value.split_whitespace().next()?;
    // Do not treat nightly, prerelease, or unknown builds as stable versions.
    let token = token.split_once("-essentials_build").map_or(token, |p| p.0);
    let token = token.split_once("-full_build").map_or(token, |p| p.0);
    if token.len() > 32 || !token.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return None;
    }
    let parts: Vec<u64> = token
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    (parts.len() >= 2 && parts.len() <= 4 && parts[0] > 0).then_some(parts)
}

pub fn short_version(value: &str) -> String {
    if let Some(parts) = version_parts(value) {
        return parts
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".");
    }
    let value = value.trim();
    value
        .strip_prefix("ffmpeg version ")
        .or_else(|| value.strip_prefix("ffprobe version "))
        .unwrap_or(value)
        .split_whitespace()
        .next()
        .unwrap_or("未知版本")
        .split("-www.")
        .next()
        .unwrap_or("未知版本")
        .to_string()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_parts(candidate), version_parts(current)) {
        (Some(mut candidate), Some(mut current)) => {
            candidate.resize(4, 0);
            current.resize(4, 0);
            candidate > current
        }
        _ => false,
    }
}

fn stable_version(value: &str) -> Result<String, String> {
    let value = value.trim();
    if version_parts(value).is_none() || !value.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return Err("更新服务器返回了无效的稳定版本号。".into());
    }
    Ok(value.to_string())
}

fn trusted_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("www.gyan.dev")
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.path().starts_with("/ffmpeg/builds/")
        && url.query().is_none()
        && url.fragment().is_none()
}

fn client() -> Result<Client, String> {
    Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(15 * 60))
        .user_agent(concat!("Frameflow/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || !trusted_url(attempt.url()) {
                attempt.error("untrusted FFmpeg redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| format!("无法创建更新连接：{e}"))
}

fn metadata(client: &Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("无法检查 FFmpeg 更新：{e}"))?;
    let mut bytes = Vec::new();
    response
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("更新信息超出允许大小。".into());
    }
    String::from_utf8(bytes).map_err(|_| "更新信息不是有效文本。".into())
}

fn parse_sha256(value: &str) -> Result<String, String> {
    let digest = value.split_whitespace().next().unwrap_or_default();
    if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("发布方未提供有效的 SHA-256 校验值。".into());
    }
    Ok(digest.to_ascii_lowercase())
}

fn archive_url(version: &str) -> String {
    format!("{ORIGIN}/ffmpeg/builds/packages/ffmpeg-{version}-essentials_build.zip")
}

pub fn latest_release() -> Result<Release, String> {
    require_supported()?;
    let client = client()?;
    let version = stable_version(&metadata(&client, VERSION_URL)?)?;
    // Pin both URLs to the same immutable version, rather than racing two latest aliases.
    let archive_url = archive_url(&version);
    let sha256 = parse_sha256(&metadata(&client, &format!("{archive_url}.sha256"))?)?;
    Ok(Release {
        version,
        archive_url,
        sha256,
    })
}

fn engine_root() -> Option<PathBuf> {
    let path = supported().then(|| std::env::var_os("LOCALAPPDATA"))??;
    engine_root_from_local(Path::new(&path))
}

fn engine_root_from_local(local: &Path) -> Option<PathBuf> {
    local.is_absolute().then(|| local.join("Frameflow/engines"))
}

fn active_engine(root: &Path) -> Option<ActiveEngine> {
    let file = File::open(root.join("active.json")).ok()?;
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let active: ActiveEngine = serde_json::from_slice(&bytes).ok()?;
    if active.directory.is_empty()
        || active.directory.len() > 96
        || !active
            .directory
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
        || active.directory.starts_with('.')
        || stable_version(&active.version).is_err()
        || parse_sha256(&active.sha256).is_err()
    {
        return None;
    }
    Some(active)
}

fn managed_engine_at(root: &Path) -> Option<PathBuf> {
    let active = active_engine(root)?;
    let directory = root.join(active.directory).join("bin");
    let root = fs::canonicalize(root).ok()?;
    let canonical = fs::canonicalize(&directory).ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }
    for name in ["ffmpeg.exe", "ffprobe.exe"] {
        let path = directory.join(name);
        let metadata = fs::symlink_metadata(&path).ok()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return None;
        }
    }
    Some(directory)
}

/// Returns only an installed, selected engine; never a prospective destination.
pub fn managed_engine_dir() -> Option<PathBuf> {
    managed_engine_at(&engine_root()?)
}

fn unique_suffix() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(root) = self.0.parent() {
            let _ = remove_staging(root, &self.0);
        }
    }
}

fn has_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn staging_name(name: &str) -> bool {
    name.strip_prefix(".staging-").is_some_and(|suffix| {
        let parts: Vec<_> = suffix.split('-').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
    })
}

fn no_links_in_tree(path: &Path) -> Result<bool, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if has_link(&metadata) {
        return Ok(false);
    }
    if metadata.is_dir() {
        for item in fs::read_dir(path).map_err(|e| e.to_string())? {
            if !no_links_in_tree(&item.map_err(|e| e.to_string())?.path())? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn remove_staging(root: &Path, path: &Path) -> Result<(), String> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    if !staging_name(name) || !path.exists() || !no_links_in_tree(path)? {
        return Ok(());
    }
    let canonical_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let canonical_path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if canonical_path.parent() != Some(canonical_root.as_path()) {
        return Ok(());
    }
    fs::remove_dir_all(path).map_err(|e| format!("无法清理上次中断的下载：{e}"))
}

fn clean_abandoned_staging(root: &Path) -> Result<(), String> {
    // Called only with the exclusive updater lock; no other installer can own these.
    for item in fs::read_dir(root).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        if item.file_type().map_err(|e| e.to_string())?.is_dir() {
            remove_staging(root, &item.path())?;
        }
    }
    Ok(())
}

fn update_lock(root: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options
        .open(root.join("update.lock"))
        .map_err(|error| update_lock_error(root, &error))
}

fn engine_storage_error(action: &str, path: &Path, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        format!(
            "{action}：当前用户没有引擎目录的访问权限（{}）。请检查该用户目录的权限。",
            path.display()
        )
    } else {
        format!("{action}（{}）：{error}", path.display())
    }
}

fn update_lock_error(root: &Path, error: &std::io::Error) -> String {
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return "另一个窗口正在更新引擎，请等待该更新完成后重试。".into();
    }
    engine_storage_error("无法锁定引擎更新目录", root, error)
}

fn download_archive(
    mut input: impl Read,
    path: &Path,
    sha256: &str,
    progress: &impl Fn(String),
) -> Result<(), String> {
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut reported = 0;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|e| format!("下载中断：{e}"))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_DOWNLOAD {
            return Err("下载文件超出允许大小。".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| format!("无法保存下载文件：{e}"))?;
        digest.update(&buffer[..count]);
        let megabytes = total / (1024 * 1024);
        if megabytes >= reported + 2 {
            reported = megabytes;
            progress(format!("已下载 {megabytes} MB"));
        }
    }
    output.sync_all().map_err(|e| e.to_string())?;
    progress("正在校验下载文件…".into());
    if format!("{:x}", digest.finalize()) != sha256 {
        return Err("SHA-256 校验不一致，已取消更新；现有引擎保持不变。".into());
    }
    Ok(())
}

fn safe_zip_components(name: &str) -> Result<Vec<&str>, String> {
    // Treat both separator styles as dangerous, independent of the host OS.
    if name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
        || name.starts_with('/')
        || name.len() > 240
    {
        return Err("更新压缩包包含不安全的路径。".into());
    }
    let components: Vec<_> = name.trim_end_matches('/').split('/').collect();
    for component in &components {
        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if component.is_empty()
            || *component == "."
            || *component == ".."
            || component.ends_with([' ', '.'])
            || component.chars().any(|c| c.is_control())
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err("更新压缩包包含不安全的路径。".into());
        }
    }
    Ok(components)
}

fn extract_archive(path: &Path, destination: &Path, version: &str) -> Result<(), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("下载文件不是有效 ZIP：{e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err("更新压缩包的文件数量异常。".into());
    }
    let expected_root = format!("ffmpeg-{version}-essentials_build");
    let mut total = 0_u64;
    let mut names = HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().to_owned();
        let components = safe_zip_components(&name)?;
        if components.first() != Some(&expected_root.as_str()) {
            return Err("更新压缩包的根目录与版本不符。".into());
        }
        if !names.insert(name.trim_end_matches('/').to_ascii_lowercase()) {
            return Err("更新压缩包包含重复路径。".into());
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                return Err("更新压缩包包含链接或特殊文件。".into());
            }
        }
        total = total.checked_add(entry.size()).ok_or("解压体积溢出。")?;
        if entry.size() > MAX_ENTRY || total > MAX_EXPANDED {
            return Err("更新压缩包的解压体积超出限制。".into());
        }
        let relative: PathBuf = components.iter().skip(1).collect();
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output).map_err(|e| e.to_string())?;
        } else {
            if components.len() < 2 {
                return Err("更新压缩包缺少文件路径。".into());
            }
            fs::create_dir_all(output.parent().ok_or("无效解压路径。")?)
                .map_err(|e| e.to_string())?;
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&output)
                .map_err(|e| e.to_string())?;
            let expected = entry.size();
            let copied = std::io::copy(&mut (&mut entry).take(MAX_ENTRY + 1), &mut file)
                .map_err(|e| e.to_string())?;
            if copied != expected || copied > MAX_ENTRY {
                return Err("更新压缩包文件大小不符。".into());
            }
            file.sync_all().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn validate_x64(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| format!("更新包缺少 {}：{e}", path.display()))?;
    let mut dos = [0; 64];
    file.read_exact(&mut dos).map_err(|e| e.to_string())?;
    if &dos[..2] != b"MZ" {
        return Err("更新包中的工具不是 Windows 程序。".into());
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    if offset
        > file
            .metadata()
            .map_err(|e| e.to_string())?
            .len()
            .saturating_sub(6)
    {
        return Err("更新程序的 PE 头无效。".into());
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    let mut pe = [0; 6];
    file.read_exact(&mut pe).map_err(|e| e.to_string())?;
    if &pe[..4] != b"PE\0\0" || u16::from_le_bytes([pe[4], pe[5]]) != 0x8664 {
        return Err("更新包不包含 Windows x64 工具。".into());
    }
    Ok(())
}

pub(crate) fn executable_version(path: &Path) -> Result<String, String> {
    let mut command = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("无法运行 FFmpeg 工具：{e}"))?;
    let stdout = child.stdout.take().ok_or("无法读取版本输出。")?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(65537).read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < Duration::from_secs(15) => {
                thread::sleep(Duration::from_millis(25))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("FFmpeg 工具响应超时。".to_string());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(e.to_string());
            }
        }
    };
    let bytes = receiver
        .recv_timeout(Duration::from_secs(15).saturating_sub(start.elapsed()))
        .map_err(|_| "版本读取超时或失败。")?
        .map_err(|e| e.to_string())?;
    if !status?.success() || bytes.len() > 65536 {
        return Err("FFmpeg 工具未通过运行验证。".into());
    }
    let output = String::from_utf8_lossy(&bytes);
    output
        .lines()
        .next()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "FFmpeg 工具未返回版本信息。".into())
}

fn validate_engine(directory: &Path, version: &str) -> Result<(), String> {
    let mut versions = Vec::new();
    for name in ["ffmpeg", "ffprobe"] {
        let path = directory.join(format!("{name}.exe"));
        validate_x64(&path)?;
        let line = executable_version(&path)?;
        if !line.starts_with(&format!("{name} version ")) || short_version(&line) != version {
            return Err("下载引擎的实际版本与发布版本不一致。".into());
        }
        versions.push(
            line.split_whitespace()
                .nth(2)
                .unwrap_or_default()
                .to_string(),
        );
    }
    if versions[0] != versions[1] {
        return Err("FFmpeg 和 FFprobe 的版本不一致。".into());
    }
    Ok(())
}

fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // Both NUL-terminated buffers remain alive for the synchronous Windows call.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(format!(
                "无法启用新引擎：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fs::rename(source, destination).map_err(|e| format!("无法启用新引擎：{e}"))
}

fn save_engine_directory(source: &Path, destination: &Path) -> Result<(), String> {
    // Windows can briefly retain scanner/image handles after executable validation.
    // Retry only sharing/access failures; never replace an existing version.
    let started = Instant::now();
    loop {
        match fs::symlink_metadata(destination) {
            Ok(_) => return Err("新引擎目标目录已存在，取消更新。".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法检查新引擎目录：{error}")),
        }
        match fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && matches!(error.raw_os_error(), Some(5 | 32 | 33))
                    && started.elapsed() < Duration::from_secs(5) =>
            {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("无法保存新引擎：{error}")),
        }
    }
}

fn install_from_reader(
    release: &Release,
    root: &Path,
    input: impl Read,
    progress: &impl Fn(String),
    validate: impl Fn(&Path, &str) -> Result<(), String>,
) -> Result<PathBuf, String> {
    stable_version(&release.version)?;
    parse_sha256(&release.sha256)?;
    if !root.is_absolute() {
        return Err("用户引擎目录必须为绝对路径。".into());
    }
    fs::create_dir_all(root)
        .map_err(|error| engine_storage_error("无法创建引擎目录", root, &error))?;
    let _lock = update_lock(root)?;
    clean_abandoned_staging(root)?;
    if let Some(active) = active_engine(root)
        && !is_newer(&release.version, &active.version)
    {
        return Err("当前已安装相同或更新版本，取消更新。".into());
    }
    let staging = Staging(root.join(format!(".staging-{}", unique_suffix())));
    fs::create_dir(&staging.0)
        .map_err(|error| engine_storage_error("无法创建引擎暂存目录", &staging.0, &error))?;
    let archive = staging.0.join("engine.zip");
    download_archive(input, &archive, &release.sha256, progress)?;
    progress("正在解压并验证引擎…".into());
    let payload = staging.0.join("payload");
    fs::create_dir(&payload).map_err(|e| e.to_string())?;
    extract_archive(&archive, &payload, &release.version)?;
    validate(&payload.join("bin"), &release.version)?;
    let directory = format!(
        "{}-{}-{}",
        release.version,
        &release.sha256[..12],
        unique_suffix()
    );
    let destination = root.join(&directory);
    let active = ActiveEngine {
        directory,
        version: release.version.clone(),
        sha256: release.sha256.clone(),
    };
    let manifest = serde_json::to_vec_pretty(&active).map_err(|e| e.to_string())?;
    fs::write(payload.join("FRAMEFLOW_ENGINE.json"), &manifest).map_err(|e| e.to_string())?;
    // A unique version directory avoids replacing any executable, including one still in use.
    save_engine_directory(&payload, &destination)?;
    let pending = staging.0.join("active.json");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&pending)
        .map_err(|e| e.to_string())?;
    file.write_all(&manifest)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    atomic_replace(&pending, &root.join("active.json"))?;
    progress(format!("FFmpeg {} 已就绪", release.version));
    Ok(destination.join("bin"))
}

fn install_at(
    release: &Release,
    root: &Path,
    progress: &impl Fn(String),
) -> Result<PathBuf, String> {
    stable_version(&release.version)?;
    if release.archive_url != archive_url(&release.version) {
        return Err("更新下载地址不可信。".into());
    }
    let response = client()?
        .get(&release.archive_url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("无法下载 FFmpeg：{e}"))?;
    if response.content_length().is_some_and(|n| n > MAX_DOWNLOAD) {
        return Err("下载文件超出允许大小。".into());
    }
    install_from_reader(release, root, response, progress, validate_engine)
}

pub fn install_release(release: &Release, progress: impl Fn(String)) -> Result<PathBuf, String> {
    require_supported()?;
    // Also prevent a stale UI check from replacing a newer bundled/default engine.
    if let Ok(tools) = crate::media::discover(None)
        && !is_newer(&release.version, &tools.version)
    {
        return Err("当前默认引擎已是相同或更新版本，取消更新。".into());
    }
    let root = engine_root().ok_or("无法确定当前用户的引擎目录。")?;
    progress(format!("正在下载 FFmpeg {}…", release.version));
    install_at(release, &root, &progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn engine_storage_requires_absolute_user_directory() {
        assert!(engine_root_from_local(Path::new("")).is_none());
        assert!(engine_root_from_local(Path::new("relative")).is_none());
        let local = std::env::temp_dir();
        assert_eq!(
            engine_root_from_local(&local),
            Some(local.join("Frameflow/engines"))
        );
    }

    #[test]
    fn lock_permission_error_is_not_reported_as_another_updater() {
        let root = std::env::temp_dir();
        let error = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let message = update_lock_error(&root, &error);
        assert!(message.contains("当前用户没有引擎目录的访问权限"));
        assert!(!message.contains("另一个窗口"));
    }

    #[cfg(windows)]
    #[test]
    fn lock_sharing_violation_is_reported_as_another_updater() {
        let root = std::env::temp_dir();
        let error = std::io::Error::from_raw_os_error(32);
        let message = update_lock_error(&root, &error);
        assert!(message.contains("另一个窗口正在更新引擎"));
        assert!(!message.contains("访问权限"));
    }
    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("frameflow-updater-test-{}", unique_suffix()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn release(bytes: &[u8]) -> Release {
        Release {
            version: "9.0.2".into(),
            archive_url: archive_url("9.0.2"),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }

    fn existing(root: &Path) -> Vec<u8> {
        fs::create_dir_all(root.join("9.0.1-existing/bin")).unwrap();
        for name in ["ffmpeg.exe", "ffprobe.exe"] {
            fs::write(root.join("9.0.1-existing/bin").join(name), b"old").unwrap();
        }
        let active = ActiveEngine {
            directory: "9.0.1-existing".into(),
            version: "9.0.1".into(),
            sha256: "a".repeat(64),
        };
        let bytes = serde_json::to_vec(&active).unwrap();
        fs::write(root.join("active.json"), &bytes).unwrap();
        bytes
    }

    #[test]
    fn versions_compare_numerically_without_downgrades() {
        assert!(is_newer(
            "9.0.2",
            "ffmpeg version 9.0.1-essentials_build-www.gyan.dev Copyright"
        ));
        assert!(is_newer("9.10", "9.9.5"));
        assert!(!is_newer("9.0.1", "9.0.2"));
        assert!(!is_newer("9.0.0", "9.0"));
        assert!(!is_newer("9.0.2", "N-20260930-git-build"));
        assert!(!is_newer("9.1-rc1", "9.0.2"));
        assert_eq!(
            short_version("ffprobe version 9.0.1-essentials_build-www.gyan.dev Copyright"),
            "9.0.1"
        );
        for version in [
            "../9.0",
            "9.0/abc",
            "9.0.2\nmalicious",
            "9..1",
            "0.0",
            "9.0-rc1",
        ] {
            assert!(stable_version(version).is_err());
        }
    }

    #[test]
    fn publisher_urls_and_hashes_are_strict() {
        for url in [
            "http://www.gyan.dev/ffmpeg/builds/x",
            "https://evil.test/ffmpeg/builds/x",
            "https://www.gyan.dev.evil.test/ffmpeg/builds/x",
            "https://www.gyan.dev:444/ffmpeg/builds/x",
            "https://user@www.gyan.dev/ffmpeg/builds/x",
        ] {
            assert!(!trusted_url(&reqwest::Url::parse(url).unwrap()));
        }
        assert!(trusted_url(&reqwest::Url::parse(VERSION_URL).unwrap()));
        assert_eq!(
            parse_sha256(&format!("{}  archive.zip", "A".repeat(64))).unwrap(),
            "a".repeat(64)
        );
        assert!(parse_sha256("bad digest").is_err());
    }

    #[test]
    fn wrong_hash_preserves_current_engine() {
        let root = TempRoot::new();
        let before = existing(&root.0);
        let bytes = fixture(&[("ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe", b"new")]);
        let mut release = release(&bytes);
        release.sha256 = "0".repeat(64);
        assert!(
            install_from_reader(
                &release,
                &root.0,
                Cursor::new(bytes),
                &|_| {},
                |_, _| panic!("must not execute unverified bytes")
            )
            .unwrap_err()
            .contains("SHA-256")
        );
        assert_eq!(fs::read(root.0.join("active.json")).unwrap(), before);
        assert_eq!(
            fs::read(managed_engine_at(&root.0).unwrap().join("ffmpeg.exe")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn unsafe_zip_and_validation_failure_preserve_current_engine() {
        for name in [
            "../escape",
            "ffmpeg-9.0.2-essentials_build/../escape",
            "C:/escape",
            "ffmpeg-9.0.2-essentials_build/bin\\..\\escape",
            "ffmpeg-9.0.2-essentials_build/NUL.txt",
            "ffmpeg-9.0.2-essentials_build/file:stream",
        ] {
            let root = TempRoot::new();
            let before = existing(&root.0);
            let bytes = fixture(&[(name, b"bad")]);
            assert!(
                install_from_reader(
                    &release(&bytes),
                    &root.0,
                    Cursor::new(bytes),
                    &|_| {},
                    |_, _| Ok(())
                )
                .is_err()
            );
            assert_eq!(fs::read(root.0.join("active.json")).unwrap(), before);
        }
        let root = TempRoot::new();
        let before = existing(&root.0);
        let bytes = fixture(&[("ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe", b"bad PE")]);
        assert!(
            install_from_reader(
                &release(&bytes),
                &root.0,
                Cursor::new(bytes),
                &|_| {},
                validate_engine
            )
            .is_err()
        );
        assert_eq!(fs::read(root.0.join("active.json")).unwrap(), before);
    }

    #[test]
    fn verified_fixture_switches_pointer_and_retains_previous_files() {
        let root = TempRoot::new();
        existing(&root.0);
        fs::create_dir(root.0.join(".staging-1-2-3")).unwrap();
        fs::write(root.0.join(".staging-1-2-3/interrupted.zip"), b"unfinished").unwrap();
        fs::create_dir(root.0.join(".staging-user-notes")).unwrap();
        let bytes = fixture(&[
            (
                "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe",
                b"new ffmpeg",
            ),
            (
                "ffmpeg-9.0.2-essentials_build/bin/ffprobe.exe",
                b"new ffprobe",
            ),
            ("ffmpeg-9.0.2-essentials_build/LICENSE", b"license"),
        ]);
        let release = release(&bytes);
        let directory = install_from_reader(
            &release,
            &root.0,
            Cursor::new(&bytes),
            &|_| {},
            |path, version| {
                assert_eq!(version, "9.0.2");
                assert_eq!(fs::read(path.join("ffprobe.exe")).unwrap(), b"new ffprobe");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(managed_engine_at(&root.0), Some(directory));
        assert!(!root.0.join(".staging-1-2-3").exists());
        assert!(root.0.join(".staging-user-notes").exists());
        assert_eq!(
            fs::read(root.0.join("9.0.1-existing/bin/ffmpeg.exe")).unwrap(),
            b"old"
        );
        let before = fs::read(root.0.join("active.json")).unwrap();
        assert!(
            install_from_reader(&release, &root.0, Cursor::new(&bytes), &|_| {}, |_, _| Ok(
                ()
            ))
            .is_err()
        );
        assert_eq!(fs::read(root.0.join("active.json")).unwrap(), before);
    }

    #[test]
    fn zip_size_and_duplicate_path_limits_are_enforced() {
        let root = TempRoot::new();
        let before = existing(&root.0);
        let mut oversized = fixture(&[("ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe", b"small")]);
        let header = oversized
            .windows(4)
            .position(|bytes| bytes == b"PK\x01\x02")
            .unwrap();
        oversized[header + 24..header + 28]
            .copy_from_slice(&((MAX_ENTRY + 1) as u32).to_le_bytes());
        assert!(
            install_from_reader(
                &release(&oversized),
                &root.0,
                Cursor::new(oversized),
                &|_| {},
                |_, _| panic!("oversized archive must not execute")
            )
            .is_err()
        );
        let duplicates = fixture(&[
            ("ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe", b"first"),
            ("ffmpeg-9.0.2-essentials_build/bin/FFMPEG.exe", b"second"),
        ]);
        assert!(
            install_from_reader(
                &release(&duplicates),
                &root.0,
                Cursor::new(duplicates),
                &|_| {},
                |_, _| panic!("duplicate archive must not execute")
            )
            .is_err()
        );
        assert_eq!(fs::read(root.0.join("active.json")).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn activation_failure_preserves_locked_pointer_and_previous_tools() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = TempRoot::new();
        let before = existing(&root.0);
        let lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.0.join("active.json"))
            .unwrap();
        let bytes = fixture(&[
            ("ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe", b"new"),
            ("ffmpeg-9.0.2-essentials_build/bin/ffprobe.exe", b"new"),
        ]);
        assert!(
            install_from_reader(
                &release(&bytes),
                &root.0,
                Cursor::new(bytes),
                &|_| {},
                |_, _| Ok(())
            )
            .unwrap_err()
            .contains("无法启用新引擎")
        );
        drop(lock);
        assert_eq!(fs::read(root.0.join("active.json")).unwrap(), before);
        assert_eq!(
            fs::read(managed_engine_at(&root.0).unwrap().join("ffmpeg.exe")).unwrap(),
            b"old"
        );
    }

    #[test]
    #[ignore = "Explicit network acceptance test: downloads the publisher ZIP into an isolated temporary directory"]
    fn live_official_update_in_isolated_directory() {
        if !supported() {
            return;
        }
        let root = TempRoot::new();
        let release = latest_release().unwrap();
        println!(
            "Official release: {} / SHA256 {} / isolated root {}",
            release.version,
            release.sha256,
            root.0.display()
        );
        let directory = if let Some(archive) = std::env::var_os("FRAMEFLOW_UPDATE_QA_ARCHIVE") {
            // Test-only replay; the archive must still match the current publisher SHA.
            println!(
                "Replaying retained publisher ZIP: {}",
                Path::new(&archive).display()
            );
            install_from_reader(
                &release,
                &root.0,
                File::open(archive).unwrap(),
                &|message| println!("{message}"),
                validate_engine,
            )
            .unwrap()
        } else {
            install_at(&release, &root.0, &|message| println!("{message}")).unwrap()
        };
        assert_eq!(managed_engine_at(&root.0), Some(directory.clone()));
        validate_engine(&directory, &release.version).unwrap();
        println!("Validated and selected: {}", directory.display());
        let before = fs::read(root.0.join("active.json")).unwrap();
        let bad = Release {
            version: "99.0.0".into(),
            archive_url: String::new(),
            sha256: "0".repeat(64),
        };
        assert!(
            install_from_reader(
                &bad,
                &root.0,
                Cursor::new(b"corrupt fixture"),
                &|_| {},
                validate_engine
            )
            .is_err()
        );
        assert_eq!(fs::read(root.0.join("active.json")).unwrap(), before);
        validate_engine(&directory, &release.version).unwrap();
        println!("Failed update preserved the active pointer and both working tools.");
    }
}

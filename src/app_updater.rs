//! Public GitHub release checks and verified, user-initiated Windows setup downloads.
//! Downloading never launches a program; the UI explicitly asks to install and exit.

use reqwest::{Url, blocking::Client};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const REPOSITORY_URL: &str = "https://github.com/FueTsui/Frameflow";
pub const RELEASES_URL: &str = "https://github.com/FueTsui/Frameflow/releases";
const LATEST_URL: &str = "https://api.github.com/repos/FueTsui/Frameflow/releases/latest";
const MAX_METADATA: u64 = 256 * 1024;
const MAX_CHECKSUM: u64 = 4096;
const MAX_INSTALLER: u64 = 512 * 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    setup_url: String,
    checksum_url: String,
    size: u64,
}

#[derive(Clone, Debug)]
pub struct DownloadedInstaller {
    pub path: PathBuf,
    version: String,
    sha256: String,
    size: u64,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    state: String,
}

pub fn supported() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

fn require_supported() -> Result<(), String> {
    supported()
        .then_some(())
        .ok_or_else(|| "软件自动更新目前支持 Windows x64。".into())
}

fn stable_version(value: &str) -> Option<[u64; 3]> {
    let value = value.strip_prefix('v').unwrap_or(value);
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    let mut parts = value.split('.');
    let mut result = [0; 3];
    for part in &mut result {
        let token = parts.next()?;
        if token.is_empty()
            || !token.bytes().all(|c| c.is_ascii_digit())
            || (token.len() > 1 && token.starts_with('0'))
        {
            return None;
        }
        *part = token.parse().ok()?;
    }
    parts.next().is_none().then_some(result)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (stable_version(candidate), stable_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn filename(version: &str) -> String {
    format!("Frameflow-{version}-windows-x64-setup.exe")
}

fn secure_origin(url: &Url, host: &str) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn repository_asset_url(url: &Url) -> bool {
    if !secure_origin(url, "github.com") || url.query().is_some() {
        return false;
    }
    let Some(rest) = url
        .path()
        .strip_prefix("/FueTsui/Frameflow/releases/download/")
    else {
        return false;
    };
    let Some((tag, asset)) = rest.split_once('/') else {
        return false;
    };
    if stable_version(tag).is_none() {
        return false;
    }
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let setup = filename(version);
    asset == setup || asset == format!("{setup}.sha256")
}

fn trusted_redirect(url: &Url, initial: &Url) -> bool {
    if initial.as_str() == LATEST_URL {
        return url.as_str() == LATEST_URL;
    }
    // A repository asset may only stay at that exact URL or use GitHub's asset CDN.
    url == initial
        || (repository_asset_url(initial)
            && secure_origin(url, "release-assets.githubusercontent.com")
            && url.path().starts_with("/github-production-release-asset/")
            && url.path().len() > "/github-production-release-asset/".len())
}

fn client() -> Result<Client, String> {
    Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(15 * 60))
        .user_agent(concat!("Frameflow/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5
                || !attempt
                    .previous()
                    .first()
                    .is_some_and(|initial| trusted_redirect(attempt.url(), initial))
            {
                attempt.error("untrusted release redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "无法创建软件更新连接。".into())
}

fn bounded_read(mut input: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取更新信息失败，请重试。".to_string())?;
    if bytes.len() as u64 > limit {
        return Err("更新信息超出允许大小。".into());
    }
    Ok(bytes)
}

fn metadata(client: &Client, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let parsed = Url::parse(url).map_err(|_| "更新地址无效。".to_string())?;
    if parsed.as_str() != LATEST_URL && !repository_asset_url(&parsed) {
        return Err("更新地址不属于官方发布仓库。".into());
    }
    let response = client
        .get(parsed)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|_| "无法连接软件更新服务，请检查网络后重试。".to_string())?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            404 => "尚未找到公开的稳定版本。",
            403 | 429 => "更新检查暂时受限，请稍后重试。",
            _ => "无法获取软件更新信息，请稍后重试。",
        }
        .into());
    }
    if response.content_length().is_some_and(|size| size > limit) {
        return Err("更新信息超出允许大小。".into());
    }
    bounded_read(response, limit)
}

fn parse_release(bytes: &[u8]) -> Result<Release, String> {
    if bytes.len() as u64 > MAX_METADATA {
        return Err("更新信息超出允许大小。".into());
    }
    let value: GithubRelease =
        serde_json::from_slice(bytes).map_err(|_| "发布信息格式无效。".to_string())?;
    if value.draft || value.prerelease || stable_version(&value.tag_name).is_none() {
        return Err("发布信息不是有效的稳定版本。".into());
    }
    let version = value
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&value.tag_name)
        .to_owned();
    if value.html_url != format!("{RELEASES_URL}/tag/{}", value.tag_name) {
        return Err("版本链接不属于官方发布仓库。".into());
    }
    if value.assets.len() > 128 {
        return Err("发布文件数量异常。".into());
    }
    let setup_name = filename(&version);
    let checksum_name = format!("{setup_name}.sha256");
    let select = |name: &str, min: u64, max: u64| -> Result<&GithubAsset, String> {
        let mut matching = value.assets.iter().filter(|asset| asset.name == name);
        let asset = matching.next().ok_or("发布版本缺少安装包或校验文件。")?;
        if matching.next().is_some()
            || asset.state != "uploaded"
            || !(min..=max).contains(&asset.size)
        {
            return Err("发布文件状态或大小无效。".into());
        }
        let expected = format!("{RELEASES_URL}/download/{}/{name}", value.tag_name);
        if asset.browser_download_url != expected
            || !Url::parse(&asset.browser_download_url).is_ok_and(|url| repository_asset_url(&url))
        {
            return Err("发布文件地址与仓库或版本不符。".into());
        }
        Ok(asset)
    };
    let setup = select(&setup_name, 512, MAX_INSTALLER)?;
    let checksum = select(&checksum_name, 64, MAX_CHECKSUM)?;
    Ok(Release {
        version,
        setup_url: setup.browser_download_url.clone(),
        checksum_url: checksum.browser_download_url.clone(),
        size: setup.size,
    })
}

pub fn latest_release() -> Result<Release, String> {
    require_supported()?;
    parse_release(&metadata(&client()?, LATEST_URL, MAX_METADATA)?)
}

#[cfg(test)]
pub(crate) fn test_release(version: &str) -> Release {
    let setup = format!("{RELEASES_URL}/download/v{version}/{}", filename(version));
    Release {
        version: version.into(),
        checksum_url: format!("{setup}.sha256"),
        setup_url: setup,
        size: 512,
    }
}

#[cfg(test)]
pub(crate) fn test_future_version() -> String {
    let [major, minor, patch] =
        stable_version(env!("CARGO_PKG_VERSION")).expect("package uses a stable semantic version");
    let patch = patch
        .checked_add(1)
        .expect("package patch version can advance");
    format!("{major}.{minor}.{patch}")
}

#[cfg(all(test, target_os = "windows", target_arch = "x86_64"))]
pub(crate) fn test_downloaded_installer(path: PathBuf) -> DownloadedInstaller {
    DownloadedInstaller {
        path,
        version: test_future_version(),
        sha256: "0".repeat(64),
        size: 512,
    }
}

fn parse_checksum(bytes: &[u8], expected_filename: &str) -> Result<String, String> {
    if bytes.len() as u64 > MAX_CHECKSUM {
        return Err("校验文件超出允许大小。".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "校验文件格式无效。".to_string())?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let line = lines.next().ok_or("校验文件为空。")?.trim();
    if lines.next().is_some() || line.len() < 64 {
        return Err("校验文件内容无效。".into());
    }
    let digest = line.get(..64).ok_or("校验值无效。")?;
    if !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("SHA-256 校验值无效。".into());
    }
    let remainder = &line[64..];
    let name = remainder.trim_start();
    if !remainder.is_empty()
        && (!remainder.starts_with(char::is_whitespace)
            || name.strip_prefix('*').unwrap_or(name) != expected_filename)
    {
        return Err("校验文件的安装包名称不符。".into());
    }
    Ok(digest.to_ascii_lowercase())
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

fn checked_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "无法访问更新目录。".to_string())?;
    if !metadata.is_dir() || has_link(&metadata) {
        return Err("更新目录不能是链接或普通文件。".into());
    }
    Ok(())
}

fn updates_root() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("无法确定用户更新目录。")?;
    let mut path = PathBuf::from(local);
    if !path.is_absolute() {
        return Err("用户更新目录必须为绝对路径。".into());
    }
    checked_directory(&path)?;
    for component in ["Frameflow", "updates"] {
        path.push(component);
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("无法创建用户更新目录。".into()),
        }
        checked_directory(&path)?;
    }
    fs::canonicalize(path).map_err(|_| "无法确定更新目录路径。".into())
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

struct PendingDownload {
    directory: PathBuf,
    file: PathBuf,
    owns_file: bool,
    retain: bool,
}

impl PendingDownload {
    fn create(root: &Path, version: &str) -> Result<(Self, File), String> {
        let directory = root.join(format!("{version}-{}", unique_suffix()));
        fs::create_dir(&directory).map_err(|_| "无法创建本次更新目录。".to_string())?;
        let mut pending = Self {
            file: directory.join(filename(version)),
            directory,
            owns_file: false,
            retain: false,
        };
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let file = options
            .open(&pending.file)
            .map_err(|_| "无法创建安装包文件。".to_string())?;
        pending.owns_file = true;
        Ok((pending, file))
    }
}

impl Drop for PendingDownload {
    fn drop(&mut self) {
        if !self.retain {
            // Remove only this attempt's owned file and empty directory, never recursively.
            if self.owns_file {
                let _ = fs::remove_file(&self.file);
            }
            let _ = fs::remove_dir(&self.directory);
        }
    }
}

fn check_pe(file: &mut File, size: u64) -> Result<(), String> {
    let invalid = || "下载文件不是支持的 Windows 安装程序。".to_string();
    if !(512..=MAX_INSTALLER).contains(&size) {
        return Err(invalid());
    }
    let mut dos = [0_u8; 64];
    file.seek(SeekFrom::Start(0)).map_err(|_| invalid())?;
    file.read_exact(&mut dos).map_err(|_| invalid())?;
    if &dos[..2] != b"MZ" {
        return Err(invalid());
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().map_err(|_| invalid())?) as u64;
    if !(64..=1024 * 1024).contains(&offset) || offset + 24 + 70 > size {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(offset)).map_err(|_| invalid())?;
    let mut header = [0_u8; 94];
    file.read_exact(&mut header).map_err(|_| invalid())?;
    let u16_at = |index| u16::from_le_bytes([header[index], header[index + 1]]);
    let machine = u16_at(4);
    let sections = u16_at(6);
    let optional_size = u16_at(20);
    let flags = u16_at(22);
    let magic = u16_at(24);
    let subsystem = u16_at(24 + 68);
    // Inno Setup uses an x86 bootstrap even when every installed application is x64.
    if &header[..4] != b"PE\0\0"
        || !matches!((machine, magic), (0x014c, 0x010b) | (0x8664, 0x020b))
        || !(1..=96).contains(&sections)
        || optional_size < 70
        || offset + 24 + u64::from(optional_size) > size
        || flags & 0x0002 == 0
        || flags & 0x2000 != 0
        || subsystem != 2
    {
        return Err(invalid());
    }
    Ok(())
}

fn save_download(
    mut input: impl Read,
    file: &mut File,
    expected_size: u64,
    expected_hash: &str,
    progress: &mut impl FnMut(String),
) -> Result<(), String> {
    if !(512..=MAX_INSTALLER).contains(&expected_size) {
        return Err("安装包大小超出允许范围。".into());
    }
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    let mut last_report = 0;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|_| "安装包下载中断，请重试。")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > expected_size {
            return Err("安装包大小与发布信息不符。".into());
        }
        file.write_all(&buffer[..count])
            .map_err(|_| "无法保存安装包。")?;
        hash.update(&buffer[..count]);
        let megabytes = total / (1024 * 1024);
        if megabytes >= last_report + 2 {
            last_report = megabytes;
            progress(format!(
                "已下载 {megabytes} / {} MB",
                expected_size.div_ceil(1024 * 1024)
            ));
        }
    }
    if total != expected_size {
        return Err("安装包下载不完整，请重试。".into());
    }
    progress("正在校验安装包…".into());
    if format!("{:x}", hash.finalize()) != expected_hash {
        return Err("安装包 SHA-256 校验失败，已取消本次下载。".into());
    }
    file.sync_all().map_err(|_| "无法完成安装包保存。")?;
    check_pe(file, total)
}

pub fn download_release(
    release: &Release,
    mut progress: impl FnMut(String),
) -> Result<DownloadedInstaller, String> {
    require_supported()?;
    if !is_newer(&release.version, env!("CARGO_PKG_VERSION")) {
        return Err("该版本不高于当前软件版本。".into());
    }
    let url = Url::parse(&release.setup_url).map_err(|_| "安装包地址无效。")?;
    if !repository_asset_url(&url)
        || !url
            .path()
            .ends_with(&format!("/{}", filename(&release.version)))
        || release.checksum_url != format!("{}.sha256", release.setup_url)
        || !(512..=MAX_INSTALLER).contains(&release.size)
    {
        return Err("安装包地址或大小与版本不符。".into());
    }
    let client = client()?;
    progress("正在获取安装包校验信息…".into());
    let sha256 = parse_checksum(
        &metadata(&client, &release.checksum_url, MAX_CHECKSUM)?,
        &filename(&release.version),
    )?;
    progress(format!("正在下载 Frameflow {}…", release.version));
    let response = client
        .get(url)
        .send()
        .map_err(|_| "无法下载安装包，请检查网络后重试。".to_string())?;
    if !response.status().is_success() {
        return Err("安装包暂时无法下载，请稍后重试。".into());
    }
    if response
        .content_length()
        .is_some_and(|size| size != release.size)
    {
        return Err("安装包大小与发布信息不符。".into());
    }
    let (mut pending, mut file) = PendingDownload::create(&updates_root()?, &release.version)?;
    let saved = save_download(response, &mut file, release.size, &sha256, &mut progress);
    drop(file);
    saved?;
    let installer = DownloadedInstaller {
        path: pending.file.clone(),
        version: release.version.clone(),
        sha256,
        size: release.size,
    };
    pending.retain = true;
    progress("安装包已验证，点击“安装并退出”继续。".into());
    Ok(installer)
}

fn verified_installer(installer: &DownloadedInstaller) -> Result<File, String> {
    if !is_newer(&installer.version, env!("CARGO_PKG_VERSION")) {
        return Err("安装包版本不高于当前版本，请重新检查更新。".into());
    }
    let root = updates_root()?;
    let parent = installer.path.parent().ok_or("安装包路径无效。")?;
    checked_directory(parent)?;
    let canonical_parent = fs::canonicalize(parent).map_err(|_| "安装包目录已失效。")?;
    if canonical_parent.parent() != Some(root.as_path())
        || installer.path.file_name() != Some(std::ffi::OsStr::new(&filename(&installer.version)))
    {
        return Err("安装包不在本应用的更新目录中。".into());
    }
    verified_file_at(installer)
}

fn verified_file_at(installer: &DownloadedInstaller) -> Result<File, String> {
    let metadata =
        fs::symlink_metadata(&installer.path).map_err(|_| "安装包已丢失，请重新下载。")?;
    if !metadata.is_file() || has_link(&metadata) || metadata.len() != installer.size {
        return Err("安装包已变化，请重新下载。".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Keep writers and deletion blocked until the verified file has been launched.
        options.share_mode(1);
    }
    let mut file = options
        .open(&installer.path)
        .map_err(|_| "无法读取或锁定安装包。")?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0;
    loop {
        let count = file.read(&mut buffer).map_err(|_| "无法重新校验安装包。")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > installer.size {
            return Err("安装包已变化，请重新下载。".into());
        }
        hash.update(&buffer[..count]);
    }
    if total != installer.size || format!("{:x}", hash.finalize()) != installer.sha256 {
        return Err("安装包已变化，请重新下载。".into());
    }
    check_pe(&mut file, total)?;
    Ok(file)
}

fn install_directory(install_dir: &Path, current_exe: &Path) -> Result<PathBuf, String> {
    if !install_dir.is_absolute() {
        return Err("安装目录必须为当前程序的绝对路径。".into());
    }
    let current = current_exe.parent().ok_or("无法确定当前程序目录。")?;
    let expected = fs::canonicalize(current).map_err(|_| "当前程序目录已失效。")?;
    let actual = fs::canonicalize(install_dir).map_err(|_| "安装目录已失效。")?;
    if actual != expected {
        return Err("更新必须安装到当前程序目录。".into());
    }
    // Keep the caller's ordinary absolute path; Inno Setup need not handle verbatim prefixes.
    Ok(install_dir.to_owned())
}

pub fn launch_installer(installer: &DownloadedInstaller, install_dir: &Path) -> Result<(), String> {
    require_supported()?;
    let current_exe = std::env::current_exe().map_err(|_| "无法确定当前程序位置。")?;
    let directory = install_directory(install_dir, &current_exe)?;
    let verified_file = verified_installer(installer)?;
    let mut directory_arg = std::ffi::OsString::from("/DIR=");
    directory_arg.push(directory);
    let mut command = Command::new(&installer.path);
    command
        .arg("/SP-")
        .arg(directory_arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let result = command
        .spawn()
        .map_err(|_| "无法启动安装向导，请重试。".to_string());
    drop(verified_file);
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture(version: &str) -> serde_json::Value {
        let setup = filename(version);
        serde_json::json!({
            "tag_name": format!("v{version}"),
            "html_url": format!("{RELEASES_URL}/tag/v{version}"),
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": setup, "browser_download_url": format!("{RELEASES_URL}/download/v{version}/{setup}"), "size": 512, "state": "uploaded"},
                {"name": format!("{setup}.sha256"), "browser_download_url": format!("{RELEASES_URL}/download/v{version}/{setup}.sha256"), "size": 128, "state": "uploaded"}
            ]
        })
    }

    fn parse(value: &serde_json::Value) -> Result<Release, String> {
        parse_release(&serde_json::to_vec(value).unwrap())
    }

    fn pe_fixture(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0; 512];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64_u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&machine.to_le_bytes());
        bytes[70..72].copy_from_slice(&1_u16.to_le_bytes());
        bytes[84..86].copy_from_slice(&224_u16.to_le_bytes());
        bytes[86..88].copy_from_slice(&0x0002_u16.to_le_bytes());
        bytes[88..90].copy_from_slice(
            &(if machine == 0x014c {
                0x010b_u16
            } else {
                0x020b
            })
            .to_le_bytes(),
        );
        bytes[156..158].copy_from_slice(&2_u16.to_le_bytes());
        bytes
    }

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("frameflow-app-update-test-{}", unique_suffix()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn stable_versions_reject_downgrades_prereleases_and_malformed_values() {
        assert!(is_newer("0.2.2", "0.2.1"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.1", "0.2.1"));
        assert!(!is_newer("0.2.0", "0.2.1"));
        for value in [
            "0.2",
            "0.2.3.4",
            "01.2.3",
            "0.2.3-beta",
            "0.2.3+build",
            " 0.2.3",
            "0.2.3/other",
            "18446744073709551616.0.0",
        ] {
            assert!(!is_newer(value, "0.2.1"), "{value}");
        }
    }

    #[test]
    fn compiled_version_guards_reject_same_version_before_network_or_filesystem_access() {
        let current = env!("CARGO_PKG_VERSION");
        let future = test_future_version();
        assert!(is_newer(&future, current));
        assert!(!is_newer(current, current));
        if supported() {
            let result = download_release(&test_release(current), |_| {
                panic!("same-version downloads must stop before network progress")
            });
            assert!(result.unwrap_err().contains("不高于当前软件版本"));
        }
        let installer = DownloadedInstaller {
            path: PathBuf::from("fixture-does-not-exist.exe"),
            version: current.into(),
            sha256: "0".repeat(64),
            size: 512,
        };
        assert!(
            verified_installer(&installer)
                .unwrap_err()
                .contains("不高于当前版本")
        );
    }

    #[test]
    fn release_assets_are_exact_published_repository_files() {
        let valid = fixture("0.2.2");
        let release = parse(&valid).unwrap();
        assert_eq!(release.version, "0.2.2");
        for (field, value) in [
            ("draft", serde_json::json!(true)),
            ("prerelease", serde_json::json!(true)),
            ("tag_name", serde_json::json!("v0.2.2-rc1")),
            (
                "html_url",
                serde_json::json!("https://github.com/other/Frameflow/releases/tag/v0.2.2"),
            ),
        ] {
            let mut changed = valid.clone();
            changed[field] = value;
            assert!(parse(&changed).is_err());
        }
        for (field, value) in [
            (
                "name",
                serde_json::json!("Frameflow-0.2.2-windows-arm64-setup.exe"),
            ),
            ("state", serde_json::json!("new")),
            ("size", serde_json::json!(MAX_INSTALLER + 1)),
            (
                "browser_download_url",
                serde_json::json!(
                    "https://github.com/other/Frameflow/releases/download/v0.2.2/Frameflow-0.2.2-windows-x64-setup.exe"
                ),
            ),
            (
                "browser_download_url",
                serde_json::json!(
                    "https://github.com/FueTsui/Frameflow/releases/download/v0.2.1/Frameflow-0.2.2-windows-x64-setup.exe"
                ),
            ),
        ] {
            let mut changed = valid.clone();
            changed["assets"][0][field] = value;
            assert!(parse(&changed).is_err());
        }
        let mut duplicate = valid;
        let extra = duplicate["assets"][0].clone();
        duplicate["assets"].as_array_mut().unwrap().push(extra);
        assert!(parse(&duplicate).is_err());
    }

    #[test]
    fn redirects_reject_other_repositories_credentials_and_unsafe_origins() {
        let release = test_release("0.2.2");
        let initial = Url::parse(&release.setup_url).unwrap();
        assert!(repository_asset_url(&initial));
        assert!(trusted_redirect(&Url::parse("https://release-assets.githubusercontent.com/github-production-release-asset/123/asset?token=signed").unwrap(), &initial));
        for url in [
            "http://release-assets.githubusercontent.com/github-production-release-asset/123/asset",
            "https://release-assets.githubusercontent.com.evil.example/github-production-release-asset/123/asset",
            "https://user:pass@release-assets.githubusercontent.com/github-production-release-asset/123/asset",
            "https://release-assets.githubusercontent.com:444/github-production-release-asset/123/asset",
            "https://release-assets.githubusercontent.com/unrelated/asset",
            "https://github.com/other/Frameflow/releases/download/v0.2.2/setup.exe",
            "https://127.0.0.1/setup.exe",
            "file:///C:/Windows/System32/calc.exe",
        ] {
            let url = Url::parse(url).unwrap();
            assert!(!trusted_redirect(&url, &initial), "{url}");
            assert!(!repository_asset_url(&url), "{url}");
        }
        assert!(!trusted_redirect(
            &initial,
            &Url::parse(LATEST_URL).unwrap()
        ));
    }

    #[test]
    fn checksum_requires_one_matching_file_and_bounded_metadata() {
        let hash = "a".repeat(64);
        let setup = filename("0.2.2");
        for text in [
            hash.clone(),
            format!("{hash}  {setup}\n"),
            format!("{hash} *{setup}\r\n"),
        ] {
            assert_eq!(parse_checksum(text.as_bytes(), &setup).unwrap(), hash);
        }
        for text in [
            format!("{hash}  other.exe"),
            format!("{hash}\n{hash}"),
            "z".repeat(64),
            format!("{hash}{setup}"),
        ] {
            assert!(parse_checksum(text.as_bytes(), &setup).is_err());
        }
        assert!(bounded_read(Cursor::new(vec![0; 17]), 16).is_err());
        assert!(parse_release(&vec![0; MAX_METADATA as usize + 1]).is_err());
        assert!(parse_checksum(&vec![0; MAX_CHECKSUM as usize + 1], &setup).is_err());
    }

    #[test]
    fn downloads_verify_hash_size_and_x86_or_x64_setup_bootstraps() {
        let directory = Directory::new();
        for machine in [0x014c, 0x8664, 0xaa64] {
            let bytes = pe_fixture(machine);
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let (pending, mut file) = PendingDownload::create(&directory.0, "0.2.2").unwrap();
            let path = pending.file.clone();
            let result = save_download(Cursor::new(bytes), &mut file, 512, &hash, &mut |_| {});
            assert_eq!(result.is_ok(), machine != 0xaa64);
            drop(file);
            drop(pending);
            assert!(!path.exists());
        }
        for (bytes, expected, hash) in [
            (pe_fixture(0x014c), 512, "0".repeat(64)),
            (vec![0; 513], 512, "0".repeat(64)),
            (vec![0; 511], 512, "0".repeat(64)),
            (vec![0; 512], MAX_INSTALLER + 1, "0".repeat(64)),
        ] {
            let (pending, mut file) = PendingDownload::create(&directory.0, "0.2.2").unwrap();
            let path = pending.file.clone();
            assert!(
                save_download(Cursor::new(bytes), &mut file, expected, &hash, &mut |_| {}).is_err()
            );
            drop(file);
            drop(pending);
            assert!(!path.exists());
        }
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn download_attempts_never_replace_existing_files_and_partial_reads_clean_up() {
        struct Interrupted;
        impl Read for Interrupted {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "test",
                ))
            }
        }
        let directory = Directory::new();
        let (pending, mut file) = PendingDownload::create(&directory.0, "0.2.2").unwrap();
        assert!(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&pending.file)
                .is_err()
        );
        let path = pending.file.clone();
        assert!(save_download(Interrupted, &mut file, 512, &"0".repeat(64), &mut |_| {}).is_err());
        drop(file);
        drop(pending);
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn installer_target_is_exact_current_program_directory() {
        let directory = Directory::new();
        let other = Directory::new();
        let executable = directory.0.join("frameflow.exe");
        assert!(install_directory(&directory.0, &executable).is_ok());
        assert!(install_directory(&other.0, &executable).is_err());
        assert!(install_directory(Path::new("relative"), &executable).is_err());
    }

    #[test]
    fn prelaunch_verification_detects_tampering_and_locks_verified_bytes() {
        let directory = Directory::new();
        let bytes = pe_fixture(0x014c);
        let version = test_future_version();
        let (pending, mut file) = PendingDownload::create(&directory.0, &version).unwrap();
        file.write_all(&bytes).unwrap();
        drop(file);
        let installer = DownloadedInstaller {
            path: pending.file.clone(),
            version,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            size: bytes.len() as u64,
        };
        let verified = verified_file_at(&installer).unwrap();
        #[cfg(windows)]
        {
            assert!(
                OpenOptions::new()
                    .write(true)
                    .open(&installer.path)
                    .is_err()
            );
            assert!(fs::remove_file(&installer.path).is_err());
        }
        drop(verified);
        let mut tampered = bytes;
        tampered[400] = 1;
        fs::write(&installer.path, tampered).unwrap();
        assert!(verified_file_at(&installer).is_err());
        drop(pending);
    }

    fn live_release_and_checksum() -> (Client, Release, String) {
        let client = client().unwrap();
        let bytes = metadata(&client, LATEST_URL, MAX_METADATA).unwrap();
        let release = parse_release(&bytes).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(stable_version(&release.version).is_some());
        assert!(repository_asset_url(
            &Url::parse(&release.setup_url).unwrap()
        ));
        let assets = value["assets"].as_array().unwrap();
        let setup_name = filename(&release.version);
        let checksum_name = format!("{setup_name}.sha256");
        let setup = assets
            .iter()
            .find(|asset| asset["name"].as_str() == Some(setup_name.as_str()))
            .unwrap();
        let checksum_asset = assets
            .iter()
            .find(|asset| asset["name"].as_str() == Some(checksum_name.as_str()))
            .unwrap();
        let checksum = metadata(&client, &release.checksum_url, MAX_CHECKSUM).unwrap();
        assert_eq!(
            checksum.len() as u64,
            checksum_asset["size"].as_u64().unwrap()
        );
        assert_eq!(
            checksum_asset["digest"]
                .as_str()
                .expect("GitHub checksum asset digest"),
            format!("sha256:{:x}", Sha256::digest(&checksum)),
            "checksum asset contents differ from GitHub metadata"
        );
        let sha256 = parse_checksum(&checksum, &setup_name).unwrap();
        assert_eq!(
            setup["digest"]
                .as_str()
                .expect("GitHub installer asset digest"),
            format!("sha256:{sha256}"),
            "published SHA-256 differs from GitHub installer metadata"
        );
        assert_eq!(setup["size"].as_u64().unwrap(), release.size);
        (client, release, sha256)
    }

    #[test]
    #[ignore = "Reads public GitHub latest and checksum assets and compares asset digests; requires network."]
    fn live_public_latest_release_metadata() {
        let (_, release, _) = live_release_and_checksum();
        assert!(release.size <= MAX_INSTALLER);
    }

    #[test]
    #[ignore = "Downloads the latest public setup to a temporary directory, verifies hash and PE, never executes it."]
    fn live_public_latest_installer_integrity() {
        let (client, release, sha256) = live_release_and_checksum();
        let response = client
            .get(&release.setup_url)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap();
        if let Some(size) = response.content_length() {
            assert_eq!(size, release.size);
        }
        let directory = Directory::new();
        let (pending, mut file) = PendingDownload::create(&directory.0, &release.version).unwrap();
        // Exercise the same bounded writer, SHA-256 and PE checks without weakening
        // production's requirement that a downloaded version be strictly newer.
        let saved = save_download(response, &mut file, release.size, &sha256, &mut |message| {
            eprintln!("{message}");
        });
        drop(file);
        saved.unwrap();
        let installer = DownloadedInstaller {
            path: pending.file.clone(),
            version: release.version,
            sha256,
            size: release.size,
        };
        drop(verified_file_at(&installer).unwrap());
        let path = installer.path;
        drop(pending);
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }
}

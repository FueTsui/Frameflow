//! Per-user instance ownership. OS file locks disappear even after a crash; the
//! lock file itself must never be deleted (unlinking it would split ownership).
use eframe::egui::{self, ViewportCommand};
use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct Instance {
    lease: Arc<platform::Lease>,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
    root: PathBuf,
}

impl Instance {
    /// None means the existing process has been notified; do not open a new UI.
    pub fn acquire() -> io::Result<Option<Self>> {
        let cwd = env::current_dir()?;
        let paths: Vec<PathBuf> = env::args_os()
            .skip(1)
            .map(|arg| {
                let path = PathBuf::from(arg);
                if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                }
            })
            .collect();
        Self::with_paths(&runtime_directory()?, &paths)
    }

    #[cfg(test)]
    fn at(root: &Path) -> io::Result<Option<Self>> {
        Self::with_paths(root, &[])
    }

    fn with_paths(root: &Path, paths: &[PathBuf]) -> io::Result<Option<Self>> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(root)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(root)?;
        fs::create_dir_all(root.join("inbox"))?;
        Ok(platform::acquire(root, paths)?.map(|lease| Self {
            lease: Arc::new(lease),
            stop: Arc::new(AtomicBool::new(false)),
            listener: None,
            root: root.to_owned(),
        }))
    }

    pub fn attach(
        &mut self,
        ctx: &egui::Context,
        files: Box<dyn Fn(Vec<PathBuf>) + Send>,
    ) -> io::Result<()> {
        let lease = Arc::clone(&self.lease);
        let stop = Arc::clone(&self.stop);
        let ctx = ctx.clone();
        let root = self.root.clone();
        self.listener = Some(
            thread::Builder::new()
                .name("frameflow-activation".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let activated = match lease.wait() {
                            Ok(activated) => activated,
                            Err(_) => break,
                        };
                        let paths = take_requests(&root);
                        if !stop.load(Ordering::Relaxed) && (activated || !paths.is_empty()) {
                            if !paths.is_empty() {
                                files(paths);
                            }
                            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(ViewportCommand::Focus);
                            ctx.request_repaint();
                        }
                    }
                })?,
        );
        Ok(())
    }
}

const REQUEST_LIMIT: u64 = 1024 * 1024;

fn queue_request(root: &Path, paths: &[PathBuf]) -> io::Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(paths)?;
    if bytes.len() as u64 > REQUEST_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "一次打开的文件过多，请分批添加。",
        ));
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = root
        .join("inbox")
        .join(format!("request-{}-{nonce}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&path, path.with_extension("json"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result
}

fn take_requests(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("inbox")) else {
        return paths;
    };
    for entry in entries.flatten().take(64) {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|t| t.is_file())
            || !entry.file_name().to_string_lossy().starts_with("request-")
            || path.extension().is_none_or(|s| s != "json")
        {
            continue;
        }
        if let Ok(file) = fs::File::open(&path) {
            let mut bytes = Vec::new();
            if file.take(REQUEST_LIMIT + 1).read_to_end(&mut bytes).is_ok()
                && bytes.len() as u64 <= REQUEST_LIMIT
                && let Ok(request) = serde_json::from_slice::<Vec<PathBuf>>(&bytes)
            {
                paths.extend(request.into_iter().filter(|p| p.is_absolute()));
            }
        }
        let _ = fs::remove_file(path);
    }
    paths
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

fn absolute_env(name: &str) -> io::Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("缺少有效的用户目录：{name}"),
            )
        })
}

fn runtime_directory() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        // Each interactive session has its own window station and activation event.
        use windows_sys::Win32::System::{
            RemoteDesktop::ProcessIdToSessionId, Threading::GetCurrentProcessId,
        };
        let mut session = 0;
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(absolute_env("LOCALAPPDATA")?
            .join("Frameflow/runtime")
            .join(session.to_string()))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(absolute_env("HOME")?.join("Library/Application Support/Frameflow/runtime"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = absolute_env("XDG_DATA_HOME")
            .or_else(|_| absolute_env("HOME").map(|p| p.join(".local/share")))?;
        Ok(base.join("Frameflow/runtime"))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        fs::{File, OpenOptions},
        io::Write,
        os::windows::fs::OpenOptionsExt,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Storage::FileSystem::FILE_SHARE_READ,
        System::Threading::{CreateEventW, SetEvent, WaitForSingleObject},
        UI::WindowsAndMessaging::AllowSetForegroundWindow,
    };

    struct Event(HANDLE);
    // Event handles may be used from any thread; the Arc lease owns their lifetime.
    unsafe impl Send for Event {}
    unsafe impl Sync for Event {}
    impl Drop for Event {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    pub struct Lease {
        event: Event,
        _lock: File,
    }

    pub fn acquire(root: &Path, paths: &[PathBuf]) -> io::Result<Option<Lease>> {
        let identity = fs::canonicalize(root)?.to_string_lossy().to_lowercase();
        let digest = Sha256::digest(identity.as_bytes());
        let name: Vec<u16> = format!("Local\\Frameflow.Activate.{digest:x}\0")
            .encode_utf16()
            .collect();
        // Create before locking so an immediate second launch cannot lose its signal.
        let event = Event(unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) });
        if event.0.is_null() {
            return Err(io::Error::last_os_error());
        }
        let path = root.join("instance.lock");
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
        {
            Ok(mut lock) => {
                lock.set_len(0)?;
                write!(lock, "{}", std::process::id())?;
                lock.flush()?;
                Ok(Some(Lease { event, _lock: lock }))
            }
            Err(error) if matches!(error.raw_os_error(), Some(32 | 33)) => {
                queue_request(root, paths)?;
                // Let the original process take foreground rights from this launch.
                if let Ok(pid) = fs::read_to_string(&path).unwrap_or_default().parse::<u32>() {
                    unsafe {
                        AllowSetForegroundWindow(pid);
                    }
                }
                if unsafe { SetEvent(event.0) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    impl Lease {
        pub fn wait(&self) -> io::Result<bool> {
            match unsafe {
                WaitForSingleObject(self.event.0, Duration::from_millis(200).as_millis() as u32)
            } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        os::{
            fd::AsRawFd,
            unix::{fs::OpenOptionsExt, net::UnixDatagram},
        },
        time::Instant,
    };
    pub struct Lease {
        socket: UnixDatagram,
        socket_path: PathBuf,
        _lock: File,
    }

    pub fn acquire(root: &Path, paths: &[PathBuf]) -> io::Result<Option<Lease>> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join("instance.lock"))?;
        let path = root.join("activate.sock");
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
            queue_request(root, paths)?;
            let sender = UnixDatagram::unbound()?;
            sender.set_write_timeout(Some(Duration::from_millis(200)))?;
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match sender.send_to(b"activate", &path) {
                    Ok(_) => return Ok(None),
                    Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                    Err(error) => return Err(error),
                }
            }
        }
        // A crashed process can leave a socket name, but only the lock owner may remove it.
        match fs::remove_file(&path) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let socket = UnixDatagram::bind(&path)?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        Ok(Some(Lease {
            socket,
            socket_path: path,
            _lock: lock,
        }))
    }
    impl Lease {
        pub fn wait(&self) -> io::Result<bool> {
            let mut message = [0u8; 16];
            match self.socket.recv(&mut message) {
                Ok(length) => Ok(&message[..length] == b"activate"),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    Ok(false)
                }
                Err(error) => Err(error),
            }
        }
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn directory() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Darwin limits Unix socket paths to 104 bytes, including the terminator;
        // its per-user temporary directory can already consume over half of that.
        env::temp_dir().join(format!("ff-i-{:x}-{nonce:x}", std::process::id()))
    }
    #[test]
    fn duplicate_signals_primary_and_lock_is_reusable() {
        let root = directory();
        let first = Instance::at(&root).unwrap().unwrap();
        assert!(Instance::at(&root).unwrap().is_none());
        assert!(first.lease.wait().unwrap());
        assert!(!first.lease.wait().unwrap());
        drop(first);
        let replacement = Instance::at(&root).unwrap().unwrap();
        assert!(!replacement.lease.wait().unwrap());
        drop(replacement);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_runtime_directory_is_an_error_not_a_duplicate() {
        let root = directory();
        fs::write(&root, b"occupied").unwrap();
        assert!(Instance::at(&root).is_err());
        fs::remove_file(root).unwrap();
    }
    #[test]
    fn activation_restores_and_focuses_the_viewport() {
        let root = directory();
        let mut first = Instance::at(&root).unwrap().unwrap();
        let ctx = egui::Context::default();
        first.attach(&ctx, Box::new(|_| {})).unwrap();
        assert!(Instance::at(&root).unwrap().is_none());
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut restored = false;
        while std::time::Instant::now() < deadline {
            let output = ctx.run(egui::RawInput::default(), |_| {});
            let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
            if commands.iter().any(|c| matches!(c, ViewportCommand::Focus)) {
                assert!(
                    commands
                        .iter()
                        .any(|c| matches!(c, ViewportCommand::Minimized(false)))
                );
                restored = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(restored);
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn secondary_launch_forwards_unicode_paths_exactly_once() {
        let root = directory();
        let mut first = Instance::at(&root).unwrap().unwrap();
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        first
            .attach(
                &ctx,
                Box::new(move |paths| {
                    tx.send(paths).unwrap();
                }),
            )
            .unwrap();
        let paths = vec![root.join("中文 空格.mp4"), root.join("second.wav")];
        assert!(Instance::with_paths(&root, &paths).unwrap().is_none());
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), paths);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(fs::read_dir(root.join("inbox")).unwrap().count(), 0);
        drop(first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by crash_releases_instance_lock"]
    fn instance_process_fixture() {
        let root = PathBuf::from(env::var_os("FRAMEFLOW_TEST_INSTANCE_ROOT").unwrap());
        let _instance = Instance::at(&root).unwrap().unwrap();
        fs::write(root.join("ready"), b"ready").unwrap();
        thread::sleep(Duration::from_secs(30));
    }

    #[test]
    fn crash_releases_instance_lock() {
        struct Process(std::process::Child);
        impl Drop for Process {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = directory();
        let mut command = std::process::Command::new(env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "single_instance::tests::instance_process_fixture",
            ])
            .env("FRAMEFLOW_TEST_INSTANCE_ROOT", &root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let process = Process(command.spawn().unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !root.join("ready").exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(root.join("ready").exists());
        assert!(Instance::at(&root).unwrap().is_none());
        drop(process); // A hard kill must not leave an unusable instance lock.
        let restarted = Instance::at(&root).unwrap().unwrap();
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}

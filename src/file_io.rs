use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

fn not_regular_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "path must point to a regular file",
    )
}

pub(crate) fn open_regular_file(path: &Path) -> io::Result<File> {
    // Avoid opening known devices or other special files. Follow ordinary symlinks.
    if !fs::metadata(path)?.is_file() {
        return Err(not_regular_file());
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // A regular file can be replaced with a FIFO after the path check.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};

        // SAFETY: the owned file keeps the handle valid throughout this call.
        if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK {
            return Err(not_regular_file());
        }
    }
    // Inspect the opened handle, rather than trusting the earlier path check.
    if !file.metadata()?.is_file() {
        return Err(not_regular_file());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "cefdetector-file-io-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn opens_regular_files_and_rejects_directories_and_missing_paths() {
        let root = TempDirectory::new();
        let path = root.0.join("image.png");
        fs::write(&path, b"image bytes").unwrap();
        let mut bytes = Vec::new();
        open_regular_file(&path)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"image bytes");
        assert!(open_regular_file(&root.0).is_err());
        assert!(open_regular_file(&root.0.join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinks_to_regular_files() {
        use std::os::unix::fs::symlink;

        let root = TempDirectory::new();
        let path = root.0.join("image.png");
        let link = root.0.join("link.png");
        fs::write(&path, b"image bytes").unwrap();
        symlink(&path, &link).unwrap();
        assert!(
            open_regular_file(&link)
                .unwrap()
                .metadata()
                .unwrap()
                .is_file()
        );
        fs::remove_file(&path).unwrap();
        assert!(open_regular_file(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_special_files_without_waiting_for_a_writer() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;
        use std::process::Command;
        use std::time::{Duration, Instant};

        const CHILD_ROOT: &str = "CEFDETECTOR_TEST_SPECIAL_FILES";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = Path::new(&root);
            for name in ["fifo.png", "link.png", "socket.png"] {
                assert!(open_regular_file(&root.join(name)).is_err());
            }
            return;
        }

        let root = TempDirectory::new();
        let fifo = root.0.join("fifo.png");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: fifo_c is a valid, NUL-terminated path owned by this test.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        symlink(&fifo, root.0.join("link.png")).unwrap();
        let _listener = UnixListener::bind(root.0.join("socket.png")).unwrap();
        // Keep a regression from hanging the test runner itself.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "file_io::tests::rejects_special_files_without_waiting_for_a_writer",
            ])
            .env(CHILD_ROOT, &root.0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("opening a special file blocked");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(feature = "index")]
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
#[cfg(any(test, feature = "index"))]
use std::io;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
#[cfg(feature = "index")]
use std::process::Command;
#[cfg(any(test, feature = "index"))]
use std::process::Output;
#[cfg(feature = "index")]
use std::process::Stdio;

#[cfg(feature = "index")]
use super::backend::{ScanCandidate, classify_candidate_name};

#[cfg(feature = "index")]
const MDFIND: &str = "/usr/bin/mdfind";
#[cfg(any(test, feature = "index"))]
const APPLICATION_QUERY: &str = r#"kMDItemContentTypeTree == "com.apple.application-bundle""#;
#[cfg(feature = "index")]
const FILE_QUERY: &str = r#"kMDItemFSName == "Chromium Embedded Framework" || kMDItemFSName == "Electron Framework" || kMDItemFSName == "libcef*"c || kMDItemFSName == "libnode*"c || kMDItemFSName == "*_100_*.pak"c"#;
#[cfg(any(test, feature = "index"))]
const MAX_MDFIND_BYTES: usize = 64 * 1024 * 1024;
#[cfg(any(test, feature = "index"))]
const MAX_CANDIDATES: usize = 250_000;
#[cfg(feature = "index")]
const MDFIND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BundleInfo {
    pub(crate) root: PathBuf,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) icon_file: Option<String>,
}

fn is_app_component(component: &OsStr) -> bool {
    Path::new(component)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

pub(super) fn outermost_bundle(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start
        .ancestors()
        .filter(|ancestor| ancestor.file_name().is_some_and(is_app_component))
        .last()
        .map(Path::to_path_buf)
}

pub(crate) fn inspect_bundle(root: &Path) -> BundleInfo {
    let mut executable = None;
    let mut icon_file = None;
    if let Ok(value) = plist::Value::from_file(root.join("Contents/Info.plist"))
        && let Some(dictionary) = value.as_dictionary()
    {
        if let Some(name) = dictionary
            .get("CFBundleExecutable")
            .and_then(plist::Value::as_string)
        {
            let path = root.join("Contents/MacOS").join(name);
            if contained_file(root, &path) {
                executable = Some(path);
            }
        }
        icon_file = dictionary
            .get("CFBundleIconFile")
            .and_then(plist::Value::as_string)
            .or_else(|| {
                dictionary
                    .get("CFBundleIconName")
                    .and_then(plist::Value::as_string)
            })
            .map(str::to_owned);
    }
    BundleInfo {
        root: root.to_path_buf(),
        executable,
        icon_file,
    }
}

fn contained_file(root: &Path, path: &Path) -> bool {
    let Ok(root) = fs::canonicalize(root) else {
        return false;
    };
    fs::canonicalize(path).is_ok_and(|path| path.is_file() && path.starts_with(root))
}

#[cfg(any(test, feature = "gui"))]
pub(crate) fn bundle_icon_path(root: &Path) -> Option<PathBuf> {
    let icon = inspect_bundle(root).icon_file?;
    let file_name = Path::new(&icon).file_name()?;
    let mut path = root.join("Contents/Resources").join(file_name);
    if path.extension().is_none() {
        path.set_extension("icns");
    }
    contained_file(root, &path).then_some(path)
}

#[cfg(any(test, feature = "index"))]
fn parse_nul_paths(bytes: Vec<u8>) -> io::Result<Vec<PathBuf>> {
    if bytes.len() > MAX_MDFIND_BYTES {
        return Err(io::Error::other(
            "Spotlight output exceeded the 64 MiB safety limit",
        ));
    }
    let mut paths = Vec::new();
    for raw in bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        if paths.len() == MAX_CANDIDATES {
            return Err(io::Error::other(
                "Spotlight candidate count exceeded the safety limit",
            ));
        }
        paths.push(PathBuf::from(std::ffi::OsString::from_vec(raw.to_vec())));
    }
    Ok(paths)
}

#[cfg(any(test, feature = "index"))]
fn checked_output(output: Output) -> io::Result<Vec<PathBuf>> {
    if output.status.success() {
        return parse_nul_paths(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let summary = stderr.trim().chars().take(512).collect::<String>();
    Err(io::Error::other(if summary.is_empty() {
        "mdfind exited unsuccessfully".to_owned()
    } else {
        format!("mdfind failed: {summary}")
    }))
}

#[cfg(feature = "index")]
fn query(expression: &str) -> io::Result<Vec<PathBuf>> {
    use std::io::Read as _;

    let mut child = Command::new(MDFIND)
        .args(["-0", expression])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture mdfind stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("failed to capture mdfind stderr"))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_MDFIND_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(513).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = std::time::Instant::now() + MDFIND_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "mdfind did not finish within 30 seconds",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("mdfind stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("mdfind stderr reader panicked"))??;
    checked_output(Output {
        status,
        stdout,
        stderr,
    })
}

pub(super) fn is_platform_excluded(path: &Path) -> bool {
    const ROOTS: &[&str] = &[
        "/dev",
        "/System/Volumes",
        "/private/var/vm",
        "/private/var/folders",
        "/private/tmp",
        "/.vol",
        "/cores",
    ];
    ROOTS.iter().any(|root| path.starts_with(root))
        || path.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some(".Spotlight-V100" | ".fseventsd" | ".Trashes" | "Backups.backupdb")
            )
        })
}

#[cfg(feature = "index")]
fn scan_bundle(root: &Path, candidates: &mut BTreeSet<(PathBuf, super::backend::CandidateKind)>) {
    let mut pending = [
        "Contents/Frameworks",
        "Contents/Resources",
        "Contents/MacOS",
    ]
    .map(|relative| root.join(relative))
    .into_iter()
    .filter(|path| path.is_dir())
    .collect::<Vec<_>>();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file()
                && let Some(kind) = classify_candidate_name(&entry.file_name().to_string_lossy())
            {
                candidates.insert((path, kind));
            }
        }
    }
}

#[cfg(feature = "index")]
pub(super) fn spotlight_candidates() -> io::Result<Vec<ScanCandidate>> {
    let mut found = BTreeSet::new();
    let mut roots = BTreeSet::new();
    // Complete both index operations before walking application bundles. This
    // makes a disabled or unhealthy Spotlight service fail before any fallback-
    // eligible filesystem work is performed, without adding a probe query.
    let application_paths = query(APPLICATION_QUERY)?;
    let file_paths = query(FILE_QUERY)?;
    for path in application_paths {
        if path.is_dir() && !is_platform_excluded(&path) {
            roots.insert(outermost_bundle(&path).unwrap_or(path));
        }
    }
    for root in &roots {
        scan_bundle(root, &mut found);
    }
    for path in file_paths {
        if path.is_file()
            && !is_platform_excluded(&path)
            && let Some(name) = path.file_name()
            && let Some(kind) = classify_candidate_name(&name.to_string_lossy())
        {
            found.insert((path, kind));
        }
    }
    Ok(found
        .into_iter()
        .map(|(path, kind)| ScanCandidate {
            application_root_hint: outermost_bundle(&path),
            path,
            kind,
        })
        .collect())
}

pub(super) fn running_processes() -> HashSet<PathBuf> {
    use std::ffi::{c_int, c_void};

    unsafe extern "C" {
        fn proc_listallpids(buffer: *mut c_void, buffersize: c_int) -> c_int;
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    }

    const PATH_BUFFER_SIZE: usize = 4096;
    // SAFETY: A null buffer with size zero asks libproc for the current PID count.
    let estimated = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if estimated <= 0 {
        return HashSet::new();
    }
    let mut pids = vec![0_i32; estimated as usize + 64];
    // SAFETY: pids is writable for the supplied byte length.
    let count = unsafe {
        proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<i32>()) as c_int,
        )
    };
    if count <= 0 {
        return HashSet::new();
    }

    let mut processes = HashSet::new();
    for pid in pids.into_iter().take(count as usize).filter(|pid| *pid > 0) {
        let mut path = vec![0_u8; PATH_BUFFER_SIZE];
        // SAFETY: path is writable for PATH_BUFFER_SIZE bytes. PID races and
        // permission failures are represented by a non-positive return value.
        let length =
            unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), PATH_BUFFER_SIZE as u32) };
        if length > 0 {
            path.truncate(length as usize);
            processes.insert(PathBuf::from(std::ffi::OsString::from_vec(path)));
        }
    }
    processes
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::{ExitStatus, Output};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        APPLICATION_QUERY, bundle_icon_path, checked_output, inspect_bundle, outermost_bundle,
        parse_nul_paths,
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(std::path::PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "cefdetector-{name}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn plist_dictionary(executable: Option<&str>) -> plist::Dictionary {
        let mut dictionary = plist::Dictionary::new();
        if let Some(executable) = executable {
            dictionary.insert(
                "CFBundleExecutable".to_owned(),
                plist::Value::String(executable.to_owned()),
            );
        }
        dictionary.insert(
            "CFBundleIconFile".to_owned(),
            plist::Value::String("AppIcon".to_owned()),
        );
        dictionary
    }

    #[test]
    fn application_query_is_exact() {
        assert_eq!(
            APPLICATION_QUERY,
            r#"kMDItemContentTypeTree == "com.apple.application-bundle""#
        );
    }

    #[test]
    fn nul_paths_preserve_newlines_and_non_utf8() {
        let paths =
            parse_nul_paths(b"/Applications/Line\nBreak.app\0/tmp/\xff\0".to_vec()).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0],
            std::path::Path::new("/Applications/Line\nBreak.app")
        );
    }

    #[test]
    fn nested_helper_resolves_to_outermost_bundle() {
        let path = std::path::Path::new(
            "/Applications/Main.app/Contents/Frameworks/Main Helper.app/Contents/MacOS/Helper",
        );
        assert_eq!(
            outermost_bundle(path),
            Some(std::path::PathBuf::from("/Applications/Main.app"))
        );
    }

    #[test]
    fn xml_and_binary_plists_resolve_executable_and_icon() {
        for binary in [false, true] {
            let fixture = Fixture::new(if binary { "binary" } else { "xml" });
            fs::write(fixture.0.join("Contents/MacOS/Demo"), b"binary").unwrap();
            fs::create_dir_all(fixture.0.join("Contents/Resources")).unwrap();
            fs::write(fixture.0.join("Contents/Resources/AppIcon.icns"), b"icon").unwrap();
            let value = plist::Value::Dictionary(plist_dictionary(Some("Demo")));
            let info_path = fixture.0.join("Contents/Info.plist");
            if binary {
                value.to_file_binary(&info_path).unwrap();
            } else {
                value.to_file_xml(&info_path).unwrap();
            }
            let bundle = inspect_bundle(&fixture.0);
            assert_eq!(
                bundle.executable,
                Some(fixture.0.join("Contents/MacOS/Demo"))
            );
            assert_eq!(bundle.icon_file.as_deref(), Some("AppIcon"));
            assert_eq!(
                bundle_icon_path(&fixture.0),
                Some(fixture.0.join("Contents/Resources/AppIcon.icns"))
            );
        }
    }

    #[test]
    fn malformed_or_incomplete_bundle_still_returns_root() {
        let fixture = Fixture::new("incomplete");
        plist::Value::Dictionary(plist_dictionary(Some("Missing")))
            .to_file_xml(fixture.0.join("Contents/Info.plist"))
            .unwrap();
        let bundle = inspect_bundle(&fixture.0);
        assert_eq!(bundle.root, fixture.0);
        assert_eq!(bundle.executable, None);
    }

    #[test]
    fn executable_symlink_outside_bundle_is_rejected() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new("external-link");
        let outside = std::env::temp_dir().join(format!(
            "cefdetector-outside-{}",
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, fixture.0.join("Contents/MacOS/Demo")).unwrap();
        plist::Value::Dictionary(plist_dictionary(Some("Demo")))
            .to_file_xml(fixture.0.join("Contents/Info.plist"))
            .unwrap();
        assert_eq!(inspect_bundle(&fixture.0).executable, None);
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn current_process_is_visible_through_libproc() {
        let current = std::env::current_exe().unwrap();
        let processes = super::running_processes();
        assert!(
            processes.contains(&current)
                || std::fs::canonicalize(current).is_ok_and(|current| processes.contains(&current))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsuccessful_command_includes_stderr() {
        use std::os::unix::process::ExitStatusExt as _;
        let error = checked_output(Output {
            status: ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"metadata server unavailable\n".to_vec(),
        })
        .unwrap_err();
        assert!(error.to_string().contains("metadata server unavailable"));
    }
}

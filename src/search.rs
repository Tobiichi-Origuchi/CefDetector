use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use crate::models::AppInfo;

mod backend;

use backend::CandidateKind;

const SIGNATURE_CHUNK_SIZE: usize = 1024 * 1024;
const SIGNATURE_OVERLAP: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppKind {
    Electron,
    Nwjs,
    CefSharp,
    Edge,
    Chrome,
    Cef,
    MiniElectron,
    MiniBlink,
}

impl AppKind {
    fn label(self) -> &'static str {
        match self {
            Self::Electron => "Electron",
            Self::Nwjs => "NWJS",
            Self::CefSharp => "CefSharp",
            Self::Edge => "Edge",
            Self::Chrome => "Chrome",
            Self::Cef => "CEF",
            Self::MiniElectron => "Mini Electron",
            Self::MiniBlink => "Mini Blink",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Electron => 100,
            Self::Edge | Self::Chrome => 95,
            Self::Nwjs => 90,
            Self::CefSharp => 80,
            Self::MiniElectron => 75,
            Self::MiniBlink => 70,
            Self::Cef => 60,
        }
    }
}

#[derive(Clone, Copy)]
enum ScanFlavor {
    Standard,
    Mini,
}

#[derive(Clone)]
struct DirectoryInspection {
    app_kind: Option<AppKind>,
    executable: Option<PathBuf>,
}

#[derive(Default)]
struct CandidateFlags {
    pak: bool,
    cef: bool,
}

struct DetectedApp {
    file: PathBuf,
    app_type: &'static str,
    type_rank: u8,
    root: PathBuf,
    is_dir: bool,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
pub fn open_path(path: String, is_dir: bool) {
    if path.contains("://") || is_dir {
        let _ = Command::new("xdg-open").arg(path).spawn();
    } else if let Some(parent) = Path::new(&path).parent() {
        let _ = Command::new("xdg-open").arg(parent).spawn();
    }
}

#[cfg(target_os = "windows")]
fn explorer_select_argument(path: &std::ffi::OsStr) -> std::ffi::OsString {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    let mut argument: Vec<u16> = "/select,\"".encode_utf16().collect();
    argument.extend(path.encode_wide());
    argument.extend("\"".encode_utf16());
    std::ffi::OsString::from_wide(&argument)
}

#[cfg(target_os = "windows")]
pub fn open_path(path: String, is_dir: bool) {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation = ['o', 'p', 'e', 'n', '\0'].map(|character| character as u16);
    let (target, parameters) = if path.contains("://") || is_dir {
        let target: Vec<u16> = std::ffi::OsStr::new(&path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        (target, None)
    } else {
        let target = "explorer.exe\0".encode_utf16().collect();
        let parameters = explorer_select_argument(std::ffi::OsStr::new(&path))
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        (target, Some(parameters))
    };
    let parameters = parameters
        .as_ref()
        .map_or(std::ptr::null(), |parameters: &Vec<u16>| {
            parameters.as_ptr()
        });

    // SAFETY: operation, target, and optional parameters are null-terminated
    // and remain alive for the duration of ShellExecuteW.
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            parameters,
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0_u64;
    #[cfg(target_os = "linux")]
    let mut visited = HashSet::new();
    let mut pending = vec![dir.to_path_buf()];

    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };

            #[cfg(target_os = "linux")]
            if !visited.insert(FileIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            }) {
                continue;
            }

            total = total.saturating_add(metadata.len());
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                pending.push(path);
            }
        }
    }

    total
}

fn calculate_dir_sizes(apps: &[DetectedApp]) -> Vec<u64> {
    if apps.len() <= 1 {
        return apps.iter().map(|app| dir_size(&app.root)).collect();
    }

    let worker_count = std::thread::available_parallelism()
        .map_or(2, |count| count.get())
        .min(4)
        .min(apps.len());
    let next_index = AtomicUsize::new(0);
    let sizes = Mutex::new(vec![0_u64; apps.len()]);

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(app) = apps.get(index) else {
                        break;
                    };
                    let size = dir_size(&app.root);
                    sizes
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())[index] = size;
                }
            });
        }
    });

    sizes
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(target_os = "linux")]
fn get_running_processes() -> HashSet<PathBuf> {
    let mut processes = HashSet::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return processes;
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if file_name
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            && let Ok(executable) = fs::read_link(entry.path().join("exe"))
        {
            processes.insert(executable);
        }
    }
    processes
}

#[cfg(target_os = "windows")]
fn get_running_processes() -> HashSet<String> {
    use std::ffi::OsString;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStringExt as _;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    struct OwnedHandle(windows_sys::Win32::Foundation::HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: OwnedHandle is only constructed for a valid owned handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    // SAFETY: The call has no borrowed parameters and the returned handle is
    // closed by OwnedHandle.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return HashSet::new();
    }
    let snapshot = OwnedHandle(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = HashSet::new();

    // SAFETY: entry has the required size and remains valid during enumeration.
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
        return processes;
    }

    loop {
        // SAFETY: OpenProcess does not borrow the process ID. A non-null handle
        // is closed before the next iteration.
        let process =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID) };
        if !process.is_null() {
            let process = OwnedHandle(process);
            let mut path = vec![0_u16; 32_768];
            let mut length = path.len() as u32;
            // SAFETY: path is writable for length UTF-16 units and process is a
            // valid process handle. Failures (for protected processes) are ignored.
            if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) }
                != 0
            {
                let path = PathBuf::from(OsString::from_wide(&path[..length as usize]));
                processes.insert(normalize_windows_path(&path));
            }
        }

        // SAFETY: entry remains initialized with the required dwSize.
        if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
            break;
        }
    }

    processes
}

#[cfg(target_os = "windows")]
fn normalize_windows_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('/', "\\");
    path.strip_prefix(r"\\?\").unwrap_or(&path).to_lowercase()
}

#[cfg(target_os = "linux")]
fn process_is_running(processes: &HashSet<PathBuf>, path: &Path) -> bool {
    processes.contains(path)
        || fs::canonicalize(path).is_ok_and(|canonical| processes.contains(&canonical))
}

#[cfg(target_os = "windows")]
fn process_is_running(processes: &HashSet<String>, path: &Path) -> bool {
    processes.contains(&normalize_windows_path(path))
        || fs::canonicalize(path)
            .is_ok_and(|canonical| processes.contains(&normalize_windows_path(&canonical)))
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(haystack, needle).is_some()
}

fn strongest_signature(current: Option<AppKind>, candidate: Option<AppKind>) -> Option<AppKind> {
    match (current, candidate) {
        (Some(current), Some(candidate)) if candidate.rank() > current.rank() => Some(candidate),
        (None, candidate) => candidate,
        (current, _) => current,
    }
}

fn signature_in_chunk(bytes: &[u8], flavor: ScanFlavor) -> Option<AppKind> {
    match flavor {
        ScanFlavor::Standard => {
            if contains_bytes(bytes, b"third_party/electron_node")
                || contains_bytes(bytes, b"register_atom_browser_web_contents")
            {
                Some(AppKind::Electron)
            } else if contains_bytes(bytes, b"url-nwjs") {
                Some(AppKind::Nwjs)
            } else if contains_bytes(bytes, b"CefSharp.Internals") {
                Some(AppKind::CefSharp)
            } else if contains_bytes(bytes, b"cef_string_utf8_to_utf16") {
                Some(AppKind::Cef)
            } else {
                None
            }
        }
        ScanFlavor::Mini => {
            if contains_bytes(bytes, b"napi_create_buffer") {
                Some(AppKind::MiniElectron)
            } else if contains_bytes(bytes, b"miniblink") {
                Some(AppKind::MiniBlink)
            } else {
                None
            }
        }
    }
}

fn scan_signature_reader(
    reader: &mut impl Read,
    flavor: ScanFlavor,
) -> io::Result<Option<AppKind>> {
    let mut buffer = vec![0_u8; SIGNATURE_CHUNK_SIZE + SIGNATURE_OVERLAP];
    let mut retained = 0;
    let mut strongest = None;

    loop {
        let read = match reader.read(&mut buffer[retained..]) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let available = retained + read;
        strongest =
            strongest_signature(strongest, signature_in_chunk(&buffer[..available], flavor));
        if strongest == Some(AppKind::Electron) {
            break;
        }

        retained = available.min(SIGNATURE_OVERLAP);
        buffer.copy_within(available - retained..available, 0);
    }

    Ok(strongest)
}

fn scan_binary_file(path: &Path, flavor: ScanFlavor) -> io::Result<Option<AppKind>> {
    let mut file = fs::File::open(path)?;
    let mut magic = [0_u8; 4];
    let magic_len = file.read(&mut magic)?;
    let is_elf = magic_len >= 4 && magic == *b"\x7fELF";
    let is_pe = magic_len >= 2 && magic[..2] == *b"MZ";
    if !is_elf && !is_pe {
        return Ok(None);
    }

    file.rewind()?;
    scan_signature_reader(&mut file, flavor)
}

fn is_shared_library(file_name: &str) -> bool {
    file_name.ends_with(".dll")
        || file_name.ends_with(".dylib")
        || file_name.ends_with(".so")
        || file_name.contains(".so.")
}

fn is_relevant_shared_library(file_name: &str) -> bool {
    file_name.contains("cef") || file_name == "nw.dll" || file_name.starts_with("libnw.")
}

fn is_unwanted_executable(file_name: &str) -> bool {
    file_name.contains("unins")
        || file_name.contains("setup")
        || file_name.contains("report")
        || file_name == "disk-free"
        || file_name == "chrome-sandbox"
        || file_name.contains("crashpad_handler")
}

fn executable_score(path: &Path, directory: &Path) -> u8 {
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let directory_name = directory
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();

    let mut score: u8 = 10;
    if extension == "exe" || extension == "appimage" {
        score += 40;
    } else if extension.is_empty() {
        score += 30;
    }
    if path
        .file_stem()
        .is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(&directory_name))
    {
        score += 20;
    }
    if file_name.contains("web") || file_name.contains("browser") || file_name.contains("cef") {
        score += 30;
    }
    score
}

fn inspect_directory(dir: &Path, flavor: ScanFlavor) -> DirectoryInspection {
    let mut entries: Vec<_> = fs::read_dir(dir).into_iter().flatten().flatten().collect();
    entries.sort_by_key(|entry| entry.path());

    let mut best_kind: Option<AppKind> = None;
    let mut best_signature_path = None;
    let mut best_signature_is_executable = false;
    let mut best_fallback: Option<(u8, PathBuf)> = None;

    for entry in entries {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }

        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if matches!(flavor, ScanFlavor::Standard) {
            let special_kind = match file_name.as_str() {
                "msedge" | "msedge.exe" | "msedge_proxy.exe" => Some(AppKind::Edge),
                "chrome" | "chrome.exe" => Some(AppKind::Chrome),
                _ => None,
            };
            if let Some(app_kind) = special_kind {
                return DirectoryInspection {
                    app_kind: Some(app_kind),
                    executable: Some(path),
                };
            }
        }

        #[cfg(target_os = "linux")]
        let is_executable = metadata.permissions().mode() & 0o111 != 0;
        #[cfg(target_os = "windows")]
        let is_executable = false;
        let is_shared = is_shared_library(&file_name);
        let is_windows_executable = file_name.ends_with(".exe");
        if !is_executable && !is_shared && !is_windows_executable {
            continue;
        }
        if matches!(flavor, ScanFlavor::Mini) && is_shared {
            // The N-API marker exists in ordinary libnode builds. Only an
            // application executable containing it is a useful MiniElectron lead.
            continue;
        }
        if matches!(flavor, ScanFlavor::Standard)
            && is_shared
            && !is_relevant_shared_library(&file_name)
        {
            continue;
        }

        let is_launchable = !is_shared
            && !is_unwanted_executable(&file_name)
            && (is_executable || is_windows_executable);
        let Ok(signature) = scan_binary_file(&path, flavor) else {
            continue;
        };

        if is_launchable {
            let score = executable_score(&path, dir);
            if best_fallback
                .as_ref()
                .is_none_or(|(best_score, _)| score > *best_score)
            {
                best_fallback = Some((score, path.clone()));
            }
        }

        if let Some(signature) = signature
            && best_kind.is_none_or(|kind| signature.rank() > kind.rank())
        {
            best_kind = Some(signature);
            best_signature_path = Some(path);
            best_signature_is_executable = is_launchable;
        }
    }

    let executable = if best_signature_is_executable {
        best_signature_path
    } else {
        best_fallback.map(|(_, path)| path)
    };
    DirectoryInspection {
        app_kind: best_kind,
        executable,
    }
}

fn inspect_cached(
    dir: &Path,
    cache: &mut HashMap<PathBuf, DirectoryInspection>,
) -> DirectoryInspection {
    if let Some(inspection) = cache.get(dir) {
        return inspection.clone();
    }
    let inspection = inspect_directory(dir, ScanFlavor::Standard);
    cache.insert(dir.to_path_buf(), inspection.clone());
    inspection
}

fn detection_from_inspection(
    dir: &Path,
    inspection: DirectoryInspection,
    default_type: &'static str,
) -> Option<DetectedApp> {
    let app_type = inspection.app_kind.map_or(default_type, AppKind::label);
    let type_rank = inspection.app_kind.map_or(0, AppKind::rank);
    if let Some(executable) = inspection.executable {
        Some(DetectedApp {
            file: executable,
            app_type,
            type_rank,
            root: dir.to_path_buf(),
            is_dir: false,
        })
    } else if inspection.app_kind.is_some() {
        Some(DetectedApp {
            file: dir.to_path_buf(),
            app_type,
            type_rank,
            root: dir.to_path_buf(),
            is_dir: true,
        })
    } else {
        None
    }
}

fn resolve_standard_candidate(
    dir: &Path,
    default_type: &'static str,
    cache: &mut HashMap<PathBuf, DirectoryInspection>,
) -> DetectedApp {
    let inspection = inspect_cached(dir, cache);
    if inspection.app_kind.is_some() || inspection.executable.is_some() {
        return detection_from_inspection(dir, inspection, default_type).unwrap();
    }

    if let Some(parent) = dir.parent() {
        let parent_inspection = inspect_cached(parent, cache);
        if parent_inspection.app_kind.is_some() || parent_inspection.executable.is_some() {
            return detection_from_inspection(parent, parent_inspection, default_type).unwrap();
        }
    }

    DetectedApp {
        file: dir.to_path_buf(),
        app_type: default_type,
        type_rank: 0,
        root: dir.to_path_buf(),
        is_dir: true,
    }
}

fn insert_detection(apps: &mut BTreeMap<PathBuf, DetectedApp>, detected: DetectedApp) {
    match apps.get(&detected.root) {
        Some(existing)
            if existing.type_rank > detected.type_rank
                || (existing.type_rank == detected.type_rank
                    && !existing.is_dir
                    && detected.is_dir) => {}
        _ => {
            apps.insert(detected.root.clone(), detected);
        }
    }
}

fn is_ignored_candidate(path: &Path) -> bool {
    path.components().any(|component| {
        let component = component.as_os_str();
        component == ".Trash" || component == "Trash"
    })
}

pub fn core_search<F>(mut on_found: F) -> io::Result<()>
where
    F: FnMut(AppInfo),
{
    let running_processes = get_running_processes();
    let candidates = backend::find_candidates()?;
    let mut standard_dirs: BTreeMap<PathBuf, CandidateFlags> = BTreeMap::new();
    let mut node_dirs = HashSet::new();

    for candidate in candidates {
        if is_ignored_candidate(&candidate.path) {
            continue;
        }
        let Some(dir) = candidate.path.parent() else {
            continue;
        };
        match candidate.kind {
            CandidateKind::Pak => standard_dirs.entry(dir.to_path_buf()).or_default().pak = true,
            CandidateKind::Cef => standard_dirs.entry(dir.to_path_buf()).or_default().cef = true,
            CandidateKind::Node => {
                node_dirs.insert(dir.to_path_buf());
            }
        }
    }

    let mut inspections = HashMap::new();
    let mut detected_by_root = BTreeMap::new();
    for (dir, flags) in standard_dirs {
        let default_type = if flags.cef { "CEF" } else { "Unknown" };
        let detected = resolve_standard_candidate(&dir, default_type, &mut inspections);
        insert_detection(&mut detected_by_root, detected);
    }

    let mut node_dirs: Vec<_> = node_dirs.into_iter().collect();
    node_dirs.sort();
    for dir in node_dirs {
        let inspection = inspect_directory(&dir, ScanFlavor::Mini);
        if let Some(app_kind) = inspection.app_kind {
            let detected = detection_from_inspection(&dir, inspection, app_kind.label())
                .unwrap_or_else(|| DetectedApp {
                    file: dir.clone(),
                    app_type: app_kind.label(),
                    type_rank: app_kind.rank(),
                    root: dir.clone(),
                    is_dir: true,
                });
            insert_detection(&mut detected_by_root, detected);
        }
    }

    let roots: Vec<_> = detected_by_root
        .values()
        .map(|app| (app.root.clone(), app.app_type))
        .collect();
    detected_by_root.retain(|_, app| {
        !(app.is_dir
            && app.app_type == "Unknown"
            && roots.iter().any(|(other_root, other_type)| {
                other_root != &app.root
                    && *other_type != "Unknown"
                    && app.root.starts_with(other_root)
            }))
    });

    let detected: Vec<_> = detected_by_root.into_values().collect();
    let sizes = calculate_dir_sizes(&detected);
    for (app, size) in detected.into_iter().zip(sizes) {
        let is_running = if app.is_dir {
            false
        } else {
            process_is_running(&running_processes, &app.file)
        };

        on_found(AppInfo {
            file: app.file.to_string_lossy().into_owned(),
            app_type: app.app_type.to_owned(),
            size,
            is_running,
            is_dir: app.is_dir,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::backend::{CandidateKind, classify_candidate_name};
    #[cfg(target_os = "windows")]
    use super::explorer_select_argument;
    use super::{
        AppKind, SIGNATURE_CHUNK_SIZE, ScanFlavor, is_relevant_shared_library,
        scan_signature_reader,
    };

    #[cfg(target_os = "windows")]
    #[test]
    fn explorer_selection_quotes_paths_with_spaces_and_unicode() {
        let path = std::ffi::OsStr::new(r"C:\Program Files (x86)\示例 应用\应用程序.exe");
        assert_eq!(
            explorer_select_argument(path),
            r#"/select,"C:\Program Files (x86)\示例 应用\应用程序.exe""#
        );
    }

    #[test]
    fn candidate_names_are_precise() {
        assert_eq!(
            classify_candidate_name("chrome_100_percent.pak"),
            Some(CandidateKind::Pak)
        );
        assert_eq!(
            classify_candidate_name("libcef.so.123"),
            Some(CandidateKind::Cef)
        );
        assert_eq!(
            classify_candidate_name("libnode.so.115"),
            Some(CandidateKind::Node)
        );
        assert_eq!(classify_candidate_name("libcefdetector.rmeta"), None);
        assert_eq!(classify_candidate_name("libnode_helpers.so"), None);
    }

    #[test]
    fn only_framework_shared_libraries_need_signature_scanning() {
        assert!(is_relevant_shared_library("libcef.so"));
        assert!(is_relevant_shared_library("cefsharp.core.dll"));
        assert!(is_relevant_shared_library("libnw.so"));
        assert!(!is_relevant_shared_library("libvulkan.so"));
        assert!(!is_relevant_shared_library("steamclient.dll"));
    }

    #[test]
    fn signature_scanning_handles_chunk_boundaries() {
        let mut bytes = vec![0_u8; SIGNATURE_CHUNK_SIZE - 5];
        bytes.extend_from_slice(b"third_party/electron_node");
        let mut reader = Cursor::new(bytes);
        assert_eq!(
            scan_signature_reader(&mut reader, ScanFlavor::Standard).unwrap(),
            Some(AppKind::Electron)
        );
    }

    #[test]
    fn stronger_signature_wins_across_chunks() {
        let mut bytes = b"cef_string_utf8_to_utf16".to_vec();
        bytes.resize(SIGNATURE_CHUNK_SIZE + 32, 0);
        bytes.extend_from_slice(b"url-nwjs");
        let mut reader = Cursor::new(bytes);
        assert_eq!(
            scan_signature_reader(&mut reader, ScanFlavor::Standard).unwrap(),
            Some(AppKind::Nwjs)
        );
    }
}

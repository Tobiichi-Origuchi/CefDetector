use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};

use ::ignore::{WalkBuilder, WalkState};

use super::{CandidateSource, ScanCandidate, classify_candidate_name};

#[derive(Default)]
pub(super) struct IgnoreCandidateSource;

#[derive(Default)]
struct IgnoreConfig {
    dir_names: HashSet<String>,
    abs_paths: HashSet<PathBuf>,
}

#[cfg(any(test, target_os = "windows"))]
fn drive_roots_from_mask(mask: u32) -> Vec<PathBuf> {
    (0..26)
        .filter(|index| mask & (1 << index) != 0)
        .map(|index| PathBuf::from(format!("{}:\\", (b'A' + index as u8) as char)))
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn search_roots() -> io::Result<Vec<PathBuf>> {
    Ok(vec![PathBuf::from("/")])
}

#[cfg(target_os = "macos")]
fn ignore_file_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/cefdetector/.ignore")
}

#[cfg(target_os = "windows")]
fn search_roots() -> io::Result<Vec<PathBuf>> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

    // SAFETY: GetLogicalDrives has no parameters and returns a bitmask.
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(drive_roots_from_mask(mask))
}

#[cfg(target_os = "linux")]
fn ignore_file_path() -> PathBuf {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    config_dir.join("cefdetector").join(".ignore")
}

#[cfg(target_os = "windows")]
fn ignore_file_path() -> PathBuf {
    let config_dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default();
    config_dir.join("cefdetector").join(".ignore")
}

fn load_ignore_config() -> IgnoreConfig {
    let mut config = IgnoreConfig::default();
    if let Ok(content) = fs::read_to_string(ignore_file_path()) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if std::path::Path::new(line).is_absolute() {
                config.abs_paths.insert(PathBuf::from(line));
            } else {
                config.dir_names.insert(line.to_owned());
            }
        }
    }

    config
}

#[cfg(target_os = "linux")]
fn is_platform_excluded(path: &std::path::Path) -> bool {
    [
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/tmp",
        "/boot",
        "/lost+found",
    ]
    .iter()
    .any(|root| path.starts_with(root))
}

#[cfg(target_os = "macos")]
fn is_platform_excluded(path: &std::path::Path) -> bool {
    super::super::macos::is_platform_excluded(path)
}

#[cfg(target_os = "windows")]
fn windows_exclusion_roots(
    windows_dir: PathBuf,
    drive_roots: impl IntoIterator<Item = PathBuf>,
) -> Vec<PathBuf> {
    let mut exclusions = vec![windows_dir.join("servicing"), windows_dir.join("WinSxS")];
    if let Some(system_drive) = windows_dir.ancestors().find(|path| path.parent().is_none()) {
        exclusions.push(system_drive.join("Recovery"));
    }
    for drive in drive_roots {
        exclusions.push(drive.join("$Recycle.Bin"));
        exclusions.push(drive.join("System Volume Information"));
    }
    exclusions
}

#[cfg(target_os = "windows")]
fn ascii_path_unit(unit: u16) -> u16 {
    match unit {
        0x41..=0x5a => unit + u16::from(b'a' - b'A'),
        0x2f => u16::from(b'\\'),
        _ => unit,
    }
}

#[cfg(target_os = "windows")]
fn path_starts_with_ignore_ascii_case(path: &std::path::Path, root: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt as _;

    let mut path_units = path.as_os_str().encode_wide();
    for expected in root.as_os_str().encode_wide() {
        let Some(actual) = path_units.next() else {
            return false;
        };
        if ascii_path_unit(actual) != ascii_path_unit(expected) {
            return false;
        }
    }
    path_units
        .next()
        .is_none_or(|unit| ascii_path_unit(unit) == u16::from(b'\\'))
}

#[cfg(target_os = "windows")]
fn is_platform_excluded(path: &std::path::Path) -> bool {
    static EXCLUSIONS: LazyLock<Vec<PathBuf>> = LazyLock::new(|| {
        let windows_dir = std::env::var_os("SystemRoot")
            .or_else(|| std::env::var_os("WINDIR"))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        windows_exclusion_roots(windows_dir, search_roots().unwrap_or_default())
    });

    EXCLUSIONS
        .iter()
        .any(|root| path_starts_with_ignore_ascii_case(path, root))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn path_is_ignored(config: &IgnoreConfig, path: &std::path::Path) -> bool {
    config.abs_paths.contains(path)
}

#[cfg(target_os = "windows")]
fn path_is_ignored(config: &IgnoreConfig, path: &std::path::Path) -> bool {
    let path = path.to_string_lossy();
    config
        .abs_paths
        .iter()
        .any(|ignored| path.eq_ignore_ascii_case(&ignored.to_string_lossy()))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn directory_name_is_ignored(config: &IgnoreConfig, name: &std::ffi::OsStr) -> bool {
    config.dir_names.contains(name.to_string_lossy().as_ref())
}

#[cfg(target_os = "windows")]
fn directory_name_is_ignored(config: &IgnoreConfig, name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    config
        .dir_names
        .iter()
        .any(|ignored| name.eq_ignore_ascii_case(ignored))
}

impl CandidateSource for IgnoreCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let ignore_config = load_ignore_config();

        let mut roots = search_roots()?.into_iter();
        let first_root = roots
            .next()
            .ok_or_else(|| io::Error::other("no filesystem roots are available to scan"))?;
        let mut builder = WalkBuilder::new(first_root);
        for root in roots {
            builder.add(root);
        }
        builder
            .standard_filters(false)
            .hidden(false)
            .threads(
                std::thread::available_parallelism()
                    .map(|count| count.get().min(8))
                    .unwrap_or(4),
            )
            .filter_entry(move |entry| {
                let path = entry.path();
                if is_platform_excluded(path) {
                    return false;
                }

                if entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_dir())
                {
                    if path_is_ignored(&ignore_config, path) {
                        return false;
                    }
                    if let Some(name) = path.file_name()
                        && directory_name_is_ignored(&ignore_config, name)
                    {
                        return false;
                    }
                }

                true
            });

        builder.build_parallel().run(|| {
            let results = Arc::clone(&results);
            Box::new(move |result| {
                if let Ok(entry) = result
                    && entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_file())
                    && let Some(kind) =
                        classify_candidate_name(entry.file_name().to_string_lossy().as_ref())
                {
                    results
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(ScanCandidate {
                            path: entry.into_path(),
                            kind,
                            #[cfg(target_os = "macos")]
                            application_root_hint: None,
                        });
                }
                WalkState::Continue
            })
        });

        let mut guard = results
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut results = std::mem::take(&mut *guard);
        results.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::drive_roots_from_mask;
    #[cfg(target_os = "macos")]
    use super::is_platform_excluded;
    #[cfg(target_os = "windows")]
    use super::{path_starts_with_ignore_ascii_case, windows_exclusion_roots};

    #[test]
    fn windows_drive_mask_maps_to_root_paths() {
        assert_eq!(
            drive_roots_from_mask((1 << 2) | (1 << 25)),
            [PathBuf::from("C:\\"), PathBuf::from("Z:\\")]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_exclusions_avoid_duplicate_and_private_system_trees() {
        for path in [
            "/System/Volumes/Data/Applications",
            "/private/var/folders/zz/cache",
            "/Volumes/Disk/.Spotlight-V100/store",
            "/Volumes/Backups/Backups.backupdb/Mac",
        ] {
            assert!(is_platform_excluded(std::path::Path::new(path)), "{path}");
        }
        assert!(!is_platform_excluded(std::path::Path::new(
            "/System/Applications/Safari.app"
        )));
        assert!(!is_platform_excluded(std::path::Path::new(
            "/Volumes/External/Apps/Demo.app"
        )));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_system_exclusions_are_scoped_to_exact_directories() {
        let exclusions = windows_exclusion_roots(
            PathBuf::from(r"C:\Windows"),
            [PathBuf::from(r"C:\"), PathBuf::from(r"D:\")],
        );
        let is_excluded = |path| {
            exclusions
                .iter()
                .any(|root| path_starts_with_ignore_ascii_case(path, root))
        };

        assert!(is_excluded(std::path::Path::new(
            r"c:\WINDOWS\servicing\Packages"
        )));
        assert!(is_excluded(std::path::Path::new(
            r"C:\Windows\WinSxS\ManifestCache"
        )));
        assert!(is_excluded(std::path::Path::new(
            r"D:\$Recycle.Bin\deleted-app"
        )));
        assert!(is_excluded(std::path::Path::new(
            r"D:\System Volume Information"
        )));
        assert!(!is_excluded(std::path::Path::new(
            r"C:\Windows\WinSxSBackup"
        )));
        assert!(!is_excluded(std::path::Path::new(
            r"C:\Program Files\WindowsApps"
        )));
        assert!(!is_excluded(std::path::Path::new(r"D:\Windows\WinSxS")));
    }
}

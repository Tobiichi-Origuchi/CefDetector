use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;
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

#[cfg(target_os = "linux")]
fn search_roots() -> io::Result<Vec<PathBuf>> {
    Ok(vec![PathBuf::from("/")])
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

#[cfg(target_os = "windows")]
fn is_platform_excluded(_path: &std::path::Path) -> bool {
    false
}

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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

    #[test]
    fn windows_drive_mask_maps_to_root_paths() {
        assert_eq!(
            drive_roots_from_mask((1 << 2) | (1 << 25)),
            [PathBuf::from("C:\\"), PathBuf::from("Z:\\")]
        );
    }
}

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};

use ::ignore::{WalkBuilder, WalkState};

use crate::config::SearchConfig;

use super::{CandidateSource, ScanCandidate, classify_candidate_name};

#[derive(Default)]
pub(super) struct IgnoreCandidateSource;

#[derive(Clone, Default)]
pub(in crate::search) struct CandidateFilter {
    roots: Option<Vec<PathBuf>>,
    dir_names: HashSet<String>,
    abs_paths: HashSet<PathBuf>,
    use_platform_excludes: bool,
    include_trash: bool,
}

#[cfg(any(test, target_os = "windows"))]
fn drive_roots_from_mask(mask: u32) -> Vec<PathBuf> {
    (0..26)
        .filter(|index| mask & (1 << index) != 0)
        .map(|index| PathBuf::from(format!("{}:\\", (b'A' + index as u8) as char)))
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn default_search_roots() -> io::Result<Vec<PathBuf>> {
    Ok(vec![PathBuf::from("/")])
}

#[cfg(target_os = "windows")]
fn default_search_roots() -> io::Result<Vec<PathBuf>> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

    // SAFETY: GetLogicalDrives has no parameters and returns a bitmask.
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(drive_roots_from_mask(mask))
}

impl CandidateFilter {
    pub(in crate::search) fn load(config: &SearchConfig) -> Self {
        Self {
            roots: config.roots.clone(),
            dir_names: config.exclude_directory_names.iter().cloned().collect(),
            abs_paths: config.exclude_paths.iter().cloned().collect(),
            use_platform_excludes: config.use_platform_excludes,
            include_trash: config.include_trash,
        }
    }

    pub(in crate::search) fn allows(&self, path: &std::path::Path) -> bool {
        if self
            .roots
            .as_ref()
            .is_some_and(|roots| !roots.iter().any(|root| path_has_prefix(path, root)))
        {
            return false;
        }
        if self
            .abs_paths
            .iter()
            .any(|ignored| path_has_prefix(path, ignored))
        {
            return false;
        }
        if path
            .components()
            .any(|component| directory_name_is_ignored(self, component.as_os_str()))
        {
            return false;
        }
        if !self.include_trash && is_trash_path(path) {
            return false;
        }
        !self.use_platform_excludes || !is_platform_excluded(path)
    }
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
        windows_exclusion_roots(windows_dir, default_search_roots().unwrap_or_default())
    });

    EXCLUSIONS
        .iter()
        .any(|root| path_starts_with_ignore_ascii_case(path, root))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn path_has_prefix(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.starts_with(root)
}

#[cfg(target_os = "windows")]
fn path_has_prefix(path: &std::path::Path, root: &std::path::Path) -> bool {
    path_starts_with_ignore_ascii_case(path, root)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn directory_name_is_ignored(config: &CandidateFilter, name: &std::ffi::OsStr) -> bool {
    config.dir_names.contains(name.to_string_lossy().as_ref())
}

#[cfg(target_os = "windows")]
fn directory_name_is_ignored(config: &CandidateFilter, name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    config
        .dir_names
        .iter()
        .any(|ignored| name.eq_ignore_ascii_case(ignored))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn directory_name_equals(name: &std::ffi::OsStr, expected: &str) -> bool {
    name == expected
}

#[cfg(target_os = "windows")]
fn directory_name_equals(name: &std::ffi::OsStr, expected: &str) -> bool {
    name.to_string_lossy().eq_ignore_ascii_case(expected)
}

fn is_trash_path(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        [".Trash", "Trash", ".Trashes", "$Recycle.Bin"]
            .iter()
            .any(|name| directory_name_equals(component.as_os_str(), name))
    })
}

impl CandidateSource for IgnoreCandidateSource {
    fn find_candidates(
        &self,
        config: &SearchConfig,
        filter: &CandidateFilter,
    ) -> io::Result<Vec<ScanCandidate>> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let candidate_filter = filter.clone();

        let mut roots = config
            .roots
            .clone()
            .map(Ok)
            .unwrap_or_else(default_search_roots)?
            .into_iter();
        let first_root = roots
            .next()
            .ok_or_else(|| io::Error::other("no filesystem roots are available to scan"))?;
        let mut builder = WalkBuilder::new(first_root);
        for root in roots {
            builder.add(root);
        }
        builder
            .standard_filters(false)
            .hidden(!config.include_hidden)
            .parents(config.respect_gitignore)
            .ignore(false)
            .git_global(config.respect_gitignore)
            .git_ignore(config.respect_gitignore)
            .git_exclude(config.respect_gitignore)
            .follow_links(config.follow_symlinks)
            .same_file_system(config.same_filesystem)
            .threads(if config.walk_threads == 0 {
                std::thread::available_parallelism()
                    .map(|count| count.get().min(8))
                    .unwrap_or(4)
            } else {
                config.walk_threads
            })
            .filter_entry(move |entry| {
                let path = entry.path();
                !entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_dir())
                    || candidate_filter.allows(path)
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::drive_roots_from_mask;
    #[cfg(target_os = "macos")]
    use super::is_platform_excluded;
    #[cfg(target_os = "windows")]
    use super::{path_starts_with_ignore_ascii_case, windows_exclusion_roots};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn windows_drive_mask_maps_to_root_paths() {
        assert_eq!(
            drive_roots_from_mask((1 << 2) | (1 << 25)),
            [PathBuf::from("C:\\"), PathBuf::from("Z:\\")]
        );
    }

    #[test]
    fn dot_ignore_files_do_not_change_search_scope() {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "cefdetector-ignore-backend-{}-{sequence}",
            std::process::id()
        ));
        let ignored_directory = root.join("ignored");
        fs::create_dir_all(&ignored_directory).unwrap();
        fs::write(root.join(".ignore"), "ignored\n").unwrap();
        let candidate = ignored_directory.join(if cfg!(target_os = "windows") {
            "libcef.dll"
        } else {
            "libcef.so"
        });
        fs::write(&candidate, []).unwrap();

        let config = crate::config::SearchConfig {
            roots: Some(vec![root.clone()]),
            use_platform_excludes: false,
            include_trash: true,
            respect_gitignore: true,
            ..Default::default()
        };
        let filter = super::CandidateFilter::load(&config);
        let result = crate::search::backend::CandidateSource::find_candidates(
            &super::IgnoreCandidateSource,
            &config,
            &filter,
        );
        let _ = fs::remove_dir_all(&root);

        let candidates = result.unwrap();
        assert!(
            candidates.iter().any(|found| found.path == candidate),
            ".ignore unexpectedly removed {candidate:?} from {candidates:?}"
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
        assert!(!is_excluded(std::path::Path::new(
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn configured_scope_applies_to_all_candidate_backends() {
        let mut config = crate::config::SearchConfig {
            roots: Some(vec![PathBuf::from("/allowed")]),
            exclude_paths: vec![PathBuf::from("/allowed/private")],
            exclude_directory_names: vec!["cache".to_owned()],
            use_platform_excludes: false,
            ..Default::default()
        };
        let filter = super::CandidateFilter::load(&config);

        assert!(filter.allows(std::path::Path::new("/allowed/app/libcef.so")));
        assert!(!filter.allows(std::path::Path::new("/other/app/libcef.so")));
        assert!(!filter.allows(std::path::Path::new("/allowed/private/app/libcef.so")));
        assert!(!filter.allows(std::path::Path::new("/allowed/cache/app/libcef.so")));
        assert!(!filter.allows(std::path::Path::new("/allowed/.Trash/app/libcef.so")));

        config.include_trash = true;
        assert!(
            super::CandidateFilter::load(&config)
                .allows(std::path::Path::new("/allowed/.Trash/app/libcef.so"))
        );
    }
}

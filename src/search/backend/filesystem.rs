use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const EXCLUDED_ROOTS: [&str; 7] = [
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/tmp",
    "/boot",
    "/lost+found",
];

#[derive(Default)]
pub(super) struct WalkFilter {
    dir_names: HashSet<String>,
    abs_paths: HashSet<PathBuf>,
}

impl WalkFilter {
    pub(super) fn load() -> Self {
        let mut filter = Self::default();
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_default();
                PathBuf::from(home).join(".config")
            });

        let ignore_file = config_dir.join("cefdetector").join(".ignore");
        if let Ok(content) = fs::read_to_string(ignore_file) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.starts_with('/') {
                    filter.abs_paths.insert(PathBuf::from(line));
                } else {
                    filter.dir_names.insert(line.to_owned());
                }
            }
        }

        filter
    }

    pub(super) fn should_descend(&self, path: &Path) -> bool {
        if EXCLUDED_ROOTS.iter().any(|root| path.starts_with(root)) {
            return false;
        }
        if self.abs_paths.contains(path) {
            return false;
        }
        !path
            .file_name()
            .is_some_and(|name| self.dir_names.contains(name.to_string_lossy().as_ref()))
    }
}

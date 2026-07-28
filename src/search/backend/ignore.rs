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

fn load_ignore_config() -> IgnoreConfig {
    let mut config = IgnoreConfig::default();
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
                config.abs_paths.insert(PathBuf::from(line));
            } else {
                config.dir_names.insert(line.to_owned());
            }
        }
    }

    config
}

impl CandidateSource for IgnoreCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let ignore_config = load_ignore_config();

        let mut builder = WalkBuilder::new("/");
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
                if [
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
                {
                    return false;
                }

                if entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_dir())
                {
                    if ignore_config.abs_paths.contains(path) {
                        return false;
                    }
                    if let Some(name) = path.file_name()
                        && ignore_config
                            .dir_names
                            .contains(name.to_string_lossy().as_ref())
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

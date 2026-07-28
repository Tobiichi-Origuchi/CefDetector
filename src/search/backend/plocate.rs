use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Condvar, Mutex};

use super::filesystem::WalkFilter;
use super::{CandidateSource, ScanCandidate, classify_candidate_name};

const PLOCATE_QUERIES: [&str; 4] = ["_100_", "libcef", "Chromium Embedded Framework", "libnode"];

#[derive(Default)]
pub(super) struct PlocateCandidateSource;

fn parse_paths(stdout: &[u8]) -> impl Iterator<Item = PathBuf> + '_ {
    stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsString::from_vec(path.to_vec())))
}

fn no_matches(output: &Output) -> bool {
    output.status.code() == Some(1)
        && output.stdout.is_empty()
        && output.stderr.iter().all(u8::is_ascii_whitespace)
}

fn query_error(query: &str, output: &Output) -> io::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let details = stderr.trim();
    let status = output.status.code().map_or_else(
        || "terminated by signal".to_owned(),
        |code| code.to_string(),
    );

    if details.is_empty() {
        io::Error::other(format!(
            "plocate query {query:?} failed with status {status}"
        ))
    } else {
        io::Error::other(format!(
            "plocate query {query:?} failed with status {status}: {details}"
        ))
    }
}

fn start_error(error: io::Error) -> io::Error {
    let message = if error.kind() == io::ErrorKind::NotFound {
        "could not start plocate; install plocate and make sure it is available in PATH".to_owned()
    } else {
        format!("could not start plocate: {error}")
    };
    io::Error::new(error.kind(), message)
}

fn run_query(query: &str) -> io::Result<Output> {
    Command::new("plocate")
        .args(["--null", "--literal", "--basename", query])
        .output()
        .map_err(start_error)
}

fn index_covers(root: &Path) -> io::Result<bool> {
    let mut prefix = root.as_os_str().to_os_string();
    if !prefix.as_bytes().ends_with(b"/") {
        prefix.push("/");
    }

    let output = Command::new("plocate")
        .args(["--null", "--literal", "--limit", "1"])
        .arg(&prefix)
        .output()
        .map_err(start_error)?;
    if output.status.success() {
        return Ok(!output.stdout.is_empty());
    }
    if no_matches(&output) {
        return Ok(false);
    }
    Err(query_error(&prefix.to_string_lossy(), &output))
}

fn decode_mount_path(encoded: &str) -> PathBuf {
    let encoded = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'\\'
            && index + 3 < encoded.len()
            && matches!(encoded[index + 1], b'0'..=b'3')
            && encoded[index + 2..=index + 3]
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'7'))
        {
            let value = u16::from(encoded[index + 1] - b'0') * 64
                + u16::from(encoded[index + 2] - b'0') * 8
                + u16::from(encoded[index + 3] - b'0');
            decoded.push(value as u8);
            index += 4;
        } else {
            decoded.push(encoded[index]);
            index += 1;
        }
    }
    PathBuf::from(OsString::from_vec(decoded))
}

// updatedb may omit a Btrfs home subvolume when PRUNE_BIND_MOUNTS is enabled.
// A separately mounted game library below an indexed home can be omitted as well.
fn home_and_mount_points() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut roots = vec![home.clone()];

    if let Ok(mount_info) = fs::read_to_string("/proc/self/mountinfo") {
        roots.extend(mount_info.lines().filter_map(|line| {
            let mount_point = decode_mount_path(line.split_ascii_whitespace().nth(4)?);
            (mount_point != home && mount_point.starts_with(&home)).then_some(mount_point)
        }));
    }

    roots
}

// Prefer the shallowest uncovered root so that a missing home index results in
// one home scan instead of redundant scans for each mount below it.
fn select_unindexed_roots<F>(mut roots: Vec<PathBuf>, mut is_indexed: F) -> io::Result<Vec<PathBuf>>
where
    F: FnMut(&Path) -> io::Result<bool>,
{
    roots.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    roots.dedup();

    let mut selected: Vec<PathBuf> = Vec::new();
    for root in roots {
        if selected.iter().any(|parent| root.starts_with(parent)) {
            continue;
        }
        if !is_indexed(&root)? {
            selected.push(root);
        }
    }
    Ok(selected)
}

fn scan_unindexed_roots(
    roots: Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    candidates: &mut Vec<ScanCandidate>,
) {
    if roots.is_empty() {
        return;
    }

    struct DirectoryQueue {
        pending: VecDeque<PathBuf>,
        active_workers: usize,
    }

    let filter = WalkFilter::load();
    let queue = (
        Mutex::new(DirectoryQueue {
            pending: roots.into(),
            active_workers: 0,
        }),
        Condvar::new(),
    );
    let discovered = Mutex::new(Vec::new());
    let worker_count = std::thread::available_parallelism()
        .map(|count| count.get().min(8))
        .unwrap_or(4);
    const DIRECTORY_BATCH_SIZE: usize = 4;

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let filter = &filter;
            let queue = &queue;
            let discovered = &discovered;
            scope.spawn(move || {
                loop {
                    let directories = {
                        let (state, available) = queue;
                        let mut state = state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        loop {
                            if !state.pending.is_empty() {
                                let mut directories = Vec::with_capacity(DIRECTORY_BATCH_SIZE);
                                for _ in 0..DIRECTORY_BATCH_SIZE {
                                    let Some(directory) = state.pending.pop_front() else {
                                        break;
                                    };
                                    directories.push(directory);
                                }
                                state.active_workers += 1;
                                break Some(directories);
                            }
                            if state.active_workers == 0 {
                                available.notify_all();
                                break None;
                            }
                            state = available
                                .wait(state)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                    };
                    let Some(directories) = directories else {
                        break;
                    };

                    let mut child_directories = Vec::new();
                    let mut local_candidates = Vec::new();
                    for directory in directories {
                        if filter.should_descend(&directory)
                            && let Ok(entries) = fs::read_dir(directory)
                        {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                let Ok(file_type) = entry.file_type() else {
                                    continue;
                                };
                                if file_type.is_dir() {
                                    if filter.should_descend(&path) {
                                        child_directories.push(path);
                                    }
                                    continue;
                                }
                                if !file_type.is_file() {
                                    continue;
                                }
                                let Some(kind) = classify_candidate_name(
                                    entry.file_name().to_string_lossy().as_ref(),
                                ) else {
                                    continue;
                                };
                                local_candidates.push(ScanCandidate { path, kind });
                            }
                        }
                    }

                    if !local_candidates.is_empty() {
                        discovered
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .extend(local_candidates);
                    }

                    let (state, available) = queue;
                    let mut state = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.pending.extend(child_directories);
                    state.active_workers -= 1;
                    available.notify_all();
                }
            });
        }
    });

    let discovered = discovered
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for candidate in discovered {
        if seen.insert(candidate.path.clone()) {
            candidates.push(candidate);
        }
    }
}

impl CandidateSource for PlocateCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();

        for query in PLOCATE_QUERIES {
            let output = run_query(query)?;
            if !output.status.success() {
                if no_matches(&output) {
                    continue;
                }
                return Err(query_error(query, &output));
            }

            for path in parse_paths(&output.stdout) {
                if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                    continue;
                }
                let Some(file_name) = path.file_name() else {
                    continue;
                };
                let Some(kind) = classify_candidate_name(file_name.to_string_lossy().as_ref())
                else {
                    continue;
                };
                if seen.insert(path.clone()) {
                    candidates.push(ScanCandidate { path, kind });
                }
            }
        }

        let fallback_roots = select_unindexed_roots(home_and_mount_points(), index_covers)?;
        scan_unindexed_roots(fallback_roots, &mut seen, &mut candidates);

        candidates.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    use super::{decode_mount_path, parse_paths, select_unindexed_roots};

    #[test]
    fn nul_output_preserves_newlines_and_non_utf8_paths() {
        let output = b"/opt/first\napp/libcef.so\0/opt/\xff/libnode.so\0";
        let paths: Vec<_> = parse_paths(output).collect();

        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0].as_os_str().as_bytes(),
            b"/opt/first\napp/libcef.so"
        );
        assert_eq!(paths[1].as_os_str().as_bytes(), b"/opt/\xff/libnode.so");
    }

    #[test]
    fn nul_output_ignores_empty_records() {
        let paths: Vec<_> = parse_paths(b"\0/opt/libcef.so\0\0").collect();
        assert_eq!(paths, [PathBuf::from("/opt/libcef.so")]);
    }

    #[test]
    fn mountinfo_paths_are_unescaped() {
        assert_eq!(
            decode_mount_path("/home/user/My\\040Games/line\\012break"),
            PathBuf::from("/home/user/My Games/line\nbreak")
        );
    }

    #[test]
    fn uncovered_parent_avoids_redundant_mount_scans() {
        let home = PathBuf::from("/home/user");
        let steam = home.join(".local/share/Steam");
        let roots = select_unindexed_roots(vec![steam, home.clone()], |_| Ok(false)).unwrap();
        assert_eq!(roots, [home]);
    }

    #[test]
    fn uncovered_mount_is_scanned_when_home_is_indexed() {
        let home = PathBuf::from("/home/user");
        let steam = home.join(".local/share/Steam");
        let roots =
            select_unindexed_roots(vec![home.clone(), steam.clone()], |path| Ok(path == home))
                .unwrap();
        assert_eq!(roots, [steam]);
    }
}

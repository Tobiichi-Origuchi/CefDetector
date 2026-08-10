use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read as _};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::config::SearchConfig;

use super::{CandidateFilter, CandidateSource, ScanCandidate, classify_candidate_name};

const PLOCATE_QUERIES: [&str; 4] = ["_100_", "libcef", "Chromium Embedded Framework", "libnode"];
const MAX_STDOUT_BYTES: usize = 128 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;

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

fn run_query(config: &SearchConfig, query: &str) -> io::Result<Output> {
    let mut child = Command::new(&config.plocate.command)
        .args(["--null", "--literal", "--basename", query])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            let message = if error.kind() == io::ErrorKind::NotFound {
                format!(
                    "could not start plocate command {:?}; install plocate or correct search.plocate.command",
                    config.plocate.command
                )
            } else {
                format!(
                    "could not start plocate command {:?}: {error}",
                    config.plocate.command
                )
            };
            io::Error::new(error.kind(), message)
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture plocate stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("failed to capture plocate stderr"))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_STDOUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .take(MAX_STDERR_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let timeout = Duration::from_millis(config.plocate.timeout_ms);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "plocate query did not finish within {} ms",
                    config.plocate.timeout_ms
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("plocate stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("plocate stderr reader panicked"))??;
    if stdout.len() > MAX_STDOUT_BYTES {
        return Err(io::Error::other(
            "plocate output exceeded the 128 MiB safety limit",
        ));
    }
    if stderr.len() > MAX_STDERR_BYTES {
        return Err(io::Error::other(
            "plocate error output exceeded the 64 KiB safety limit",
        ));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

impl CandidateSource for PlocateCandidateSource {
    fn find_candidates(
        &self,
        config: &SearchConfig,
        _filter: &CandidateFilter,
    ) -> io::Result<Vec<ScanCandidate>> {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();

        for query in PLOCATE_QUERIES {
            let output = run_query(config, query)?;
            if !output.status.success() {
                if no_matches(&output) {
                    continue;
                }
                return Err(query_error(query, &output));
            }

            for path in parse_paths(&output.stdout) {
                if !seen.insert(path.clone()) {
                    continue;
                }
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
                candidates.push(ScanCandidate {
                    path,
                    kind,
                    #[cfg(target_os = "macos")]
                    application_root_hint: None,
                });
            }
        }

        candidates.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::process::{ExitStatus, Output};

    use super::{no_matches, parse_paths};

    fn failed_output(stderr: &[u8]) -> Output {
        use std::os::unix::process::ExitStatusExt as _;

        Output {
            status: ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: stderr.to_vec(),
        }
    }

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
    fn empty_status_one_is_treated_as_no_matches() {
        assert!(no_matches(&failed_output(b"\n")));
    }

    #[test]
    fn database_permission_error_is_not_treated_as_no_matches() {
        assert!(!no_matches(&failed_output(
            b"/var/lib/plocate/plocate.db: Permission denied\n"
        )));
    }
}

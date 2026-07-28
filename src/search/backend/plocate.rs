use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Output};

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

fn run_query(query: &str) -> io::Result<Output> {
    Command::new("plocate")
        .args(["--null", "--literal", "--basename", query])
        .output()
        .map_err(|error| {
            let message = if error.kind() == io::ErrorKind::NotFound {
                "could not start plocate; install plocate and make sure it is available in PATH"
                    .to_owned()
            } else {
                format!("could not start plocate: {error}")
            };
            io::Error::new(error.kind(), message)
        })
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
                candidates.push(ScanCandidate { path, kind });
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

    use super::parse_paths;

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
}

use std::io;
use std::path::PathBuf;

#[cfg(all(feature = "index", target_os = "windows"))]
mod everything;
#[cfg(any(test, all(feature = "index", target_os = "windows")))]
mod everything_protocol;
mod ignore;
#[cfg(all(feature = "index", target_os = "linux"))]
mod plocate;
#[cfg(all(feature = "index", target_os = "macos"))]
mod spotlight;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
compile_error!("cefdetector supports Linux, Windows, and macOS");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum CandidateKind {
    Pak,
    Cef,
    Node,
}

#[derive(Clone, Debug)]
pub(super) struct ScanCandidate {
    pub(super) path: PathBuf,
    pub(super) kind: CandidateKind,
    #[cfg(target_os = "macos")]
    pub(super) application_root_hint: Option<PathBuf>,
}

/// Finds the small set of files that the shared detection pipeline must inspect.
///
/// Implementations should only discover candidates. Binary inspection, application
/// grouping, size calculation, and process matching remain backend-independent.
pub(super) trait CandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>>;
}

#[cfg(any(test, feature = "index"))]
/// Uses the real indexed lookup as its availability check.
///
/// A separate probe could still race with the subsequent query and would add
/// latency without proving that the complete operation works. Falling back on
/// the real operation's error is both stronger and has no probe overhead.
fn indexed_or_fallback<T>(
    indexed: impl FnOnce() -> io::Result<T>,
    fallback: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    match indexed() {
        Ok(result) => Ok(result),
        Err(indexed_error) => fallback().map_err(|fallback_error| {
            io::Error::new(
                fallback_error.kind(),
                format!(
                    "indexed search failed ({indexed_error}); filesystem fallback failed ({fallback_error})"
                ),
            )
        }),
    }
}

#[cfg(all(feature = "index", target_os = "linux"))]
pub(super) fn find_candidates() -> io::Result<Vec<ScanCandidate>> {
    indexed_or_fallback(
        || plocate::PlocateCandidateSource.find_candidates(),
        || ignore::IgnoreCandidateSource.find_candidates(),
    )
}

#[cfg(all(feature = "index", target_os = "windows"))]
pub(super) fn find_candidates() -> io::Result<Vec<ScanCandidate>> {
    indexed_or_fallback(
        || everything::EverythingCandidateSource.find_candidates(),
        || ignore::IgnoreCandidateSource.find_candidates(),
    )
}

#[cfg(all(feature = "index", target_os = "macos"))]
pub(super) fn find_candidates() -> io::Result<Vec<ScanCandidate>> {
    indexed_or_fallback(
        || spotlight::SpotlightCandidateSource.find_candidates(),
        || ignore::IgnoreCandidateSource.find_candidates(),
    )
}

#[cfg(not(feature = "index"))]
pub(super) fn find_candidates() -> io::Result<Vec<ScanCandidate>> {
    ignore::IgnoreCandidateSource.find_candidates()
}

pub(super) fn classify_candidate_name(name: &str) -> Option<CandidateKind> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let name = name.to_ascii_lowercase();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let name = name.as_str();

    if name.contains("_100_") && name.ends_with(".pak") {
        Some(CandidateKind::Pak)
    } else if name == "libcef.so"
        || name.starts_with("libcef.so.")
        || name == "libcef.dll"
        || name == "libcef.dylib"
        || name == "Chromium Embedded Framework"
        || name == "chromium embedded framework"
        || name == "Electron Framework"
        || name == "electron framework"
    {
        Some(CandidateKind::Cef)
    } else if name == "libnode.so"
        || name.starts_with("libnode.so.")
        || name == "libnode.dll"
        || name == "libnode.dylib"
    {
        Some(CandidateKind::Node)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::io;

    use super::indexed_or_fallback;

    #[test]
    fn successful_index_search_does_not_touch_fallback() {
        let fallback_called = Cell::new(false);
        let result = indexed_or_fallback(
            || Ok(7),
            || {
                fallback_called.set(true);
                Ok(9)
            },
        );

        assert_eq!(result.unwrap(), 7);
        assert!(!fallback_called.get());
    }

    #[test]
    fn failed_index_search_uses_filesystem_fallback() {
        let result = indexed_or_fallback(
            || Err(io::Error::other("index unavailable")),
            || Ok::<_, io::Error>(9),
        );

        assert_eq!(result.unwrap(), 9);
    }

    #[test]
    fn failure_reports_both_backend_errors() {
        let error = indexed_or_fallback::<()>(
            || Err(io::Error::other("index unavailable")),
            || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "scan denied",
                ))
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("index unavailable"));
        assert!(error.to_string().contains("scan denied"));
    }
}

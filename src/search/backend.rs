use std::io;
use std::path::PathBuf;

use crate::config::{SearchBackend, SearchConfig};

#[cfg(all(feature = "index", target_os = "windows"))]
mod everything;
#[cfg(any(test, all(feature = "index", target_os = "windows")))]
mod everything_protocol;
mod ignore;
#[cfg(all(feature = "index", target_os = "linux"))]
mod plocate;
#[cfg(all(feature = "index", target_os = "macos"))]
mod spotlight;

pub(super) use ignore::CandidateFilter;

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
    fn find_candidates(
        &self,
        config: &SearchConfig,
        filter: &CandidateFilter,
    ) -> io::Result<Vec<ScanCandidate>>;
}

#[cfg(any(test, feature = "index"))]
/// Uses the real indexed lookup as its availability check.
///
/// A separate probe could still race with the subsequent query and would add
/// latency without proving that the complete operation works. Falling back on
/// the real operation's error is both stronger and has no probe overhead.
fn indexed_or_fallback<T>(
    indexed_backend: &str,
    fallback_enabled: bool,
    report_backend: bool,
    indexed: impl FnOnce() -> io::Result<T>,
    fallback: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    match indexed() {
        Ok(result) => {
            if report_backend {
                eprintln!("cefdetector-search-backend={indexed_backend}");
            }
            Ok(result)
        }
        Err(indexed_error) if !fallback_enabled => Err(indexed_error),
        Err(indexed_error) => match fallback() {
            Ok(result) => {
                if report_backend {
                    eprintln!("cefdetector-search-backend=ignore");
                }
                Ok(result)
            }
            Err(fallback_error) => Err(io::Error::new(
                fallback_error.kind(),
                format!(
                    "indexed search failed ({indexed_error}); filesystem fallback failed ({fallback_error})"
                ),
            )),
        },
    }
}

#[cfg(all(feature = "index", target_os = "linux"))]
fn find_indexed_candidates(
    config: &SearchConfig,
    filter: &CandidateFilter,
    report_backend: bool,
) -> io::Result<Vec<ScanCandidate>> {
    indexed_or_fallback(
        "plocate",
        config.index_fallback,
        report_backend,
        || plocate::PlocateCandidateSource.find_candidates(config, filter),
        || ignore::IgnoreCandidateSource.find_candidates(config, filter),
    )
}

#[cfg(all(feature = "index", target_os = "windows"))]
fn find_indexed_candidates(
    config: &SearchConfig,
    filter: &CandidateFilter,
    report_backend: bool,
) -> io::Result<Vec<ScanCandidate>> {
    indexed_or_fallback(
        "everything",
        config.index_fallback,
        report_backend,
        || everything::EverythingCandidateSource.find_candidates(config, filter),
        || ignore::IgnoreCandidateSource.find_candidates(config, filter),
    )
}

#[cfg(all(feature = "index", target_os = "macos"))]
fn find_indexed_candidates(
    config: &SearchConfig,
    filter: &CandidateFilter,
    report_backend: bool,
) -> io::Result<Vec<ScanCandidate>> {
    indexed_or_fallback(
        "spotlight",
        config.index_fallback,
        report_backend,
        || spotlight::SpotlightCandidateSource.find_candidates(config, filter),
        || ignore::IgnoreCandidateSource.find_candidates(config, filter),
    )
}

pub(super) fn find_candidates(
    config: &SearchConfig,
    report_backend: bool,
) -> io::Result<Vec<ScanCandidate>> {
    let filter = CandidateFilter::load(config);
    let candidates = match config.backend {
        SearchBackend::Filesystem => {
            let candidates = ignore::IgnoreCandidateSource.find_candidates(config, &filter)?;
            if report_backend {
                eprintln!("cefdetector-search-backend=ignore");
            }
            candidates
        }
        #[cfg(feature = "index")]
        SearchBackend::Auto | SearchBackend::Index => {
            find_indexed_candidates(config, &filter, report_backend)?
        }
        #[cfg(not(feature = "index"))]
        SearchBackend::Auto => {
            let candidates = ignore::IgnoreCandidateSource.find_candidates(config, &filter)?;
            if report_backend {
                eprintln!("cefdetector-search-backend=ignore");
            }
            candidates
        }
        #[cfg(not(feature = "index"))]
        SearchBackend::Index => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the index backend requires compiling cefdetector with the index feature",
            ));
        }
    };

    Ok(candidates
        .into_iter()
        .filter(|candidate| filter.allows(&candidate.path))
        .collect())
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
            "test-index",
            true,
            false,
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
            "test-index",
            true,
            false,
            || Err(io::Error::other("index unavailable")),
            || Ok::<_, io::Error>(9),
        );

        assert_eq!(result.unwrap(), 9);
    }

    #[test]
    fn failure_reports_both_backend_errors() {
        let error = indexed_or_fallback::<()>(
            "test-index",
            true,
            false,
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

    #[test]
    fn disabled_fallback_preserves_index_error() {
        let fallback_called = Cell::new(false);
        let error = indexed_or_fallback::<()>(
            "test-index",
            false,
            false,
            || Err(io::Error::other("index unavailable")),
            || {
                fallback_called.set(true);
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "index unavailable");
        assert!(!fallback_called.get());
    }
}

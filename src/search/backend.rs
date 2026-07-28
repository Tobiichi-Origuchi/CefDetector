use std::io;
use std::path::PathBuf;

#[cfg(feature = "everything")]
mod everything;
#[cfg(feature = "ignore")]
mod ignore;
#[cfg(feature = "plocate")]
mod plocate;

#[cfg(not(any(feature = "ignore", feature = "plocate", feature = "everything")))]
compile_error!("a search backend is required; enable exactly one of: ignore, plocate, everything");

#[cfg(any(
    all(feature = "ignore", feature = "plocate"),
    all(feature = "ignore", feature = "everything"),
    all(feature = "plocate", feature = "everything"),
))]
compile_error!(
    "search backends are mutually exclusive; enable exactly one of: ignore, plocate, everything"
);

#[cfg(all(feature = "plocate", not(target_os = "linux")))]
compile_error!("the plocate search backend is only supported on Linux");

#[cfg(all(feature = "everything", not(target_os = "windows")))]
compile_error!("the everything search backend is only supported on Windows");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CandidateKind {
    Pak,
    Cef,
    Node,
}

#[derive(Clone, Debug)]
pub(super) struct ScanCandidate {
    pub(super) path: PathBuf,
    pub(super) kind: CandidateKind,
}

/// Finds the small set of files that the shared detection pipeline must inspect.
///
/// Implementations should only discover candidates. Binary inspection, application
/// grouping, size calculation, and process matching remain backend-independent.
pub(super) trait CandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>>;
}

#[cfg(all(
    not(feature = "ignore"),
    not(feature = "plocate"),
    feature = "everything"
))]
use self::everything::EverythingCandidateSource as ActiveCandidateSource;
#[cfg(feature = "ignore")]
use self::ignore::IgnoreCandidateSource as ActiveCandidateSource;
#[cfg(all(not(feature = "ignore"), feature = "plocate"))]
use self::plocate::PlocateCandidateSource as ActiveCandidateSource;

#[cfg(not(any(feature = "ignore", feature = "plocate", feature = "everything")))]
#[derive(Default)]
struct ActiveCandidateSource;

#[cfg(not(any(feature = "ignore", feature = "plocate", feature = "everything")))]
impl CandidateSource for ActiveCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        unreachable!("the compile-time feature check requires a search backend")
    }
}

pub(super) fn find_candidates() -> io::Result<Vec<ScanCandidate>> {
    ActiveCandidateSource.find_candidates()
}

#[cfg_attr(not(feature = "ignore"), allow(dead_code))]
pub(super) fn classify_candidate_name(name: &str) -> Option<CandidateKind> {
    if name.contains("_100_") && name.ends_with(".pak") {
        Some(CandidateKind::Pak)
    } else if name == "libcef.so"
        || name.starts_with("libcef.so.")
        || name == "libcef.dll"
        || name == "Chromium Embedded Framework"
    {
        Some(CandidateKind::Cef)
    } else if name == "libnode.so" || name.starts_with("libnode.so.") || name == "libnode.dll" {
        Some(CandidateKind::Node)
    } else {
        None
    }
}

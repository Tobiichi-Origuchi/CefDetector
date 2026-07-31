use std::io;
use std::path::PathBuf;

#[cfg(all(feature = "everything", target_os = "windows"))]
mod everything;
#[cfg(any(test, all(feature = "everything", target_os = "windows")))]
mod everything_protocol;
#[cfg(feature = "ignore")]
mod ignore;
#[cfg(all(feature = "plocate", target_os = "linux"))]
mod plocate;
#[cfg(all(feature = "spotlight", target_os = "macos"))]
mod spotlight;

#[cfg(not(any(
    feature = "ignore",
    feature = "plocate",
    feature = "everything",
    feature = "spotlight"
)))]
compile_error!(
    "a search backend is required; enable exactly one of: ignore, plocate, everything, spotlight"
);

#[cfg(any(
    all(feature = "ignore", feature = "plocate"),
    all(feature = "ignore", feature = "everything"),
    all(feature = "ignore", feature = "spotlight"),
    all(feature = "plocate", feature = "everything"),
    all(feature = "plocate", feature = "spotlight"),
    all(feature = "everything", feature = "spotlight"),
))]
compile_error!(
    "search backends are mutually exclusive; enable exactly one of: ignore, plocate, everything, spotlight"
);

#[cfg(all(feature = "plocate", not(target_os = "linux")))]
compile_error!("the plocate search backend is only supported on Linux");

#[cfg(all(feature = "everything", not(target_os = "windows")))]
compile_error!("the everything search backend is only supported on Windows");

#[cfg(all(feature = "spotlight", not(target_os = "macos")))]
compile_error!("the spotlight search backend is only supported on macOS");

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

#[cfg(all(
    feature = "everything",
    not(feature = "ignore"),
    not(feature = "plocate"),
    not(feature = "spotlight"),
    target_os = "windows"
))]
use self::everything::EverythingCandidateSource as ActiveCandidateSource;
#[cfg(all(
    feature = "ignore",
    not(feature = "plocate"),
    not(feature = "everything"),
    not(feature = "spotlight")
))]
use self::ignore::IgnoreCandidateSource as ActiveCandidateSource;
#[cfg(all(
    feature = "plocate",
    not(feature = "ignore"),
    not(feature = "everything"),
    not(feature = "spotlight"),
    target_os = "linux"
))]
use self::plocate::PlocateCandidateSource as ActiveCandidateSource;
#[cfg(all(
    feature = "spotlight",
    not(feature = "ignore"),
    not(feature = "plocate"),
    not(feature = "everything"),
    target_os = "macos"
))]
use self::spotlight::SpotlightCandidateSource as ActiveCandidateSource;

#[cfg(not(any(
    all(
        feature = "ignore",
        not(feature = "plocate"),
        not(feature = "everything"),
        not(feature = "spotlight")
    ),
    all(
        feature = "plocate",
        not(feature = "ignore"),
        not(feature = "everything"),
        not(feature = "spotlight"),
        target_os = "linux"
    ),
    all(
        feature = "everything",
        not(feature = "ignore"),
        not(feature = "plocate"),
        not(feature = "spotlight"),
        target_os = "windows"
    ),
    all(
        feature = "spotlight",
        not(feature = "ignore"),
        not(feature = "plocate"),
        not(feature = "everything"),
        target_os = "macos"
    )
)))]
#[derive(Default)]
struct ActiveCandidateSource;

#[cfg(not(any(
    all(
        feature = "ignore",
        not(feature = "plocate"),
        not(feature = "everything"),
        not(feature = "spotlight")
    ),
    all(
        feature = "plocate",
        not(feature = "ignore"),
        not(feature = "everything"),
        not(feature = "spotlight"),
        target_os = "linux"
    ),
    all(
        feature = "everything",
        not(feature = "ignore"),
        not(feature = "plocate"),
        not(feature = "spotlight"),
        target_os = "windows"
    ),
    all(
        feature = "spotlight",
        not(feature = "ignore"),
        not(feature = "plocate"),
        not(feature = "everything"),
        target_os = "macos"
    )
)))]
impl CandidateSource for ActiveCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        unreachable!("the compile-time feature check requires a search backend")
    }
}

pub(super) fn find_candidates() -> io::Result<Vec<ScanCandidate>> {
    ActiveCandidateSource.find_candidates()
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

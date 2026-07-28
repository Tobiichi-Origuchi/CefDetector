use std::io;

use super::{CandidateSource, ScanCandidate};

#[derive(Default)]
pub(super) struct EverythingCandidateSource;

impl CandidateSource for EverythingCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the Everything search backend interface is present, but its implementation is not available yet",
        ))
    }
}

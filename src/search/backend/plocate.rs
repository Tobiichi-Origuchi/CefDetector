use std::io;

use super::{CandidateSource, ScanCandidate};

#[derive(Default)]
pub(super) struct PlocateCandidateSource;

impl CandidateSource for PlocateCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the plocate search backend interface is present, but its implementation is not available yet",
        ))
    }
}

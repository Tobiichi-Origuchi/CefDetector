use std::io;

use super::{CandidateSource, ScanCandidate};

#[derive(Default)]
pub(super) struct SpotlightCandidateSource;

impl CandidateSource for SpotlightCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        super::super::macos::spotlight_candidates()
    }
}

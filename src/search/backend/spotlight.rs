use std::io;

use crate::config::SearchConfig;

use super::{CandidateFilter, CandidateSource, ScanCandidate};

#[derive(Default)]
pub(super) struct SpotlightCandidateSource;

impl CandidateSource for SpotlightCandidateSource {
    fn find_candidates(
        &self,
        config: &SearchConfig,
        filter: &CandidateFilter,
    ) -> io::Result<Vec<ScanCandidate>> {
        super::super::macos::spotlight_candidates(&config.spotlight, filter)
    }
}

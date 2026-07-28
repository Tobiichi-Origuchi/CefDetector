use std::io;
use std::sync::{Arc, Mutex};

use ::ignore::{WalkBuilder, WalkState};

use super::filesystem::WalkFilter;
use super::{CandidateSource, ScanCandidate, classify_candidate_name};

#[derive(Default)]
pub(super) struct IgnoreCandidateSource;

impl CandidateSource for IgnoreCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let walk_filter = WalkFilter::load();

        let mut builder = WalkBuilder::new("/");
        builder
            .standard_filters(false)
            .hidden(false)
            .threads(
                std::thread::available_parallelism()
                    .map(|count| count.get().min(8))
                    .unwrap_or(4),
            )
            .filter_entry(move |entry| {
                if entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_dir())
                {
                    return walk_filter.should_descend(entry.path());
                }

                true
            });

        builder.build_parallel().run(|| {
            let results = Arc::clone(&results);
            Box::new(move |result| {
                if let Ok(entry) = result
                    && entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_file())
                    && let Some(kind) =
                        classify_candidate_name(entry.file_name().to_string_lossy().as_ref())
                {
                    results
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(ScanCandidate {
                            path: entry.into_path(),
                            kind,
                        });
                }
                WalkState::Continue
            })
        });

        let mut guard = results
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut results = std::mem::take(&mut *guard);
        results.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(results)
    }
}

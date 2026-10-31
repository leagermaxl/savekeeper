//! The `Score` phase (SPEC-01 §4.4, SPEC-09 T-09-08): merge, score, default
//! selection, result order and `Totals` by `sk_score::run`.

use sk_core::env::Environment;
use sk_score::ScoreInput;
use time::OffsetDateTime;

use super::run::Run;
use super::{Accumulated, ScanPipeline};

impl ScanPipeline {
    /// Replaces the findings with the merged, scored and sorted ones of
    /// `sk_score::run` and keeps its `Totals` for the report. `now` is the
    /// scan clock of the engine (`started_at`), so one scan scores against
    /// one instant. The issues of the phase go to the report and to the
    /// events in the order `sk_score::run` gives them.
    pub(super) fn score(
        &self,
        env: &Environment,
        now: OffsetDateTime,
        acc: &mut Accumulated,
        run: &Run<'_>,
    ) {
        let input = ScoreInput {
            findings: std::mem::take(&mut acc.findings),
            env,
            now,
        };
        let out = sk_score::run(input, &self.config.scoring);
        acc.findings = out.findings;
        acc.totals = out.totals;
        for issue in out.issues {
            acc.push_issue(issue, run);
        }
    }
}

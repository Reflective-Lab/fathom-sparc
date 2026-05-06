//! `RiskFactorDriftSuggestor` — proposes a year-over-year delta in the count
//! of disclosed Item 1A risk factors.
//!
//! # Reading and writing
//!
//! - Reads from [`ContextKey::Signals`]: expects two or more
//!   [`RiskFactorSection`] facts (JSON-encoded in `ContextFact.content`) for
//!   the same CIK across consecutive fiscal years.
//! - Writes to [`ContextKey::Proposals`]: emits one [`RiskFactorDrift`]
//!   proposal per CIK for which a (current, prior) pair was found.
//!
//! # Idempotency
//!
//! Per the Suggestor contract, idempotency is checked against context, not
//! internal state. We refuse to re-execute if a proposal carrying our
//! provenance string already exists in `ContextKey::Proposals`.

use async_trait::async_trait;
use converge_pack::{AgentEffect, Context, ContextKey, ProposedFact, Suggestor};
use fathom_core::{Cik, RiskFactorSection, analytic::RiskFactorDrift};
use std::collections::HashMap;

const PROVENANCE: &str = "fathom:risk_factor_drift:v1";

pub struct RiskFactorDriftSuggestor {
    name: String,
    deps: Vec<ContextKey>,
}

impl Default for RiskFactorDriftSuggestor {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskFactorDriftSuggestor {
    pub fn new() -> Self {
        Self {
            name: "risk_factor_drift".to_string(),
            deps: vec![ContextKey::Signals],
        }
    }

    fn already_proposed(&self, ctx: &dyn Context) -> bool {
        ctx.get_proposals(ContextKey::Proposals)
            .iter()
            .any(|p| p.provenance == PROVENANCE)
    }

    fn parse_signals(&self, ctx: &dyn Context) -> Vec<RiskFactorSection> {
        ctx.get(ContextKey::Signals)
            .iter()
            .filter_map(|f| serde_json::from_str::<RiskFactorSection>(&f.content).ok())
            .collect()
    }
}

#[async_trait]
impl Suggestor for RiskFactorDriftSuggestor {
    fn name(&self) -> &str {
        &self.name
    }

    fn dependencies(&self) -> &[ContextKey] {
        &self.deps
    }

    fn accepts(&self, ctx: &dyn Context) -> bool {
        if self.already_proposed(ctx) {
            return false;
        }
        // Need at least two RiskFactorSection signals to compute a delta.
        self.parse_signals(ctx).len() >= 2
    }

    async fn execute(&self, ctx: &dyn Context) -> AgentEffect {
        let drifts = compute_drifts(&self.parse_signals(ctx));
        if drifts.is_empty() {
            return AgentEffect::empty();
        }

        let proposals = drifts
            .into_iter()
            .map(|drift| {
                let id = format!(
                    "risk_factor_drift::{}::{}",
                    drift.current.cik.as_str(),
                    drift.current.fiscal_year
                );
                let content = serde_json::to_string(&drift).unwrap_or_default();
                ProposedFact::new(ContextKey::Proposals, id, content, PROVENANCE)
            })
            .collect();
        AgentEffect::with_proposals(proposals)
    }

    fn complexity_hint(&self) -> Option<&'static str> {
        Some("O(n log n) — n = filings in scope; sort dominates")
    }
}

/// Pure drift computation — separated from the Suggestor for direct testing.
///
/// Groups sections by CIK, sorts each group by fiscal year, and pairs each
/// year with its immediate predecessor. Returns one drift per consecutive
/// pair found.
pub fn compute_drifts(sections: &[RiskFactorSection]) -> Vec<RiskFactorDrift> {
    let mut by_cik: HashMap<&Cik, Vec<&RiskFactorSection>> = HashMap::new();
    for s in sections {
        by_cik.entry(&s.filing.cik).or_default().push(s);
    }

    let mut out = Vec::new();
    for group in by_cik.values_mut() {
        group.sort_by_key(|s| s.filing.fiscal_year);
        for window in group.windows(2) {
            let (prior, current) = (window[0], window[1]);
            if current.filing.fiscal_year != prior.filing.fiscal_year + 1 {
                continue;
            }
            let current_count = current.count();
            let prior_count = prior.count();
            out.push(RiskFactorDrift {
                current: current.filing.clone(),
                prior: prior.filing.clone(),
                current_count,
                prior_count,
                delta: current_count as i32 - prior_count as i32,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fathom_core::{Cik, FilingId, FormType};

    fn section(cik: &str, fy: u16, n: usize) -> RiskFactorSection {
        RiskFactorSection {
            filing: FilingId {
                cik: Cik::new(cik),
                form: FormType::TenK,
                fiscal_year: fy,
            },
            risk_factors: (0..n).map(|i| format!("risk-{i}")).collect(),
        }
    }

    #[test]
    fn drift_detected_for_consecutive_years() {
        let sections = vec![
            section("0000320193", 2023, 23), // Apple FY23 (illustrative count)
            section("0000320193", 2024, 27), // Apple FY24
        ];
        let drifts = compute_drifts(&sections);
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].delta, 4);
        assert_eq!(drifts[0].current_count, 27);
        assert_eq!(drifts[0].prior_count, 23);
        assert_eq!(drifts[0].current.fiscal_year, 2024);
    }

    #[test]
    fn no_drift_for_non_consecutive_years() {
        // FY2020 and FY2024 — not consecutive, so no pair emitted.
        let sections = vec![
            section("0000320193", 2020, 20),
            section("0000320193", 2024, 27),
        ];
        assert!(compute_drifts(&sections).is_empty());
    }

    #[test]
    fn drifts_are_per_cik() {
        let sections = vec![
            section("0000320193", 2023, 23),
            section("0000320193", 2024, 27),
            section("0000789019", 2023, 30), // Microsoft FY23 (illustrative)
            section("0000789019", 2024, 32),
        ];
        let drifts = compute_drifts(&sections);
        assert_eq!(drifts.len(), 2);
    }

    #[test]
    fn negative_drift_when_count_falls() {
        let sections = vec![
            section("0000320193", 2023, 27),
            section("0000320193", 2024, 23),
        ];
        let drifts = compute_drifts(&sections);
        assert_eq!(drifts[0].delta, -4);
    }

    #[test]
    fn suggestor_construction() {
        let s = RiskFactorDriftSuggestor::new();
        assert_eq!(s.name(), "risk_factor_drift");
        assert_eq!(s.dependencies(), &[ContextKey::Signals]);
    }
}

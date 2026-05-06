//! `RiskFactorLanguageSuggestor` — proposes year-over-year *language* drift
//! across consecutive 10-K Item 1A risk-factor sections.
//!
//! Where [`crate::RiskFactorDriftSuggestor`] reports a single signed delta on
//! the count of risk factors, this suggestor reports how much of the
//! *language* survived year over year:
//!
//! - `jaccard_similarity` — size of the intersection of the two heading sets
//!   divided by the size of their union, in `[0.0, 1.0]`.
//! - `identical_count` — number of headings byte-identical between years.
//! - `added` / `removed` — the actionable lists of which headings appeared
//!   and disappeared.
//!
//! These two signals are deliberately separate because the formation is
//! meant to flag *disagreement* between them — flat count + low Jaccard is
//! a louder signal than either alone.

use async_trait::async_trait;
use converge_pack::{AgentEffect, Context, ContextKey, ProposedFact, Suggestor};
use fathom_sparc_core::{Cik, RiskFactorSection, analytic::RiskFactorLanguageDrift};
use std::collections::{HashMap, HashSet};

const PROVENANCE: &str = "fathom-sparc:risk_factor_language:v1";

pub struct RiskFactorLanguageSuggestor {
    name: String,
    deps: Vec<ContextKey>,
}

impl Default for RiskFactorLanguageSuggestor {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskFactorLanguageSuggestor {
    pub fn new() -> Self {
        Self {
            name: "risk_factor_language".to_string(),
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
            .filter_map(|f| serde_json::from_str::<RiskFactorSection>(f.content()).ok())
            .collect()
    }
}

#[async_trait]
impl Suggestor for RiskFactorLanguageSuggestor {
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
        self.parse_signals(ctx).len() >= 2
    }

    async fn execute(&self, ctx: &dyn Context) -> AgentEffect {
        let drifts = compute_language_drifts(&self.parse_signals(ctx));
        if drifts.is_empty() {
            return AgentEffect::empty();
        }

        let proposals = drifts
            .into_iter()
            .map(|drift| {
                let id = format!(
                    "risk_factor_language::{}::{}",
                    drift.current.cik.as_str(),
                    drift.current.fiscal_year
                );
                // Confidence == Jaccard. When language is highly stable
                // (Jaccard near 1.0) the analytical interpretation is
                // straightforward; when the issuer rewrote half the section
                // (Jaccard < 0.7) the suggestor honestly reports lower
                // confidence so the engine's HITL policy can pause for
                // human review.
                let confidence = drift.jaccard_similarity;
                let content = serde_json::to_string(&drift).unwrap_or_default();
                ProposedFact::new(ContextKey::Proposals, id, content, PROVENANCE)
                    .with_confidence(confidence)
            })
            .collect();
        AgentEffect::with_proposals(proposals)
    }

    fn complexity_hint(&self) -> Option<&'static str> {
        Some("O(n·k) — n = filings, k = avg risk factors per filing")
    }
}

/// Pure computation: pair consecutive-year sections per CIK and compute
/// Jaccard similarity over their heading sets.
pub fn compute_language_drifts(sections: &[RiskFactorSection]) -> Vec<RiskFactorLanguageDrift> {
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
            out.push(jaccard_drift(prior, current));
        }
    }
    out
}

fn jaccard_drift(prior: &RiskFactorSection, current: &RiskFactorSection) -> RiskFactorLanguageDrift {
    let prior_set: HashSet<&str> = prior.risk_factors.iter().map(String::as_str).collect();
    let current_set: HashSet<&str> = current.risk_factors.iter().map(String::as_str).collect();

    let intersection: usize = prior_set.intersection(&current_set).count();
    let union: usize = prior_set.union(&current_set).count();
    let jaccard = if union == 0 {
        1.0
    } else {
        intersection as f64 / union as f64
    };

    let mut added: Vec<String> = current_set
        .difference(&prior_set)
        .map(|s| (*s).to_string())
        .collect();
    let mut removed: Vec<String> = prior_set
        .difference(&current_set)
        .map(|s| (*s).to_string())
        .collect();
    added.sort();
    removed.sort();

    RiskFactorLanguageDrift {
        current: current.filing.clone(),
        prior: prior.filing.clone(),
        identical_count: intersection,
        jaccard_similarity: jaccard,
        added,
        removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fathom_sparc_core::{Cik, FilingId, FormType};

    fn section(cik: &str, fy: u16, headings: &[&str]) -> RiskFactorSection {
        RiskFactorSection {
            filing: FilingId {
                cik: Cik::new(cik),
                form: FormType::TenK,
                fiscal_year: fy,
            },
            risk_factors: headings.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn identical_sections_jaccard_one() {
        let h = ["A", "B", "C"];
        let drifts = compute_language_drifts(&[
            section("0000000001", 2023, &h),
            section("0000000001", 2024, &h),
        ]);
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].jaccard_similarity, 1.0);
        assert_eq!(drifts[0].identical_count, 3);
        assert!(drifts[0].added.is_empty());
        assert!(drifts[0].removed.is_empty());
    }

    #[test]
    fn disjoint_sections_jaccard_zero() {
        let drifts = compute_language_drifts(&[
            section("0000000001", 2023, &["A", "B"]),
            section("0000000001", 2024, &["X", "Y"]),
        ]);
        assert_eq!(drifts[0].jaccard_similarity, 0.0);
        assert_eq!(drifts[0].identical_count, 0);
        assert_eq!(drifts[0].added, vec!["X".to_string(), "Y".to_string()]);
        assert_eq!(drifts[0].removed, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn partial_overlap_jaccard() {
        // 2 shared out of 4 union → 0.5
        let drifts = compute_language_drifts(&[
            section("0000000001", 2023, &["A", "B", "C"]),
            section("0000000001", 2024, &["B", "C", "D"]),
        ]);
        assert!((drifts[0].jaccard_similarity - 0.5).abs() < 1e-9);
        assert_eq!(drifts[0].identical_count, 2);
        assert_eq!(drifts[0].added, vec!["D".to_string()]);
        assert_eq!(drifts[0].removed, vec!["A".to_string()]);
    }

    #[test]
    fn skips_non_consecutive_years() {
        let drifts = compute_language_drifts(&[
            section("0000000001", 2020, &["A"]),
            section("0000000001", 2024, &["A"]),
        ]);
        assert!(drifts.is_empty());
    }

    #[test]
    fn drifts_per_cik() {
        let drifts = compute_language_drifts(&[
            section("0000000001", 2023, &["A"]),
            section("0000000001", 2024, &["B"]),
            section("0000000002", 2023, &["X"]),
            section("0000000002", 2024, &["X"]),
        ]);
        assert_eq!(drifts.len(), 2);
    }

    #[test]
    fn suggestor_construction() {
        let s = RiskFactorLanguageSuggestor::new();
        assert_eq!(s.name(), "risk_factor_language");
        assert_eq!(s.dependencies(), &[ContextKey::Signals]);
    }
}

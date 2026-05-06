//! Custom Converge invariants for Fathom — SPARC.
//!
//! Invariants are the engine's "law" — checked at well-defined points in the
//! convergence loop, with violations halting promotion or rejecting results.
//! The invariant here enforces a mathematical identity that *must* hold
//! between the count-drift and language-drift facts when both are emitted
//! for the same `(CIK, fiscal_year)` pair. If it doesn't, something
//! upstream (parser, fixture, suggestor) is wrong.

use std::collections::HashMap;

use converge_core::invariant::Violation;
use converge_kernel::{Invariant, InvariantClass, InvariantResult};
use converge_pack::{Context, ContextKey};
use fathom_sparc_core::analytic::{RiskFactorDrift, RiskFactorLanguageDrift};

/// Mass conservation: for any `(CIK, fiscal_year)` where both a count drift
/// and a language drift were promoted, the identity
/// `added.len() - removed.len() == count.delta` must hold.
///
/// **Why this is a real invariant, not just a heuristic.** The count delta
/// *is* the number of headings added minus the number removed — by
/// definition. If the language suggestor's `added`/`removed` lists don't
/// agree with the count suggestor's `delta`, the two suggestors are reading
/// different inputs, the parser broke, or one of them has a bug. None of
/// those is an analytical disagreement worth surfacing — it's a structural
/// inconsistency that should fail the run.
///
/// Class is `Acceptance` — checked once at convergence claim. Violations
/// reject the run rather than gating it for human review.
pub struct RiskFactorMassConservationInvariant;

impl Invariant for RiskFactorMassConservationInvariant {
    fn name(&self) -> &str {
        "risk_factor_mass_conservation"
    }

    fn class(&self) -> InvariantClass {
        InvariantClass::Acceptance
    }

    fn check(&self, ctx: &dyn Context) -> InvariantResult {
        let mut counts = Vec::new();
        let mut langs = Vec::new();
        for fact in ctx.get(ContextKey::Proposals) {
            if let Ok(d) = serde_json::from_str::<RiskFactorDrift>(fact.content()) {
                counts.push(d);
                continue;
            }
            if let Ok(d) = serde_json::from_str::<RiskFactorLanguageDrift>(fact.content()) {
                langs.push(d);
            }
        }
        match check_mass_conservation(&counts, &langs) {
            None => InvariantResult::Ok,
            Some(reason) => InvariantResult::Violated(Violation::new(reason)),
        }
    }
}

/// Pure-data implementation of the invariant — testable without constructing
/// `ContextFact` instances (which 3.8.1 reserves for the engine).
///
/// Returns `None` when the identity holds for every `(CIK, fiscal_year)` pair
/// where both kinds of drift are present, otherwise returns a human-readable
/// description of the first violation found.
pub fn check_mass_conservation(
    counts: &[RiskFactorDrift],
    langs: &[RiskFactorLanguageDrift],
) -> Option<String> {
    let by_key: HashMap<(String, u16), &RiskFactorLanguageDrift> = langs
        .iter()
        .map(|d| {
            (
                (d.current.cik.as_str().to_string(), d.current.fiscal_year),
                d,
            )
        })
        .collect();

    for count in counts {
        let key = (
            count.current.cik.as_str().to_string(),
            count.current.fiscal_year,
        );
        let Some(lang) = by_key.get(&key) else {
            continue;
        };
        let lang_delta = lang.added.len() as i32 - lang.removed.len() as i32;
        if lang_delta != count.delta {
            return Some(format!(
                "mass conservation violated for CIK {} FY{}: \
                 count delta = {}, language (added - removed) = ({} - {}) = {}",
                count.current.cik.as_str(),
                count.current.fiscal_year,
                count.delta,
                lang.added.len(),
                lang.removed.len(),
                lang_delta,
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use fathom_sparc_core::{Cik, FilingId, FormType};

    fn filing(cik: &str, fy: u16) -> FilingId {
        FilingId {
            cik: Cik::new(cik),
            form: FormType::TenK,
            fiscal_year: fy,
        }
    }

    fn count_drift(cik: &str, fy: u16, prior: usize, current: usize) -> RiskFactorDrift {
        RiskFactorDrift {
            current: filing(cik, fy),
            prior: filing(cik, fy - 1),
            current_count: current,
            prior_count: prior,
            delta: current as i32 - prior as i32,
        }
    }

    fn lang_drift(cik: &str, fy: u16, added: usize, removed: usize) -> RiskFactorLanguageDrift {
        RiskFactorLanguageDrift {
            current: filing(cik, fy),
            prior: filing(cik, fy - 1),
            identical_count: 20,
            jaccard_similarity: 0.8,
            added: (0..added).map(|i| format!("a-{i}")).collect(),
            removed: (0..removed).map(|i| format!("r-{i}")).collect(),
        }
    }

    #[test]
    fn passes_when_identity_holds() {
        // delta = -1 → added = 6, removed = 7, diff = -1 ✓
        let counts = vec![count_drift("0000320193", 2025, 28, 27)];
        let langs = vec![lang_drift("0000320193", 2025, 6, 7)];
        assert!(check_mass_conservation(&counts, &langs).is_none());
    }

    #[test]
    fn passes_when_only_count_present() {
        let counts = vec![count_drift("0000320193", 2025, 28, 27)];
        assert!(check_mass_conservation(&counts, &[]).is_none());
    }

    #[test]
    fn passes_when_only_language_present() {
        let langs = vec![lang_drift("0000320193", 2025, 3, 3)];
        assert!(check_mass_conservation(&[], &langs).is_none());
    }

    #[test]
    fn violates_when_identity_breaks() {
        // delta = -1 but language reports added=6, removed=8 → diff = -2 ≠ -1
        let counts = vec![count_drift("0000320193", 2025, 28, 27)];
        let langs = vec![lang_drift("0000320193", 2025, 6, 8)];
        let reason = check_mass_conservation(&counts, &langs).expect("violation");
        assert!(reason.contains("mass conservation"));
        assert!(reason.contains("0000320193"));
    }

    #[test]
    fn violates_when_count_grows_but_language_does_not() {
        let counts = vec![count_drift("0000789019", 2025, 30, 35)]; // delta = +5
        let langs = vec![lang_drift("0000789019", 2025, 2, 0)]; // diff = +2
        assert!(check_mass_conservation(&counts, &langs).is_some());
    }

    #[test]
    fn checks_per_cik_independently() {
        // Apple is consistent (-1, -1); Microsoft is inconsistent (+5 vs +2).
        let counts = vec![
            count_drift("0000320193", 2025, 28, 27),
            count_drift("0000789019", 2025, 30, 35),
        ];
        let langs = vec![
            lang_drift("0000320193", 2025, 6, 7),
            lang_drift("0000789019", 2025, 2, 0),
        ];
        let reason = check_mass_conservation(&counts, &langs).expect("violation");
        assert!(reason.contains("0000789019"));
    }

    #[test]
    fn invariant_metadata() {
        let inv = RiskFactorMassConservationInvariant;
        assert_eq!(inv.name(), "risk_factor_mass_conservation");
        assert_eq!(inv.class(), InvariantClass::Acceptance);
    }
}

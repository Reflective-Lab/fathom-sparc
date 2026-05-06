//! Domain types for Fathom.
//!
//! These types are the lingua franca passed through `ContextFact.content` as
//! JSON. Suggestors deserialize them, compute, and emit `ProposedFact`s whose
//! content is also JSON-encoded — the `analytic::*` types in this module.

use serde::{Deserialize, Serialize};

/// Central Index Key — SEC's primary identifier for a filer.
///
/// Stored as a zero-padded ten-character string (e.g. `"0000320193"` for Apple).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cik(pub String);

impl Cik {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// SEC form type. We only model the ones Fathom actively analyses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FormType {
    /// Annual report.
    #[serde(rename = "10-K")]
    TenK,
    /// Quarterly report.
    #[serde(rename = "10-Q")]
    TenQ,
    /// Material event report.
    #[serde(rename = "8-K")]
    EightK,
}

/// Identity of a single filing — a (filer, form, fiscal year) triple.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FilingId {
    pub cik: Cik,
    pub form: FormType,
    pub fiscal_year: u16,
}

/// The Item 1A "Risk Factors" section of a 10-K, parsed into individual items.
///
/// Each entry is one risk factor as disclosed by the issuer — typically a
/// short heading followed by a paragraph of detail. We keep the full text so
/// downstream suggestors can do language analysis (drift, tone, hedging).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFactorSection {
    pub filing: FilingId,
    pub risk_factors: Vec<String>,
}

impl RiskFactorSection {
    pub fn count(&self) -> usize {
        self.risk_factors.len()
    }
}

/// Analytic outputs — proposed facts emitted by Fathom suggestors.
pub mod analytic {
    use super::FilingId;
    use serde::{Deserialize, Serialize};

    /// Year-over-year drift in the count of disclosed risk factors.
    ///
    /// A positive `delta` means the issuer disclosed more risks this year than
    /// last; negative means fewer. Magnitude alone isn't a verdict — the
    /// formation interprets it alongside language-tone and segment signals.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RiskFactorDrift {
        pub current: FilingId,
        pub prior: FilingId,
        pub current_count: usize,
        pub prior_count: usize,
        pub delta: i32,
    }

    impl RiskFactorDrift {
        pub fn metric_name() -> &'static str {
            "risk_factor_count_delta_yoy"
        }
    }

    /// Year-over-year drift in the *language* of disclosed risk factors.
    ///
    /// `jaccard_similarity` is the size of the intersection of the two
    /// heading sets divided by the size of their union, in `[0.0, 1.0]`. A
    /// value of `1.0` means every heading is byte-identical between years;
    /// `0.0` means a complete rewrite. The `added` and `removed` lists are
    /// the actionable detail — they tell a reader *what* changed, not just
    /// *that* it changed.
    ///
    /// This signal is complementary to [`RiskFactorDrift`]: the count can be
    /// flat while language churn is high (or vice versa), and the formation
    /// is meant to flag the disagreement, not paper over it.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct RiskFactorLanguageDrift {
        pub current: FilingId,
        pub prior: FilingId,
        pub identical_count: usize,
        pub jaccard_similarity: f64,
        pub added: Vec<String>,
        pub removed: Vec<String>,
    }

    impl RiskFactorLanguageDrift {
        pub fn metric_name() -> &'static str {
            "risk_factor_language_drift_yoy"
        }
    }
}

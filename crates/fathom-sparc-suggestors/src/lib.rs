//! Converge suggestors for Fathom — SPARC: propose facts from the EDGAR
//! lakehouse.

pub mod risk_factor_drift;
pub mod risk_factor_language;

pub use risk_factor_drift::RiskFactorDriftSuggestor;
pub use risk_factor_language::RiskFactorLanguageSuggestor;

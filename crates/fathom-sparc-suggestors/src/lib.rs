//! Converge suggestors for Fathom — propose facts from the EDGAR lakehouse.

pub mod risk_factor_drift;

pub use risk_factor_drift::RiskFactorDriftSuggestor;

//! Ingestion pipeline for Fathom — SPARC.
//!
//! Three on-ramps to a [`RiskFactorSection`], all produce the same shape:
//!
//! 1. **Fixture loader** (always available) — reads pre-extracted JSON files
//!    from `fixtures/`. Handy for tests and reproducible demos.
//! 2. **SEC EDGAR live ingest** (`feature = "sec"`) — fetches a 10-K HTML
//!    document via `manifold::HttpFetchProvider`, locates Item 1A, and
//!    extracts risk-factor headings via `manifold::ScraperHtmlBackend`.
//! 3. **HuggingFace dataset ingest** (`feature = "hf"`, future) — reads a
//!    parquet slice from a HF dataset via
//!    `manifold::object_storage::HuggingFaceObjectStore` + the `parquet`
//!    crate.
//!
//! Keeping the same output shape across all three means downstream
//! suggestors don't need to change when the source flips.

#[cfg(feature = "sec")]
pub mod sec;

use std::fs;
use std::path::Path;

use fathom_sparc_core::RiskFactorSection;

#[cfg(feature = "sec")]
pub use sec::{SecIngestError, fetch_and_extract as fetch_and_extract_sec};

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("failed to read fixture {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse fixture {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Loads a single [`RiskFactorSection`] from a JSON fixture file.
///
/// Fixture files live in `fixtures/` at the workspace root. Each is a single
/// JSON object matching the [`RiskFactorSection`] schema (a `filing` object
/// plus a `risk_factors` string array).
pub fn load_risk_factor_fixture(path: impl AsRef<Path>) -> Result<RiskFactorSection, IngestError> {
    let path = path.as_ref();
    let raw = fs::read_to_string(path).map_err(|source| IngestError::Io {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(|source| IngestError::Parse {
        path: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fathom_sparc_core::FormType;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        // CARGO_MANIFEST_DIR is crates/fathom-ingest; fixtures live at the
        // workspace root.
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn loads_real_apple_fy2025_fixture() {
        let section = load_risk_factor_fixture(fixture_path("apple-fy2025-risk-factors.json"))
            .expect("fixture should load");
        assert_eq!(section.filing.cik.as_str(), "0000320193");
        assert_eq!(section.filing.form, FormType::TenK);
        assert_eq!(section.filing.fiscal_year, 2025);
        assert_eq!(section.count(), 27);
    }

    #[test]
    fn loads_real_apple_fy2024_fixture() {
        let section = load_risk_factor_fixture(fixture_path("apple-fy2024-risk-factors.json"))
            .expect("fixture should load");
        assert_eq!(section.filing.fiscal_year, 2024);
        assert_eq!(section.count(), 28);
    }
}

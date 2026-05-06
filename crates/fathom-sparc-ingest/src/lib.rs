//! Ingestion pipeline for Fathom.
//!
//! At 1.0 this crate provides a minimal **fixture loader**: it deserialises
//! pre-extracted [`RiskFactorSection`] JSON files into the domain type. The
//! production path — HuggingFace download → DataFusion parse → Iceberg
//! materialisation — lands in 1.x as later slices.
//!
//! Keeping the same input shape (`RiskFactorSection`) across fixture and
//! materialised paths means downstream suggestors don't need to change when
//! the source flips.

use std::fs;
use std::path::Path;

use fathom_sparc_core::RiskFactorSection;

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

//! Live HuggingFace dataset ingest, via `manifold::HuggingFaceObjectStore`.
//!
//! Currently supports JSONL-shaped datasets — specifically
//! [`JanosAudran/financial-reports-sec`](https://huggingface.co/datasets/JanosAudran/financial-reports-sec),
//! which exposes one record per company with each company's filings under
//! a `report.section_1A` array of sentences.
//!
//! ## Granularity caveat
//!
//! `JanosAudran/financial-reports-sec` is sentence-level: the
//! [`RiskFactorSection`]s this module produces will have *hundreds* of
//! `risk_factors` entries per filing (one per sentence), not the ~25
//! heading-level entries that the SEC EDGAR ingest produces. Downstream
//! drift / language analysis still works but is computing
//! sentence-churn, not heading-churn — a different signal.
//!
//! Mark fixtures from this path with the `:hf-sentences` provenance hint
//! so consumers can disambiguate. (We use a separate filename suffix to
//! avoid mixing the two granularities in the same fixture set.)

use std::path::Path as StdPath;

use fathom_sparc_core::{Cik, FilingId, FormType, RiskFactorSection};
use manifold::object_storage::HuggingFaceObjectStore;
use manifold::{ObjectPath, ObjectStore};
use object_store::GetOptions;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum HfIngestError {
    #[error("HF object store error: {0}")]
    Store(String),
    #[error("download failed for {path}: {source}")]
    Download {
        path: String,
        #[source]
        source: object_store::Error,
    },
    #[error("read body failed: {0}")]
    ReadBody(String),
    #[error("invalid UTF-8 in shard: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("invalid JSON line {line_no}: {source}")]
    Json {
        line_no: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("io error writing fixture: {0}")]
    Io(#[from] std::io::Error),
}

/// Companies are the top-level rows in JanosAudran/financial-reports-sec
/// JSONL shards. Each row carries every 10-K the company has filed, with
/// per-section sentence arrays under `report`.
#[derive(Debug, Deserialize)]
struct CompanyRecord {
    cik: String,
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    filings: Vec<FilingRecord>,
}

#[derive(Debug, Deserialize)]
struct FilingRecord {
    #[serde(rename = "filingDate")]
    filing_date: String,
    form: String,
    #[serde(default)]
    report: ReportSections,
}

#[derive(Debug, Default, Deserialize)]
struct ReportSections {
    #[serde(rename = "section_1A", default)]
    section_1a: Vec<String>,
}

/// Downloads a single JSONL shard from a HuggingFace dataset and
/// materialises every 10-K filing it contains as a [`RiskFactorSection`].
///
/// `dataset_path` is the path *within the dataset repo* (e.g.
/// `data/small/test/shard_0.jsonl`).
pub async fn fetch_shard(
    repo_id: &str,
    dataset_path: &str,
) -> Result<Vec<RiskFactorSection>, HfIngestError> {
    let store = HuggingFaceObjectStore::try_new()
        .map_err(|e| HfIngestError::Store(e.to_string()))?;
    // HF object store path convention: `{user}/{name}[@rev]/{path-in-repo}`.
    let object_path = ObjectPath::parse(format!("{repo_id}/{dataset_path}"))
        .map_err(|e| HfIngestError::Store(e.to_string()))?;
    // The bare `get()` lives on the `ObjectStoreExt` extension trait; we
    // call the trait-required `get_opts` directly to avoid the extra
    // import dance.
    let result = store
        .get_opts(&object_path, GetOptions::default())
        .await
        .map_err(|source| HfIngestError::Download {
            path: format!("{repo_id}/{dataset_path}"),
            source,
        })?;
    let bytes = result
        .bytes()
        .await
        .map_err(|e| HfIngestError::ReadBody(e.to_string()))?;
    let text = std::str::from_utf8(&bytes)?;

    let mut out = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let company: CompanyRecord =
            serde_json::from_str(line).map_err(|source| HfIngestError::Json {
                line_no: line_no + 1,
                source,
            })?;
        for filing in &company.filings {
            if let Some(section) = to_risk_factor_section(&company, filing) {
                out.push(section);
            }
        }
    }
    Ok(out)
}

/// Writes each [`RiskFactorSection`] out as
/// `cik-{cik}-fy{year}-risk-factors-hf.json` so HF-derived
/// sentence-granularity fixtures don't get mixed with SEC heading-
/// granularity ones. Returns the list of written paths.
pub async fn fetch_shard_to_fixtures(
    repo_id: &str,
    dataset_path: &str,
    fixtures_dir: &StdPath,
) -> Result<Vec<std::path::PathBuf>, HfIngestError> {
    let sections = fetch_shard(repo_id, dataset_path).await?;
    std::fs::create_dir_all(fixtures_dir)?;
    let mut written = Vec::new();
    for section in &sections {
        let path = fixtures_dir.join(format!(
            "cik-{}-fy{}-risk-factors-hf.json",
            section.filing.cik.as_str(),
            section.filing.fiscal_year,
        ));
        let json = serde_json::to_string_pretty(section).map_err(|e| HfIngestError::Json {
            line_no: 0,
            source: e,
        })?;
        std::fs::write(&path, json)?;
        written.push(path);
    }
    Ok(written)
}

fn to_risk_factor_section(
    company: &CompanyRecord,
    filing: &FilingRecord,
) -> Option<RiskFactorSection> {
    let form = match filing.form.as_str() {
        "10-K" => FormType::TenK,
        "10-Q" => FormType::TenQ,
        "8-K" => FormType::EightK,
        _ => return None,
    };
    let fiscal_year: u16 = filing
        .filing_date
        .split('-')
        .next()
        .and_then(|y| y.parse().ok())?;
    if filing.report.section_1a.is_empty() {
        return None;
    }
    Some(RiskFactorSection {
        filing: FilingId {
            cik: Cik::new(&company.cik),
            form,
            fiscal_year,
        },
        risk_factors: filing.report.section_1a.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonl_company_record() {
        let line = r#"{
            "cik": "0001602658",
            "name": "Investar Holding Corp",
            "filings": [{
                "filingDate": "2021-03-10",
                "form": "10-K",
                "report": {
                    "section_1A": ["Item 1A.", "Risk one.", "Risk two."]
                }
            }]
        }"#;
        let company: CompanyRecord = serde_json::from_str(line).unwrap();
        assert_eq!(company.cik, "0001602658");
        assert_eq!(company.filings.len(), 1);
        let section =
            to_risk_factor_section(&company, &company.filings[0]).expect("section");
        assert_eq!(section.filing.cik.as_str(), "0001602658");
        assert_eq!(section.filing.fiscal_year, 2021);
        assert_eq!(section.filing.form, FormType::TenK);
        assert_eq!(section.risk_factors.len(), 3);
    }

    #[test]
    fn skips_filings_without_section_1a() {
        let company = CompanyRecord {
            cik: "0000000001".to_string(),
            name: "Test".to_string(),
            filings: vec![FilingRecord {
                filing_date: "2024-01-01".to_string(),
                form: "10-K".to_string(),
                report: ReportSections::default(),
            }],
        };
        assert!(to_risk_factor_section(&company, &company.filings[0]).is_none());
    }

    #[test]
    fn skips_unknown_form_types() {
        let line = r#"{
            "cik": "0001",
            "name": "X",
            "filings": [{
                "filingDate": "2024-01-01",
                "form": "DEF 14A",
                "report": { "section_1A": ["foo."] }
            }]
        }"#;
        let company: CompanyRecord = serde_json::from_str(line).unwrap();
        assert!(to_risk_factor_section(&company, &company.filings[0]).is_none());
    }
}

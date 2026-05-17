//! SEC EDGAR → `RiskFactorSection` synthesis.
//!
//! The SEC contract (User-Agent, rate-limit politeness, the three observed
//! 10-K markup-selector patterns, the Item-N section locator, the
//! multi-selector heading extractor) lives in
//! [`embassy_sec_edgar::live`]. This module is the thin synthesis layer
//! that turns embassy's contract outputs into Fathom's own
//! [`RiskFactorSection`] shape.
//!
//! What stays here:
//! - The SPARC-side `fetch_and_extract` wrapper that callers (CLI, future
//!   suggestors) hold a stable handle to.
//! - The error type that flattens embassy's `LiveError` plus the
//!   SPARC-specific "section not found" branch into one app-side enum.
//! - The `FilingId`-shaped output (cik + form + fiscal_year) embassy
//!   doesn't know about — fiscal year is a SPARC analytic concern.
//!
//! What moved out:
//! - User-Agent / max-bytes / timeout / politeness sleep
//! - Three observed selector patterns + plausibility bounds
//! - Item-1A boundary detection (now generalized as
//!   `locate_item_section(html, "1A", "1B")` in embassy)
//! - The HTTP fetch + HTML extract plumbing

use embassy_sec_edgar::live::{
    self, HeadingExtractOptions, LiveError, LiveFetchOptions,
};
use fathom_narrative_core::{Cik, FilingId, FormType, RiskFactorSection};

#[derive(Debug, thiserror::Error)]
pub enum SecIngestError {
    #[error("Item 1A section not found in document")]
    SectionNotFound,
    #[error(transparent)]
    Live(#[from] LiveError),
}

/// Fetch a 10-K HTML document from SEC EDGAR and extract its Item 1A
/// risk-factor headings.
///
/// `filing_url` should be the direct URL to the primary HTML document, e.g.
/// `https://www.sec.gov/Archives/edgar/data/320193/000032019325000079/aapl-20250927.htm`.
///
/// `cik` and `fiscal_year` are echoed into the resulting
/// [`RiskFactorSection`] metadata — SEC EDGAR doesn't surface the issuer's
/// fiscal year in the document URL, so the caller supplies it.
pub async fn fetch_and_extract(
    filing_url: &str,
    cik: &Cik,
    fiscal_year: u16,
) -> Result<RiskFactorSection, SecIngestError> {
    let html = live::fetch_filing_html(filing_url, &LiveFetchOptions::default()).await?;
    let section_html =
        live::locate_item_section(&html, "1A", "1B").ok_or(SecIngestError::SectionNotFound)?;
    let headings = live::extract_section_headings(section_html, &HeadingExtractOptions::default())?;
    Ok(RiskFactorSection {
        filing: FilingId {
            cik: cik.clone(),
            form: FormType::TenK,
            fiscal_year,
        },
        risk_factors: headings,
    })
}

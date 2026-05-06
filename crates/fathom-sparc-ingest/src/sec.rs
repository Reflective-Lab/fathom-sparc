//! Live SEC EDGAR ingest — `Filing URL → RiskFactorSection`.
//!
//! Wires up:
//! - `manifold::HttpFetchProvider` for the HTTP fetch (with SEC's required
//!   User-Agent header — the SEC blocks anonymous requests).
//! - `manifold::ScraperHtmlBackend` for HTML parsing.
//!
//! Layered on top: Fathom-specific Item 1A bounds detection (regex on the
//! raw text) plus the three markup-pattern selectors observed across
//! Apple, Microsoft, and NVIDIA 10-K filings. The "best pattern" heuristic
//! picks whichever selector yields the most plausible heading count
//! (15–40 inclusive). This mirrors the python extractor that was used to
//! seed the existing fixtures, ported to Rust.

use std::time::Duration;

use fathom_sparc_core::{Cik, FilingId, FormType, RiskFactorSection};
use manifold::{
    ExtractedNode, HtmlExtractBackend, HttpFetchProvider, ScraperHtmlBackend, WebFetchBackend,
    WebFetchRequest,
};

const DEFAULT_USER_AGENT: &str = "Reflective Labs Research kpernyer@gmail.com";
// 10-Ks are routinely 1.5–8 MB of HTML; manifold's WebFetchByteLimit caps at 8 MiB.
const MAX_BYTES: usize = 8 * 1024 * 1024;
const TIMEOUT_MS: u64 = 60_000;

const SELECTORS: &[&str] = &[
    // Apple — italic + bold (font-weight:700) headings.
    r#"span[style*="font-style:italic"][style*="font-weight:700"]"#,
    // MSFT — plain bold (font-weight:bold).
    r#"span[style*="font-weight:bold"]"#,
    // NVDA — weight-700 without italic.
    r#"span[style*="font-weight:700"]"#,
];

const MIN_HEADINGS: usize = 15;
const MAX_HEADINGS: usize = 40;
const MIN_HEADING_LEN: usize = 30;
const MAX_HEADING_LEN: usize = 300;

#[derive(Debug, thiserror::Error)]
pub enum SecIngestError {
    #[error("fetch failed: {0}")]
    Fetch(String),
    #[error("extract failed: {0}")]
    Extract(String),
    #[error("Item 1A section not found in document")]
    SectionNotFound,
    #[error("no extractor pattern produced a plausible heading count (15–40); best yielded {best} headings")]
    NoPlausiblePattern { best: usize },
    #[error("HTTP {status} from {url}")]
    Http { status: u16, url: String },
    #[error("blocking task join failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Fetch a 10-K HTML document from SEC EDGAR and extract its Item 1A
/// risk-factor headings.
///
/// `filing_url` should be the direct URL to the primary HTML document, e.g.
/// `https://www.sec.gov/Archives/edgar/data/320193/000032019325000079/aapl-20250927.htm`.
///
/// `cik` and `fiscal_year` are echoed into the resulting [`RiskFactorSection`]
/// metadata — SEC EDGAR doesn't surface the issuer's fiscal year in the
/// document URL, so the caller supplies it.
pub async fn fetch_and_extract(
    filing_url: &str,
    cik: &Cik,
    fiscal_year: u16,
) -> Result<RiskFactorSection, SecIngestError> {
    let html = fetch(filing_url).await?;
    let section_html = locate_item_1a(&html).ok_or(SecIngestError::SectionNotFound)?;
    let headings = best_pattern(section_html)?;
    Ok(RiskFactorSection {
        filing: FilingId {
            cik: cik.clone(),
            form: FormType::TenK,
            fiscal_year,
        },
        risk_factors: headings,
    })
}

async fn fetch(url: &str) -> Result<String, SecIngestError> {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        let provider = HttpFetchProvider::new().with_user_agent(DEFAULT_USER_AGENT);
        let request = WebFetchRequest::new(&url)
            .map_err(|e| SecIngestError::Fetch(e.to_string()))?
            .with_max_bytes(MAX_BYTES)
            .map_err(|e| SecIngestError::Fetch(e.to_string()))?
            .with_timeout_ms(TIMEOUT_MS)
            .map_err(|e| SecIngestError::Fetch(e.to_string()))?;
        // Tag SEC submissions with our research contact so EDGAR's rate-limit
        // logs can identify us — required by their fair-use policy.
        let response = provider
            .fetch(&request)
            .map_err(|e| SecIngestError::Fetch(e.to_string()))?;
        if response.status >= 400 {
            return Err(SecIngestError::Http {
                status: response.status,
                url: response.url,
            });
        }
        // Add a tiny politeness delay — SEC EDGAR caps at 10 req/s/IP.
        std::thread::sleep(Duration::from_millis(120));
        Ok(response.body)
    })
    .await?
}

/// Returns the slice of `html` covering the Item 1A → Item 1B section, or
/// `None` if those bounds aren't present (TOC-only filings, amendments,
/// etc.). Three occurrences of "Item 1A" are typical: TOC entry,
/// forward-looking-statement reference, and the actual section heading;
/// we take the third as the start, and the last "Item 1B" as the end.
fn locate_item_1a(html: &str) -> Option<&str> {
    let positions_1a = find_all(html, "Item 1A").chain(find_all(html, "Item\u{00a0}1A"));
    let positions_1a: Vec<usize> = positions_1a.collect();
    let positions_1b: Vec<usize> = find_all(html, "Item 1B")
        .chain(find_all(html, "Item\u{00a0}1B"))
        .collect();
    if positions_1a.len() < 3 || positions_1b.is_empty() {
        return None;
    }
    let mut start_candidates = positions_1a.clone();
    start_candidates.sort_unstable();
    let start = start_candidates[2];
    let end = positions_1b
        .iter()
        .copied()
        .filter(|&p| p > start)
        .max()
        .unwrap_or(html.len());
    Some(&html[start..end])
}

fn find_all<'a>(haystack: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    let mut start = 0;
    std::iter::from_fn(move || {
        let pos = haystack[start..].find(needle)?;
        let abs = start + pos;
        start = abs + needle.len();
        Some(abs)
    })
}

fn best_pattern(section_html: &str) -> Result<Vec<String>, SecIngestError> {
    let backend = ScraperHtmlBackend::new();
    let mut best: Vec<String> = Vec::new();
    let mut max_seen = 0usize;
    for selector in SELECTORS {
        let nodes = backend
            .extract(section_html, &[selector])
            .map_err(|e| SecIngestError::Extract(e.to_string()))?;
        let candidates: Vec<String> = nodes
            .into_iter()
            .map(|n: ExtractedNode| n.text)
            .filter(|text| {
                let len = text.len();
                len >= MIN_HEADING_LEN
                    && len <= MAX_HEADING_LEN
                    && text.ends_with('.')
            })
            .collect();
        let count = candidates.len();
        if (MIN_HEADINGS..=MAX_HEADINGS).contains(&count) {
            // First selector to yield a plausible count wins; later
            // selectors might yield broader counts that include body
            // paragraphs, so we prefer narrower successful matches.
            return Ok(candidates);
        }
        if count > max_seen {
            max_seen = count;
            best = candidates;
        }
    }
    if best.is_empty() || !(MIN_HEADINGS..=MAX_HEADINGS).contains(&max_seen) {
        return Err(SecIngestError::NoPlausiblePattern { best: max_seen });
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_item_1a_with_three_markers() {
        let html = "TOC: Item 1A. Risk Factors p.5. \
            Forward looking: see Item 1A. \
            Item 1A. Risk Factors\n\
            ...real content...\n\
            Item 1B. Unresolved comments";
        let section = locate_item_1a(html).expect("section");
        assert!(section.contains("...real content..."));
        assert!(!section.contains("Item 1B"));
    }

    #[test]
    fn returns_none_when_section_missing() {
        let html = "TOC: Item 1A in toc, but only one occurrence";
        assert!(locate_item_1a(html).is_none());
    }

    #[test]
    fn picks_pattern_yielding_plausible_count() {
        // Build a fake Item 1A section with 16 italic+bold headings.
        let mut section = String::new();
        for i in 0..16 {
            section.push_str(&format!(
                r#"<span style="font-style:italic;font-weight:700">Risk factor heading number {i:02} ends with a period.</span>"#
            ));
        }
        let headings = best_pattern(&section).expect("ok");
        assert_eq!(headings.len(), 16);
        assert!(headings[0].ends_with("period."));
    }

    #[test]
    fn rejects_when_no_pattern_plausible() {
        // 5 headings is below MIN_HEADINGS (15) — should fail.
        let section = (0..5)
            .map(|i| {
                format!(
                    r#"<span style="font-weight:bold">Risk factor heading number {i:02} ends with a period.</span>"#
                )
            })
            .collect::<String>();
        let err = best_pattern(&section).expect_err("should fail");
        assert!(matches!(err, SecIngestError::NoPlausiblePattern { best: 5 }));
    }
}

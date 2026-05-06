//! Fathom CLI — first-slice entry point.
//!
//! At 1.0 the CLI loads `RiskFactorSection` fixtures from `fixtures/`, wraps
//! them in a minimal in-memory `Context`, runs `RiskFactorDriftSuggestor`
//! directly, and prints the proposed drift facts. The full Converge engine
//! (eligibility scheduling, promotion gate, integrity proof) lands when a
//! second suggestor exists worth composing with.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use converge_pack::fact::Fact;
use converge_pack::fact::kernel_authority::new_fact;
use converge_pack::{Context, ContextKey, Suggestor};
use fathom_sparc_core::{Cik, RiskFactorSection};
use fathom_sparc_ingest::load_risk_factor_fixture;
use fathom_sparc_suggestors::{RiskFactorDriftSuggestor, RiskFactorLanguageSuggestor};

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

#[derive(Parser)]
#[command(
    name = "fathom",
    about = "Convergence-driven analysis of public-company financial filings"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Materialise a HuggingFace dataset slice into the local Iceberg lakehouse.
    /// Not yet implemented at 1.0; fixtures under `fixtures/` are the on-ramp.
    Ingest,
    /// Run the risk-factor-drift suggestor against the fixtures for `cik`.
    Analyse {
        /// SEC CIK (Central Index Key), e.g. 0000320193 for Apple.
        cik: String,
        /// Override the fixtures directory.
        #[arg(long, default_value = FIXTURES_DIR)]
        fixtures: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match Cli::parse().command {
        Command::Ingest => {
            tracing::info!("ingest pipeline not yet implemented; use fixtures under fixtures/");
            Ok(())
        }
        Command::Analyse { cik, fixtures } => analyse(Cik::new(cik), &fixtures).await,
    }
}

/// Loads every JSON fixture in `fixtures_dir`, keeps those for `cik`, runs
/// the suggestor, and prints the proposed facts as pretty JSON.
async fn analyse(cik: Cik, fixtures_dir: &std::path::Path) -> anyhow::Result<()> {
    let sections = load_sections_for_cik(&cik, fixtures_dir)?;
    if sections.len() < 2 {
        bail!(
            "found {} fixture(s) for CIK {} in {}; need at least 2 to compute drift",
            sections.len(),
            cik.as_str(),
            fixtures_dir.display()
        );
    }
    tracing::info!(
        cik = cik.as_str(),
        count = sections.len(),
        "loaded fixtures"
    );

    let ctx = build_context(&sections)?;
    let mut all_proposals: Vec<converge_pack::ProposedFact> = Vec::new();

    // Run each suggestor in registration order. Engine integration (eligibility
    // scheduling, dependency-driven re-runs, promotion gate) lands when there
    // are signals for suggestors to compose on; for now, sequential direct
    // calls demonstrate the analytical surface end-to-end.
    let drift = RiskFactorDriftSuggestor::new();
    if drift.accepts(&ctx) {
        all_proposals.extend(drift.execute(&ctx).await.proposals);
    }
    let language = RiskFactorLanguageSuggestor::new();
    if language.accepts(&ctx) {
        all_proposals.extend(language.execute(&ctx).await.proposals);
    }

    if all_proposals.is_empty() {
        println!("no consecutive-year pairs found; no proposals emitted");
        return Ok(());
    }

    println!(
        "{}",
        serde_json::to_string_pretty(
            &all_proposals.iter().map(ProposalView::from).collect::<Vec<_>>()
        )?
    );
    Ok(())
}

fn load_sections_for_cik(
    cik: &Cik,
    fixtures_dir: &std::path::Path,
) -> anyhow::Result<Vec<RiskFactorSection>> {
    let mut out = Vec::new();
    let entries = fs::read_dir(fixtures_dir)
        .with_context(|| format!("reading fixtures dir {}", fixtures_dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        match load_risk_factor_fixture(&path) {
            Ok(s) if &s.filing.cik == cik => out.push(s),
            Ok(_) => {}
            Err(err) => tracing::warn!(?path, error = %err, "skipping unreadable fixture"),
        }
    }
    out.sort_by_key(|s| s.filing.fiscal_year);
    Ok(out)
}

/// Wraps the loaded sections as `Fact`s under `ContextKey::Signals`, the
/// shape `RiskFactorDriftSuggestor` expects.
fn build_context(sections: &[RiskFactorSection]) -> anyhow::Result<MockContext> {
    let signals = sections
        .iter()
        .map(|s| {
            let id = format!(
                "filing::{}::{}",
                s.filing.cik.as_str(),
                s.filing.fiscal_year
            );
            let content = serde_json::to_string(s)?;
            Ok::<_, serde_json::Error>(new_fact(ContextKey::Signals, id, content))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MockContext { signals })
}

/// Minimal in-memory `Context` carrying signals only. The first-slice CLI
/// runs a single suggestor directly and never enters the engine, so we don't
/// need to model the full set of context keys.
pub struct MockContext {
    signals: Vec<Fact>,
}

impl Context for MockContext {
    fn has(&self, key: ContextKey) -> bool {
        matches!(key, ContextKey::Signals) && !self.signals.is_empty()
    }

    fn get(&self, key: ContextKey) -> &[Fact] {
        match key {
            ContextKey::Signals => &self.signals,
            _ => &[],
        }
    }
}

/// Display-friendly projection of `ProposedFact` — strips the internal id
/// type wrapper so the JSON output reads cleanly.
#[derive(serde::Serialize)]
struct ProposalView<'a> {
    key: &'a str,
    id: String,
    content: serde_json::Value,
    provenance: &'a str,
}

impl<'a> From<&'a converge_pack::ProposedFact> for ProposalView<'a> {
    fn from(p: &'a converge_pack::ProposedFact) -> Self {
        let key = match p.key {
            ContextKey::Proposals => "Proposals",
            ContextKey::Signals => "Signals",
            ContextKey::Hypotheses => "Hypotheses",
            ContextKey::Strategies => "Strategies",
            ContextKey::Constraints => "Constraints",
            ContextKey::Seeds => "Seeds",
            ContextKey::Competitors => "Competitors",
            ContextKey::Evaluations => "Evaluations",
            ContextKey::Diagnostic => "Diagnostic",
            ContextKey::Votes => "Votes",
            ContextKey::Disagreements => "Disagreements",
            ContextKey::ConsensusOutcomes => "ConsensusOutcomes",
        };
        let content =
            serde_json::from_str(&p.content).unwrap_or(serde_json::Value::String(p.content.clone()));
        Self {
            key,
            id: p.id.to_string(),
            content,
            provenance: &p.provenance,
        }
    }
}

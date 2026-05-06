//! Fathom — SPARC CLI.
//!
//! `fathom-sparc analyse <CIK>` discovers JSON fixtures under `fixtures/`
//! for that CIK, seeds them as inputs into a `ContextState`, registers the
//! risk-factor drift and language suggestors with a Converge `Engine`, runs
//! the convergence loop, and prints the promoted facts (with provenance) as
//! JSON.
//!
//! The engine is the load-bearing piece: it owns eligibility scheduling,
//! deterministic merge order, the promotion gate that turns `ProposedFact`
//! into authoritative `Fact`, and the integrity proof for the final
//! context. None of that is fake — the same engine drives Converge
//! consumers like Organism and Wolfgang.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, bail};
use arbiter::{
    PolicyEngine,
    decision::PolicyOutcome,
    types::{ContextIn, DecideRequest, PrincipalIn, ResourceIn},
};
use clap::{Parser, Subcommand};
use converge_core::{AuthorityLevel, FlowAction, FlowPhase};
use converge_kernel::{
    ContextState, Engine, EngineHitlPolicy, GateDecision, RunResult, TimeoutPolicy,
};
use converge_optimization::suggestors::portfolio::{PortfolioSelection, PortfolioSuggestor};
use converge_pack::{ContextFact, ContextKey};
use fathom_sparc_core::{Cik, RiskFactorSection};
use fathom_sparc_ingest::load_risk_factor_fixture;
use fathom_sparc_suggestors::{
    PortfolioCoverageSeedSuggestor, RiskFactorDriftSuggestor, RiskFactorLanguageSuggestor,
    RiskFactorMassConservationInvariant,
};

/// Confidence floor — proposals at or below this trigger a HITL pause.
/// `RiskFactorLanguageSuggestor` sets confidence = Jaccard similarity, so
/// any consecutive-year pair with substantial language churn (Jaccard ≤ 0.7)
/// requires explicit approval before promotion.
const HITL_CONFIDENCE_THRESHOLD: f64 = 0.7;

/// Cedar policy text — checked into `policies/fathom_sparc.cedar`,
/// embedded at build time so a single-file `fathom-sparc` binary doesn't
/// need its policy on disk at runtime.
const POLICY_TEXT: &str = include_str!("../../../policies/fathom_sparc.cedar");

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

#[derive(Parser)]
#[command(
    name = "fathom-sparc",
    about = "Convergence-driven analysis of public-company financial filings"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Live SEC EDGAR ingest — fetch a 10-K HTML, extract Item 1A risk
    /// factors, write a `RiskFactorSection` JSON fixture under `fixtures/`.
    /// Requires the `sec-ingest` feature.
    Ingest {
        /// Source: `sec` (live SEC EDGAR fetch). Future: `hf`.
        #[arg(long, default_value = "sec")]
        source: String,
        /// SEC CIK (e.g. 0000320193).
        #[arg(long)]
        cik: String,
        /// Direct URL to the primary 10-K HTML document on EDGAR (e.g.
        /// `https://www.sec.gov/Archives/edgar/data/320193/000032019325000079/aapl-20250927.htm`).
        #[arg(long)]
        url: String,
        /// Fiscal year of the filing (e.g. 2025 for Apple's FY2025 10-K).
        #[arg(long)]
        fiscal_year: u16,
        /// Override the fixtures directory.
        #[arg(long, default_value = FIXTURES_DIR)]
        fixtures: PathBuf,
    },
    /// Run the engine for `cik`: register both suggestors, converge, print
    /// promoted facts as JSON.
    Analyse {
        /// SEC CIK (Central Index Key), e.g. 0000320193 for Apple.
        cik: String,
        /// Override the fixtures directory.
        #[arg(long, default_value = FIXTURES_DIR)]
        fixtures: PathBuf,
    },
    /// Solve "which CIKs should we deep-review this period under our
    /// analyst-time budget?" as a 0-1 knapsack. Loads every fixture, runs
    /// drift + language + portfolio_seed + PortfolioSuggestor, prints the
    /// selected portfolio.
    Portfolio {
        /// Analyst-reading budget, in risk-factor-equivalents (each
        /// disclosed Item 1A factor counts as one unit).
        #[arg(long, default_value_t = 50)]
        budget: i64,
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
        Command::Ingest {
            source,
            cik,
            url,
            fiscal_year,
            fixtures,
        } => ingest(&source, Cik::new(cik), &url, fiscal_year, &fixtures).await,
        Command::Analyse { cik, fixtures } => analyse(Cik::new(cik), &fixtures).await,
        Command::Portfolio { budget, fixtures } => portfolio(budget, &fixtures).await,
    }
}

async fn ingest(
    source: &str,
    cik: Cik,
    url: &str,
    fiscal_year: u16,
    fixtures_dir: &std::path::Path,
) -> anyhow::Result<()> {
    match source {
        #[cfg(feature = "sec-ingest")]
        "sec" => ingest_sec(&cik, url, fiscal_year, fixtures_dir).await,
        #[cfg(not(feature = "sec-ingest"))]
        "sec" => anyhow::bail!(
            "SEC ingest is gated behind the `sec-ingest` cargo feature; rebuild with `--features=sec-ingest`"
        ),
        other => anyhow::bail!("unknown ingest source {other:?}; supported: sec"),
    }
}

#[cfg(feature = "sec-ingest")]
async fn ingest_sec(
    cik: &Cik,
    url: &str,
    fiscal_year: u16,
    fixtures_dir: &std::path::Path,
) -> anyhow::Result<()> {
    tracing::info!(cik = cik.as_str(), %url, fiscal_year, "fetching SEC EDGAR filing");
    let section = fathom_sparc_ingest::fetch_and_extract_sec(url, cik, fiscal_year)
        .await
        .map_err(|e| anyhow::anyhow!("SEC ingest failed: {e}"))?;
    let out_path = fixtures_dir.join(format!(
        "cik-{}-fy{}-risk-factors.json",
        cik.as_str(),
        fiscal_year,
    ));
    std::fs::create_dir_all(fixtures_dir)?;
    let json = serde_json::to_string_pretty(&section)?;
    std::fs::write(&out_path, json)?;
    println!(
        "extracted {} risk factors → {}",
        section.count(),
        out_path.display()
    );
    Ok(())
}

async fn analyse(cik: Cik, fixtures_dir: &std::path::Path) -> anyhow::Result<()> {
    // Front gate: Cedar policy preflight. In production the principal comes
    // from an auth token; for the dev CLI we hard-code an analyst persona.
    preflight_policy_check(&cik)?;

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

    let context = seed_context(&sections)?;
    let mut engine = Engine::new();
    engine.register_suggestor(RiskFactorDriftSuggestor::new());
    engine.register_suggestor(RiskFactorLanguageSuggestor::new());
    engine.register_invariant(RiskFactorMassConservationInvariant);
    engine.set_hitl_policy(EngineHitlPolicy {
        confidence_threshold: Some(HITL_CONFIDENCE_THRESHOLD),
        gated_keys: Vec::new(),
        timeout: TimeoutPolicy::default(),
    });

    let mut gated: Vec<String> = Vec::new();
    let mut step = engine.run_with_hitl(context).await;
    let result = loop {
        match step {
            RunResult::Complete(r) => {
                break r.map_err(|e| anyhow::anyhow!("engine run failed: {e:?}"))?;
            }
            RunResult::HitlPause(pause) => {
                let summary = pause.request.summary.clone();
                let gate_id = pause.request.gate_id.clone();
                gated.push(format!(
                    "gate={gate_id} cycle={cycle} summary={summary:?}",
                    cycle = pause.cycle
                ));
                tracing::warn!(
                    %gate_id,
                    cycle = pause.cycle,
                    summary = %summary,
                    "auto-approving HITL gate (confidence ≤ {HITL_CONFIDENCE_THRESHOLD}); \
                     interactive review path lands when there's a UI to host it"
                );
                let decision = GateDecision::approve(gate_id, "fathom-sparc:auto-approver");
                step = engine.resume(*pause, decision).await;
            }
        }
    };

    tracing::info!(
        cycles = result.cycles,
        converged = result.converged,
        stop_reason = ?result.stop_reason,
        gated = gated.len(),
        "engine finished"
    );
    if !gated.is_empty() {
        eprintln!(
            "INFO: {} HITL gate(s) auto-approved during this run:",
            gated.len()
        );
        for g in &gated {
            eprintln!("  - {g}");
        }
    }

    let promoted = result.context.get(ContextKey::Proposals);
    if promoted.is_empty() {
        println!("no proposals promoted to facts");
        return Ok(());
    }

    let view: Vec<FactView<'_>> = promoted.iter().map(FactView::from).collect();
    println!("{}", serde_json::to_string_pretty(&view)?);
    Ok(())
}

/// Portfolio coverage formation: load every fixture, register the per-CIK
/// drift suggestors plus the portfolio_seed → PortfolioSuggestor chain,
/// converge, print the selected portfolio + its rationale.
async fn portfolio(budget: i64, fixtures_dir: &std::path::Path) -> anyhow::Result<()> {
    let sections = load_all_sections(fixtures_dir)?;
    if sections.is_empty() {
        bail!("no fixtures found under {}", fixtures_dir.display());
    }
    let cik_count = sections
        .iter()
        .map(|s| s.filing.cik.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    tracing::info!(
        sections = sections.len(),
        cik_count,
        budget,
        "loaded fixtures for portfolio formation"
    );

    let mut context = seed_context(&sections)?;
    // Override the placeholder budget the seed suggestor will write. The
    // PortfolioSuggestor reads `portfolio-budget:fathom-sparc:portfolio:risk-coverage`
    // implicitly via the request body's budget field, so we pre-seed a
    // *budget directive* fact under Constraints that the seed suggestor
    // honours when it builds the request.
    context
        .add_input_with_provenance(
            ContextKey::Constraints,
            "portfolio-budget:risk-coverage",
            budget.to_string(),
            "fathom-sparc:cli",
        )
        .map_err(|e| anyhow::anyhow!("failed to seed budget: {e:?}"))?;

    let mut engine = Engine::new();
    engine.register_suggestor(RiskFactorDriftSuggestor::new());
    engine.register_suggestor(RiskFactorLanguageSuggestor::new());
    engine.register_suggestor(PortfolioCoverageSeedSuggestor::new());
    engine.register_suggestor(PortfolioSuggestor);
    // Ferrox HighsMipSuggestor — registered when built with
    // `--features=ferrox-mip`. Both suggestors then compete on
    // ContextKey::Strategies; engine merges in registration order.
    #[cfg(feature = "ferrox-mip")]
    engine.register_suggestor(ferrox::mip::HighsMipSuggestor);
    engine.register_invariant(RiskFactorMassConservationInvariant);
    engine.set_hitl_policy(EngineHitlPolicy {
        confidence_threshold: Some(HITL_CONFIDENCE_THRESHOLD),
        gated_keys: Vec::new(),
        timeout: TimeoutPolicy::default(),
    });

    let mut step = engine.run_with_hitl(context).await;
    let mut gated = 0usize;
    let result = loop {
        match step {
            RunResult::Complete(r) => {
                break r.map_err(|e| anyhow::anyhow!("engine run failed: {e:?}"))?;
            }
            RunResult::HitlPause(pause) => {
                gated += 1;
                let gate_id = pause.request.gate_id.clone();
                tracing::warn!(%gate_id, cycle = pause.cycle, "auto-approving HITL gate");
                let decision = GateDecision::approve(gate_id, "fathom-sparc:auto-approver");
                step = engine.resume(*pause, decision).await;
            }
        }
    };

    tracing::info!(
        cycles = result.cycles,
        converged = result.converged,
        stop_reason = ?result.stop_reason,
        gated,
        "portfolio engine finished"
    );

    // Output is a structured pair so both solvers' answers are visible side
    // by side when both are registered. With the foundation suggestor only,
    // `mip_plans` is empty; with `--features=ferrox-mip`, both populate and
    // a reader can verify they agree (or notice when they disagree).
    let strategies = result.context.get(ContextKey::Strategies);
    let portfolio_selections: Vec<PortfolioSelection> = strategies
        .iter()
        .filter(|f| f.id().as_str().starts_with("portfolio-selection:"))
        .filter_map(|f| serde_json::from_str(f.content()).ok())
        .collect();
    let mip_plans: Vec<serde_json::Value> = strategies
        .iter()
        .filter(|f| f.id().as_str().starts_with("mip-plan:"))
        .filter_map(|f| serde_json::from_str(f.content()).ok())
        .collect();

    if portfolio_selections.is_empty() && mip_plans.is_empty() {
        println!("no portfolio selection promoted; check that drift+language suggestors fired");
        return Ok(());
    }

    let envelope = serde_json::json!({
        "portfolio_selections": portfolio_selections,
        "mip_plans": mip_plans,
    });
    println!("{}", serde_json::to_string_pretty(&envelope)?);
    Ok(())
}

fn load_all_sections(
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
            Ok(s) => out.push(s),
            Err(err) => tracing::warn!(?path, error = %err, "skipping unreadable fixture"),
        }
    }
    out.sort_by(|a, b| {
        a.filing
            .cik
            .as_str()
            .cmp(b.filing.cik.as_str())
            .then(a.filing.fiscal_year.cmp(&b.filing.fiscal_year))
    });
    Ok(out)
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

/// Cedar-policy preflight via `arbiter`. Constructs a `DecideRequest`
/// describing *who* is asking *what* of *which* resource, evaluates the
/// embedded policy, and aborts with a descriptive error on deny/escalate.
///
/// This is the **front gate** in the Fathom — SPARC gate taxonomy. It runs
/// before the Converge engine starts; it does not consume engine cycles or
/// produce facts. Pair it with the **back gate** (`EngineHitlPolicy` for
/// low-confidence proposals) and the **acceptance gate**
/// (`RiskFactorMassConservationInvariant`).
fn preflight_policy_check(cik: &Cik) -> anyhow::Result<()> {
    let engine = PolicyEngine::from_policy_str(POLICY_TEXT)
        .map_err(|e| anyhow::anyhow!("loading policy: {e:?}"))?;
    let request = analyst_decide_request(cik);
    let decision = engine
        .evaluate(&request)
        .map_err(|e| anyhow::anyhow!("policy eval: {e:?}"))?;
    match decision.outcome {
        PolicyOutcome::Promote => {
            tracing::info!(
                principal = %decision.principal_id,
                resource = %decision.resource_id,
                "policy preflight: permitted"
            );
            Ok(())
        }
        PolicyOutcome::Reject => bail!(
            "policy denied analysis of CIK {}: {}",
            cik.as_str(),
            decision.reason.unwrap_or_else(|| "no reason given".into())
        ),
        PolicyOutcome::Escalate => bail!(
            "policy escalation required for CIK {}: {}",
            cik.as_str(),
            decision.reason.unwrap_or_else(|| "no reason given".into())
        ),
    }
}

/// Constructs the `DecideRequest` the dev CLI submits to `arbiter`. In
/// production the principal would come from an auth token; for the dev
/// binary the analyst persona is hard-coded.
fn analyst_decide_request(cik: &Cik) -> DecideRequest {
    DecideRequest {
        principal: PrincipalIn {
            id: "agent:fathom-sparc:analyst".into(),
            authority: AuthorityLevel::Participatory,
            domains: vec!["financial-analysis".into()],
            policy_version: None,
        },
        resource: ResourceIn {
            id: format!("flow:fathom-sparc:analyse:{}", cik.as_str()).into(),
            resource_type: Some("research".into()),
            phase: Some(FlowPhase::Framing),
            gates_passed: None,
        },
        action: FlowAction::Propose,
        context: Some(ContextIn::default()),
        delegation_b64: None,
    }
}

/// Stages each loaded section as an *input* into a fresh `ContextState`.
/// The engine drains these proposals at cycle 0 and promotes them to
/// authoritative `Fact`s under `ContextKey::Signals` — which is exactly
/// what the suggestors then read from.
fn seed_context(sections: &[RiskFactorSection]) -> anyhow::Result<ContextState> {
    let mut ctx = ContextState::new();
    for s in sections {
        let id = format!(
            "filing::{}::{}",
            s.filing.cik.as_str(),
            s.filing.fiscal_year
        );
        let content = serde_json::to_string(s)?;
        ctx.add_input_with_provenance(
            ContextKey::Signals,
            id.clone(),
            content,
            "fathom-sparc:fixture",
        )
        .map_err(|e| anyhow::anyhow!("add_input failed for {id}: {e:?}"))?;
    }
    Ok(ctx)
}

/// Display-friendly projection of a promoted `ContextFact` — strips the
/// internal id type wrapper so the JSON output reads cleanly. Provenance is
/// recovered from the fact's promotion record (the actor that promoted it).
#[derive(serde::Serialize)]
struct FactView<'a> {
    key: &'a str,
    id: String,
    content: serde_json::Value,
    promoted_by: String,
}

impl<'a> From<&'a ContextFact> for FactView<'a> {
    fn from(f: &'a ContextFact) -> Self {
        let key = match f.key() {
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
        let raw = f.content();
        let content = serde_json::from_str(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
        Self {
            key,
            id: f.id().to_string(),
            content,
            promoted_by: format!("{:?}", f.promotion_record().approver()),
        }
    }
}

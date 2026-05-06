# Fathom — SPARC

**Fathom** runs SPARC workflows for convergence-driven analysis of public-company financial filings.

Fathom turns the SEC EDGAR corpus and the wider HuggingFace financial-data
ecosystem into a queryable substrate that **Organism formations** interrogate
through **Converge** suggestors and **Prism** analytic packs. The output isn't
a summary — it's a set of provenance-bearing facts that survived a promotion
gate, plus the disagreements that didn't.

---

## The problem

A 10-K is a 200-page document that simultaneously reports financial reality,
narrates strategy, discloses risk, and signals confidence. Three readers will
take three different things from it. A coverage analyst reads ten a week. A
portfolio manager wants signal across the whole market every quarter. An
M&A team wants to know what changed since the last filing.

The information density is enormous and the questions are specialised — but
the standard tools collapse everything down to one perspective:

| Class of tool | What it does | What gets lost |
|---|---|---|
| Bloomberg / Sentieo / S&P Capital IQ | Surface curated metrics, headlines, alerts | The textual softening, the omitted segment, the auditor language change. Pre-extracted fields only. |
| RAG over filings (chat-with-your-docs) | Plausible-sounding answers from chunked text | Provenance, confidence, repeatability. Two queries an hour apart give two different answers; neither is auditable. |
| Single-model scoring (sentiment, fraud-risk classifiers) | One number per filing | Why. The number is a black box; an analyst can't act on it without re-deriving the reasoning by hand. |

The shared failure mode is **early collapse**. Many perspectives get flattened
into a single output (a metric, a paragraph, a score) before a human sees
them. The interesting signal in financial filings is often in the
*disagreement* between perspectives — language softened while numbers
strengthened, segment accelerated while management warned of headwinds — and
early collapse destroys exactly that signal.

---

## The Fathom approach

Keep the perspectives separate. Run each as an independent **suggestor**.
Promote only what survives a deterministic gate. Surface the disagreements as
first-class output, not noise.

A single question against a single filing typically engages five to ten
suggestors. Each one queries a slice of the lakehouse, applies a narrow
analytic, and proposes a fact:

```
Question: "Is MegaCorp's latest 10-K a yellow flag?"

  RiskFactorDriftSuggestor       → proposes risk_factor_count_delta_yoy = +12
  RiskFactorLanguageSuggestor    → proposes hedging_score_delta = +0.18
  SegmentRevenueSuggestor        → proposes segment_growth_dispersion = 0.34
  MdnaToneSuggestor              → proposes mgmt_confidence_delta = -0.21
  AuditorLanguageSuggestor       → proposes audit_emphasis_count = 2 (was 0)
  RestatementSuggestor           → proposes prior_period_restated = false
  InsiderActivitySuggestor       → proposes net_insider_sales_90d = +$47M

  Converge engine
    → promotes facts that meet eligibility & integrity rules
    → flags disagreement: language softened (-0.21) while
       segment numbers strengthened (+) — escalate to HITL
    → produces signed run artifact (every promoted fact carries
       its suggestor, its data slice, and its query plan)
```

The output isn't a recommendation. It's a structured, auditable set of facts
with explicit confidence boundaries — the input an analyst, a model, or a
downstream formation can actually reason against.

---

## Why Converge

Converge is the kernel that makes the above tractable. Four properties matter
for financial analysis specifically:

**Promotion is gated, not consensus-averaged.** A `ProposedFact` is freely
constructible by any suggestor; the authoritative `Fact` is kernel-gated.
Eligibility and merge order follow registration order, not ad hoc side
channels. This means *which* conclusions enter the record is a deliberate,
inspectable choice — not the artefact of whichever model ran last or had the
loudest output.

**Every promoted fact carries provenance.** When `risk_factor_count_delta_yoy
= +12` is promoted, the run artifact includes which suggestor produced it,
what query it ran, against which Iceberg snapshot, with what parameters. An
auditor, a regulator, or a curious portfolio manager can reproduce the number
six months later — bit-for-bit. This is table stakes for finance and it is
the property that RAG fundamentally cannot offer.

**The loop is deterministic.** Re-running the same formation against the same
lakehouse snapshot produces the same promoted facts. This makes regression
testing a real possibility — when you change a suggestor or upgrade a model,
you can compare the new run's facts against the old run's facts at the level
of individual promotions, not just final scores.

**Human review is first-class, not an exception path.** A suggestor or a
disagreement pattern can request HITL pause. The engine stops, surfaces the
context, and resumes when reviewed. For decisions that affect capital
allocation this isn't a feature — it's a requirement, and bolting it onto a
RAG pipeline after the fact is structurally awkward.

These four properties are what turn "an LLM looked at the 10-K" into "a
defensible analytical artefact."

---

## Why Organism

Organism is the layer that turns one-off analyses into a repeatable practice.
The same suggestors get reassembled into different **formations** for
different questions:

| Formation | Question it answers | Suggestor mix |
|---|---|---|
| `DisclosureRiskFormation` | Is this single 10-K a yellow flag? | Risk-factor drift, MD&A tone, auditor language, restatements |
| `SegmentTruthFormation` | Are the segment narratives consistent with the segment numbers? | Segment revenue, MD&A tone, footnote anomaly, peer-relative growth |
| `PortfolioScreenFormation` | Across our 200 holdings, who shows yellow flags this quarter? | Same as `DisclosureRiskFormation`, fanned out, ranked |
| `MAScreenFormation` | Of these 50 acquisition candidates, which have hidden write-down risk? | Restatements, goodwill anomaly, auditor language, working-capital trend |
| `EarningsPrepFormation` | What questions should we ask on the call? | Disagreement-finder across all of the above, ranked by surprise |

The key insight: **the suggestors don't change between formations**. The
`AuditorLanguageSuggestor` doesn't know whether it's running inside
`DisclosureRiskFormation` or `MAScreenFormation` — it just queries its slice
and proposes its fact. The formation is the composition, not the work. New
analytical products are days of wiring, not weeks of integration.

This is what "bringing clarity to complex data problems" actually looks like:
not a smarter model, but a *separation* — between the substrate (lakehouse),
the perspectives (suggestors), the gate (Converge engine), and the
composition (Organism formation). Each layer has one job, each layer is
inspectable, and each layer is reusable.

---

## Why a real lakehouse underneath

Most "AI on financial documents" projects skip this step. They chunk PDFs,
embed them, store vectors, and call it done. That works for one analyst
asking one-off questions. It collapses the moment you want to ask the same
question across a portfolio, or compare this filing against the same
company's filings five years ago.

Fathom materialises the corpus into Iceberg tables on object storage —
partitioned by `(cik, fiscal_year, form_type)` — and queries them with
DataFusion in-process or Sail when distributed. This buys three things:

- **Time-travel.** Iceberg snapshots let a formation run *as of* a specific
  date, which matters when you're back-testing a signal or auditing a
  historical decision.
- **Schema evolution without re-ingestion.** Add a new column extracted from
  the same filings (e.g. parsed segment table) without re-downloading the
  corpus.
- **Same SQL local and distributed.** A suggestor written against DataFusion
  on a laptop runs unchanged against a Sail cluster when the corpus grows.
  The migration story is "change the connection string."

The lakehouse is the *durable* layer. Suggestors and formations come and go;
the materialised corpus pays for itself across all of them.

---

## Stack

```
HuggingFace (EDGAR corpus, financial datasets)
          │  one-time + scheduled delta
          ▼
       Sail  ◄──── distributed
       DataFusion  ◄──── in-process (laptop / single node)
          │  materialises into
          ▼
       Iceberg tables on S3 (RustFS for local dev)
          │   partitioned (cik, fiscal_year, form_type)
   ┌──────┴──────────┐
   ▼                 ▼
Prism packs    Fathom suggestors
(stats, ML)    (domain-specific queries)
   │                 │
   └──────► Converge ◄──────┘
              │   engine, promotion gate, integrity proof, HITL
              ▼
        Promoted Facts  (provenance-bearing, deterministic, auditable)
              │
              ▼
        Organism formations
              │
              ▼
   Decisions / artefacts:
     - "10-K yellow-flag report"
     - "portfolio screen ranked by disclosure risk"
     - "earnings-call question list"
```

---

## Crate layout

| Crate | Owns |
|---|---|
| `fathom-core` | Domain types: `Filing`, `Company`, `Section`, `Period`, `FilingId`, `Cik` |
| `fathom-ingest` | HuggingFace → DataFusion → Iceberg materialisation |
| `fathom-suggestors` | Converge suggestors that query Iceberg slices and propose facts |
| `fathom-cli` | Binary entry point; assembles formations and runs convergence |

Path-deps to `~/dev/work/converge`, `~/dev/extensions/prism`, and
`~/dev/work/organism` for co-development. Versions of arrow, datafusion,
parquet, and object_store are pinned to match Sail's foundation, so the
stack is consistent whether you run DataFusion in-process or Sail
distributed.

---

## Datasets (HuggingFace candidates)

- [`JanosAudran/financial-reports-sec`](https://huggingface.co/datasets/JanosAudran/financial-reports-sec) — 10-K/10-Q with sentence-level labels. Good for first slice.
- [`eloukas/edgar-corpus`](https://huggingface.co/datasets/eloukas/edgar-corpus) — annual reports 1993–2020, item-segmented. Good for time-series suggestors.
- [`AdaptLLM/finance-tasks`](https://huggingface.co/datasets/AdaptLLM/finance-tasks) — task-oriented finance corpora, useful for evaluation.

Start with one CIK and one filing year before materialising the full corpus.

---

## Local infrastructure

```bash
just up      # RustFS (S3-compatible) on :9000/:9001 + Sail server on :50051
just down
just logs
```

RustFS replaces MinIO as the local S3 endpoint. Sail is wired to it with
`AWS_ENDPOINT_URL=http://rustfs:9000`. The same Iceberg tables produced
locally are byte-compatible with the same tables on AWS S3 — moving from
laptop to cloud is configuration, not code.

---

## Status

**1.0.0 — architecture + dependency surface.** Four crates exist; platform
deps (Converge 3.7.6, Organism 1.4.0, Prism via git, Sail-aligned
arrow/datafusion/parquet/object_store) are wired; local infra (RustFS + Sail
in `compose.yml`) is in place.

**1.1.0 — first end-to-end slice (shipped).** `RiskFactorDriftSuggestor`,
fixture-based ingest, CLI wiring against an in-memory `Context`. Real Apple
10-K data: Item 1A risk factor headings extracted from `aapl-20240928.htm`
and `aapl-20250927.htm` on SEC EDGAR, committed under `fixtures/`.

**1.2.0 — disagreement-as-signal (shipped).** Second suggestor —
`RiskFactorLanguageSuggestor` — runs alongside the count drift and proves
the architecture's central claim: **two perspectives that disagree are more
informative than one perspective averaged.**

```text
$ fathom-sparc analyse 0000320193

count drift     →  28 → 27   (delta -1, visually quiet)
language drift  →  jaccard 0.618, +6 added / -7 removed (loud)

  - Removed: "The Company's retail stores are subject to numerous risks…"
  - Removed: "Expectations relating to environmental, social and
              governance considerations…"
  + Added:   "Varied stakeholder expectations about social and other
              issues…"                       ← softer ESG framing
  - Removed: "carriers, wholesalers, retailers and other resellers"
  + Added:   "carriers and other resellers"  ← narrowed
```

If Fathom collapsed these to a single number — sentiment score, risk
rating, anomaly probability — the qualitative shift would vanish. Keeping
the perspectives separate, with provenance per fact, is the entire point.

What's wired in 1.2.0:

| Crate | Owns |
|---|---|
| `fathom-sparc-core::analytic` | `RiskFactorDrift` (count) + `RiskFactorLanguageDrift` (Jaccard, identical_count, added/removed lists) |
| `fathom-sparc-ingest::load_risk_factor_fixture` | JSON → `RiskFactorSection` |
| `fathom-sparc-suggestors::RiskFactorDriftSuggestor` | provenance `fathom:risk_factor_drift:v1` |
| `fathom-sparc-suggestors::RiskFactorLanguageSuggestor` | provenance `fathom-sparc:risk_factor_language:v1` |
| `fathom-sparc-cli` | discovers fixtures by CIK, runs both suggestors sequentially against a hand-rolled `Context`, prints proposals as JSON |

Tests: 11 unit (5 drift + 6 language) + 2 ingest + 2 binary integration =
**15 passing**.

**Deliberate deferral in 1.2.0.** The CLI runs each `Suggestor::execute()`
directly rather than through `Engine::run()`. This was the right call when
there was only one suggestor (no scheduling decisions to make), but with two
it now hides what the engine actually contributes — eligibility checks,
deterministic merge order, the promotion gate that turns `ProposedFact` into
authoritative `Fact` with full provenance. Wiring the real engine is the
1.3.0 slice.

**Next slices.**

1. **`Engine::run()` integration (1.3.0).** Replace the hand-rolled
   `MockContext` with a real `ContextState`, register both suggestors with
   the engine, run the convergence loop, inspect promoted `Fact`s. This is
   what makes the architecture diagram in this README actually true.
2. **Real ingest from SEC EDGAR (1.4.0).** Replace fixtures with a Rust
   port of the python heading extractor (italic+bold span detection),
   writing `RiskFactorSection` rows to Iceberg via DataFusion. Same
   downstream shape; suggestors unchanged.
3. **HuggingFace pipeline (1.5.0).** Fan out across the full
   `JanosAudran/financial-reports-sec` corpus into the same Iceberg
   tables, partition by `(cik, fiscal_year, form_type)`.
4. **Organism formation (2.0.0).** Assemble both suggestors into
   `DisclosureRiskFormation`. Run across a portfolio of CIKs for a
   screen-style output. This is the moment Organism earns its keep.

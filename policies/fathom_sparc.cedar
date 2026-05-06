// Fathom — SPARC analytical access policy.
//
// Cedar policy evaluated at the CLI boundary (preflight), before the
// Converge engine runs. This is the *front* gate: "should this analysis
// even be attempted?" — orthogonal to HITL (the *back* gate: "should this
// analytical conclusion be admitted as truth?") and to invariants
// ("are the promoted facts internally consistent?").
//
// In production this policy would be loaded from a configuration store and
// the principal would come from an auth token. For 1.5.0 the policy is
// version-controlled in this repository and the CLI hard-codes a
// development principal — see `crates/fathom-sparc-cli/src/main.rs`.
//
// Principal attributes (set by the CLI):
//   - id        : PrincipalId  (e.g. "agent:fathom-sparc:analyst")
//   - authority : AuthorityLevel — advisory | supervisory | participatory | sovereign
//   - domains   : Set<DomainId>  — typically {"financial-analysis"}
//
// Resource attributes:
//   - id            : ResourceId    — "flow:fathom-sparc:analyse:{cik}"
//   - resource_type : ResourceKind  — "research"
//   - phase         : FlowPhase     — "framing" at preflight
//
// Action: "propose" — about to launch the convergence loop that will
// emit ProposedFacts.

// Default-deny: nothing is allowed unless an explicit `permit` matches.

// Authorised analysts may propose research analyses.
permit(principal, action == Action::"propose", resource)
when {
  resource.resource_type == "research" &&
  principal.domains.contains("financial-analysis") &&
  (principal.authority == "participatory" ||
   principal.authority == "supervisory" ||
   principal.authority == "sovereign")
};

// Advisory-only personas cannot launch new analyses — they consume results.
forbid(principal, action == Action::"propose", resource)
when {
  resource.resource_type == "research" &&
  principal.authority == "advisory"
};

# GrillForge Engineering Creed

These rules are non-negotiable for every implementation and review in this
repository.

## Small and Beautiful

- Build the smallest complete solution that satisfies the current requirement.
- Prefer clear, direct code over frameworks, indirection, and speculative
  extensibility.
- Add an abstraction only after it protects a real boundary or removes proven
  duplication.
- Keep modules cohesive, names precise, control flow visible, and dependencies
  one-directional.
- Do not copy an entire upstream subsystem when a tested capability slice is
  sufficient.

## Fail Fast

- Validate configuration at the boundary where it enters the system.
- Return a typed, actionable error immediately when configuration or runtime
  state is invalid.
- Never silently repair, reinterpret, downgrade, or ignore invalid input.
- Never hide failures behind catch-and-continue behavior.
- Do not add unbounded retries, fallback chains, compatibility layers, shadow
  state, or duplicate sources of truth.
- Protocol fallback must be an explicit user choice. Authentication, quota,
  model, and endpoint errors are surfaced as-is with safe context.

## Minimal Safety, Not Redundancy

- Atomic writes, credential redaction, and one recoverable configuration
  snapshot are required safety boundaries.
- Safety mechanisms must remain narrow, deterministic, and testable.
- Do not build backup generations, repair daemons, automatic failover systems,
  or self-healing state machines unless a later explicit requirement demands
  them.

## Scope Discipline

- Implement only the current MVP and the interfaces required by a real current
  use case.
- Future Agent Adapters may shape clean boundaries, but must not create empty
  implementations, plugin frameworks, registries, factories, or marketplaces
  today.
- Every dependency, module, configuration field, and background task must have
  a concrete MVP consumer.
- Remove dead branches and unused compatibility code instead of preserving them
  “just in case.”

## Test-Driven Delivery

- Build each behavior as one vertical RED -> GREEN -> REFACTOR cycle.
- Tests exercise public interfaces and observable results, not private methods
  or internal call counts.
- Mock only true system boundaries. Prefer temporary real files, a real local
  HTTP server, and the real public service interface over mocking our own code.
- A mock-backed protocol test does not prove connectivity. Every supported
  engineering route must pass its opt-in live end-to-end test before release.
- Never place real credentials in fixtures, source files, snapshots, logs, or
  default CI jobs.

## Error Experience

- Backend errors identify the failed operation and safe remediation without
  leaking credentials or prompt content.
- The GUI shows the first actionable error close to the setting or action that
  caused it.
- A failed change leaves the previous valid state active.
- Logs support diagnosis; they are not a substitute for returning an error.

When two implementations satisfy the same behavior, choose the one with fewer
concepts, less state, and a shorter path from input to result.

# GrillForge Architecture Supplement

Status: Accepted

This document supplements `GRILLFORGE_SPEC_FINAL.md` and
`GRILLFORGE_REVIEW_FINAL.md`. Where product positioning or integration details
conflict, this document and the later requirements recorded here take
precedence.

## Product Positioning

GrillForge is the control plane for AI coding agents.

The MVP integrates the pinned cc-switch client list plus Pi, while the core
domain remains independent from every client integration. The shared Provider
and Model Registry is consumed by Claude Code, Claude Client, Codex, Pi, Gemini
CLI, Grok Build, OpenCode, OpenClaw, and Hermes Adapters.

GrillForge remains a control plane. It does not become an agent runtime, workflow
engine, scheduler, agent marketplace, or replacement coding agent.

The local gateway authorized by the later requirements is a model API protocol
bridge. It may translate tool-call payloads, but it never executes tools or
owns the agent loop; therefore it is not the prohibited custom tool-calling
proxy or Agent Runtime from the original specification.

## Architectural Boundaries

The following concepts are separate:

1. `AgentAdapter` connects GrillForge to a coding agent.
2. `Provider` describes an upstream model service and its credentials.
3. `ModelRegistry` describes available models, capabilities, and enablement.
4. `ProtocolBridge` adapts model API protocols when direct compatibility is not
   available.

An agent is not a provider, and a provider is not a model.

```text
GrillForge Core
    |
    +-- Agent Adapter Layer
    |      +-- Claude Code Adapter (MVP)
    |      +-- Codex Adapter (future)
    |      +-- Pi Adapter (future)
    |      +-- Kimi Code Adapter (future)
    |
    +-- Provider Layer
    |      +-- Anthropic
    |      +-- OpenAI Responses / Compatible
    |      +-- Local models
    |
    +-- Model Registry
```

## Agent Adapter Contract

The MVP contract stays deliberately small. It covers only operations already
required by the Claude Code integration:

```rust
pub trait AgentAdapter {
    fn id(&self) -> &'static str;
    fn detect(&self) -> Result<AgentStatus, AgentAdapterError>;
    fn install_integration(&self, context: &InstallContext)
        -> Result<(), AgentAdapterError>;
    fn apply_configuration(&self, config: &AgentConfiguration)
        -> Result<(), AgentAdapterError>;
    fn restore_configuration(&self) -> Result<(), AgentAdapterError>;
    fn status(&self) -> Result<AgentStatus, AgentAdapterError>;
}
```

The core passes neutral data structures into this interface. Claude paths,
environment variable names, Skill locations, and Claude Desktop profiles stay
inside the Claude Code adapter.

Adding another agent may add an adapter module and composition-root
registration, but must not require changes to provider, model registry, or
model-routing business rules. A dynamic plugin system is explicitly outside
the MVP.

## Provider and Protocol Compatibility

All model-provider presets, configuration shapes, authentication behavior,
model mapping, request/response transformations, streaming transformations,
tool-call conversion, reasoning conversion, multimodal handling, and Codex
OAuth behavior are learned from and adapted from cc-switch.

Initial upstream baseline:

```text
repository: https://github.com/farion1231/cc-switch
commit: 413c09e0790c304506888ae24b9be72820aca126
license: MIT
local reference: upstream/cc-switch
```

Substantial copied code must retain the upstream MIT copyright and license
notice.

Compatibility priority:

1. Native Anthropic Messages API: direct pass-through.
2. OpenAI Responses API: bridge Anthropic Messages to and from Responses.
3. OpenAI Chat Completions-compatible API: bridge Anthropic Messages to and
   from Chat Completions.
4. Other protocol support present in selected cc-switch providers is ported
   from the corresponding upstream adapter rather than independently designed.

The cc-switch application-level `AppType` branching must not leak into
GrillForge Core. Agent-specific configuration takeover belongs to an
`AgentAdapter`; reusable model protocol conversion belongs to the Provider
Layer.

## MVP Module Layout

The exact Rust crate boundaries may be adjusted during scaffolding, but the
dependency direction is fixed:

```text
src-tauri/src/
  core/
    provider/
    model_registry/
    routing/
  agent_adapters/
    mod.rs
    claude_code/
      detection.rs
      configuration.rs
      skill.rs
      status.rs
  provider_protocols/
    anthropic/
    openai_responses/
    openai_chat/
  infrastructure/
    config_store/
    secrets/
    local_gateway/
```

Rules:

- `core/provider` and `core/model_registry` never import an agent adapter.
- `core/routing` selects an enabled model/provider using neutral identifiers.
- `agent_adapters/claude_code` may depend on core interfaces, never the reverse.
- Protocol bridges never read or write Claude Code configuration.
- UI state is obtained through application services, not by directly editing
  agent configuration files.

## Client Adapter MVP Behavior

The first release implements independent tested Adapters for Claude Code,
Claude Client, Codex, Pi, Gemini CLI, Grok Build, OpenCode, OpenClaw, and
Hermes. Claude Code additionally:

- Detect Claude Code.
- Install and update the GrillForge Skill.
- Back up, apply, and restore Claude Code configuration atomically.
- Expose the local Anthropic-compatible gateway when external models are active.
- Expose agent-safe model routes for external SubAgent routing.
- Use `CLAUDE_CODE_SUBAGENT_MODEL` only for an explicitly forced single Worker;
  leave it unset when the Skill must choose among multiple Worker models.
- Keep the main Claude route separate from external SubAgent model routes.
- Prefer a native Anthropic upstream; use cc-switch-derived protocol conversion
  for Responses or Chat Completions upstreams.
- Restore native Claude behavior when the external model pool is disabled.

cc-switch currently selects a provider globally per application. GrillForge must
add a thin model-to-provider routing rule so the Claude main request and an
external SubAgent request can resolve to different providers. This routing rule
is GrillForge domain behavior; the underlying provider protocol adapters remain
cc-switch-derived.

## Configuration Ownership

The target persistent shape remains:

```text
~/.grillforge/
  config.yaml
  models.yaml
  agents.yaml
```

`agents.yaml` stores agent enablement and adapter selection. It must not contain
provider secrets or duplicate the model registry.

```yaml
agents:
  claude_code:
    enabled: true
    adapter: claude_code
  codex:
    enabled: false
    adapter: codex
  pi:
    enabled: false
    adapter: pi
```

Only clients with a tested executable Adapter appear as configurable. Future
entries must not produce placeholder runtime behavior.

## MVP Exclusions

The future adapter architecture does not authorize implementation of:

- Kimi Code or clients without a tested current configuration contract
- Dynamic adapter plugins
- Agent marketplace
- Agent runtime
- Workflow engine
- Task scheduler
- Custom SubAgent lifecycle management

## Acceptance Constraints

The MVP architecture is acceptable when:

1. Claude Code is the only implemented Agent Adapter.
2. Provider and model registry tests run without Claude-specific types.
3. Claude-specific paths and environment keys exist only in the Claude adapter
   or its integration tests.
4. Provider protocol conversion is reusable by a future Agent Adapter.
5. Adding an Agent Adapter does not require changing provider or model registry
   logic.
6. No speculative runtime or plugin framework is introduced.

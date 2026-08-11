# GrillForge Architecture

Status: Canonical architecture

## System View

```text
                          GrillForge
                    Model Control Plane
                             |
              +--------------+--------------+
              |                             |
      Main Model Manager          Worker Model Manager
              |                             |
              +--------------+--------------+
                             |
                    Agent Adapter Layer
                             |
              +--------------+--------------+
              |              |              |
       Claude adapters   Codex adapter   Pi / Kimi adapters
              |              |              |
       Gemini / Grok / OpenCode / OpenClaw / Hermes adapters
                             |
                       Provider Layer
                             |
       Anthropic / OpenAI Responses / OpenAI Chat / Gemini / Local
                             |
                       Model Registry
```

## Simplicity and Failure Semantics

The architecture optimizes for the fewest concepts needed to deliver correct
MVP behavior.

- Invalid configuration is rejected at the entry boundary.
- Runtime failures are returned immediately as typed errors.
- There is no automatic provider failover, unbounded retry, silent protocol
  downgrade, background repair loop, or duplicate configuration state in the
  MVP.
- A failed configuration application keeps the previous valid state.
- Atomic writes and one recoverable pre-change snapshot are the complete MVP
  recovery mechanism.
- Upstream cc-switch code is ported by tested capability slice; unrelated
  infrastructure is not copied.

## Domain Boundaries

### Agent Adapter

An Agent Adapter knows how to connect GrillForge to one coding agent. It owns:

- Agent installation detection
- Agent-specific configuration locations and formats
- Configuration backup, application, verification, and restoration
- Skill or plugin installation for that agent
- Agent integration and health status
- Translation between neutral GrillForge model references and agent-facing model
  identifiers
- The descriptor for model Slots supported by that client, including whether a
  Slot accepts one model or a model pool

Claude paths, Claude environment variables, and Claude Desktop profile details
must not appear in GrillForge Core.

The MVP interface remains small:

```rust
pub trait AgentAdapter {
    fn id(&self) -> &'static str;
    fn detect(&self) -> Result<AgentStatus, AgentAdapterError>;
    fn install_integration(
        &self,
        context: &InstallContext,
    ) -> Result<(), AgentAdapterError>;
    fn apply_configuration(
        &self,
        config: &AgentConfiguration,
    ) -> Result<(), AgentAdapterError>;
    fn restore_configuration(&self) -> Result<(), AgentAdapterError>;
    fn status(&self) -> Result<AgentStatus, AgentAdapterError>;
}
```

Adding an agent may add an adapter module and composition-root registration. It
must not require changes to Provider or Model Registry business rules. A
dynamic plugin system is not required.

### Provider Layer

The Provider Layer owns upstream connectivity:

- Endpoint and full-URL rules
- Authentication configuration
- API protocol selection
- Provider presets
- Connection and model-list testing
- Protocol-specific request and response handling

It does not know which coding agent initiated the request.

Protocol compatibility order:

1. Native Anthropic Messages: pass through with only required normalization.
2. OpenAI Responses: bridge to and from Anthropic Messages.
3. OpenAI Chat Completions-compatible: bridge to and from Anthropic Messages.
4. Gemini Native: bridge to and from Anthropic Messages.

Protocol format is selected by a cc-switch-derived preset or explicit user
configuration. GrillForge must not blindly retry another protocol after an
authentication, quota, or model error.

### Model Registry

The Model Registry owns neutral model metadata:

- Stable GrillForge model ID
- Provider ID
- Upstream model ID
- Display name
- Capability tags
- Context/output capability metadata where known
- Global availability and Worker enablement

It does not contain agent configuration paths or API protocol conversion code.
Native agent defaults are not forced into the registry. `Native/Default` is an
Agent Adapter selection state, not a fake `claude-default` model.

### Model Routing

Routing resolves an agent-facing model route into:

```text
Agent + route alias -> GrillForge model -> Provider -> Protocol bridge
```

Routing is neutral core behavior. Protocol conversion is delegated to the
Provider Layer, and live agent configuration is delegated to the Agent Adapter.

cc-switch currently selects a provider globally per application. GrillForge adds
only the thin model-to-provider selection needed to keep the main model and
Worker models independent. All underlying provider protocol behavior remains
cc-switch-derived.

## Source Layout

```text
src-tauri/src/
  core/
    agent.rs
    model.rs
    provider.rs
    routing.rs
  adapters/
    mod.rs
    claude_code/
    claude_desktop/
    codex/
    pi/
    kimi_code/
    gemini/
    grok_build/
    opencode/
    openclaw/
    hermes/
  bridge/
  application.rs
  configuration.rs
  gateway.rs
  integration.rs
  storage.rs
```

Dependency rules:

- `core` imports no Agent Adapter.
- `core/provider` and `core/model_registry` do not import one another's
  infrastructure.
- Agent Adapters depend on neutral core types, never the reverse.
- Protocol bridges never read or write coding-agent configuration.
- Filesystem, HTTP, and secret-storage details remain outside neutral core
  modules.
- Tauri commands call application services rather than domain internals.

## Claude Code Adapter

The MVP `ClaudeCodeAdapter` is responsible for:

- Detecting Claude Code and supported configuration locations
- Reading current integration status without changing it
- Installing/updating the GrillForge model-selection Skill
- Taking a recoverable snapshot before managed configuration changes
- Applying Claude Code settings atomically
- Exposing the local Anthropic-compatible gateway when routing is needed
- Mapping GrillForge route aliases to model IDs understood by the gateway
- Mapping the single-model `main`, `sonnet`, `opus`, `fable`, and `haiku`
  Slots to Claude Code configuration keys derived from the pinned cc-switch
  implementation
- Exposing the SubAgent Worker Pool as the MVP multi-model Slot
- Generating one GrillForge-owned native Claude Agent definition per effective
  Worker so multiple arbitrary model aliases remain selectable by
  `subagent_type`
- Restoring the exact pre-GrillForge configuration on disable

Claude Code CLI uses its settings and `ANTHROPIC_BASE_URL` gateway mechanism.
Claude Client is a second, narrow MVP Adapter for its conversation/Cowork 3P
profile and Claude-safe role routes. Its Code and background development tasks
reuse the same user/project/local Agent definitions, Skill, model Slots, and
SubAgent records, but Claude Client overrides the bundled runtime's network and
authentication environment. External Workers become routable only after the
Client 3P profile is applied; GrillForge must not create a duplicate Worker
configuration store on the Claude Client page.

## Local Gateway

The gateway is infrastructure, not an Agent Runtime.

Responsibilities:

- Accept Anthropic Messages-compatible requests from the Claude adapter
- Resolve the request's model route
- Select the main or Worker provider/model
- Replace external-provider authentication at the forwarding boundary
- Apply the configured protocol bridge
- Return Anthropic-compatible JSON or SSE
- Avoid logging credentials or sensitive request bodies by default

It does not create agents, execute tools, schedule tasks, or manage context.

For the standalone Claude Code CLI, when the main model is Native/Default and no
Worker model is active, the adapter restores native configuration and the
gateway is not involved.

For the standalone CLI, when Native/Default main and external Workers are both active, main-model
requests pass through the gateway to the native Anthropic route while Worker
aliases are sent to their configured Providers. Preservation of Claude account
authentication on this mixed route must be verified by an integration test
before release.

For this mixed route the adapter changes `ANTHROPIC_BASE_URL` but does not set a
placeholder `ANTHROPIC_AUTH_TOKEN`: Claude Code gives that variable precedence
over subscription OAuth. The gateway forwards inbound OAuth only to the native
Anthropic route and replaces it only for external Provider routes.

Claude Client's bundled Code runtime is different: the Client host injects the
official API host and authentication after reading the standalone CLI config.
The supported external route is the Client's 3P inference profile, which sends
conversation, Cowork, and bundled Code to one authenticated local gateway. The
Client gateway never reads or proxies a subscription OAuth token. Consequently,
official-subscription main plus third-party Worker is not advertised for Client
Code, and the selector rejects external Workers while the Client remains in 1P
mode.

A custom base URL disables some Claude first-party additions, including Remote
Control and default optimistic Tool Search behavior. The MVP does not emulate
those services or use private first-party override flags.

## Persistence

Target user-visible configuration:

```text
~/.grillforge/
  config.yaml
  models.yaml
  agents.yaml
```

- `config.yaml`: application and Provider configuration
- `models.yaml`: neutral model registry and capability metadata
- `agents.yaml`: Agent Adapter enablement, fixed model Slots, and named SubAgent definitions

Secrets must never be copied into a coding agent's live configuration. The MVP
secret persistence mechanism should follow the selected cc-switch-derived
implementation and enforce user-only file permissions where applicable.

All writes to GrillForge and agent configuration are atomic. Agent takeover must
have a recoverable backup and restoration path.

## GUI Architecture

The GUI is a core product component but remains intentionally small.

The GUI is client-first rather than model-first. Its v0.6 primary navigation is:

- Control Center
- Coding Agent Clients
- Providers
- Routing

Selecting a Client opens the configuration described by that Client Adapter.
The Claude Code page contains its status, four fixed single-model Slots, named
SubAgent definitions, and Apply/Disable actions. Every SubAgent has its own
stable ID, display name, Model reference, capability tags, and enabled state;
multiple SubAgents may intentionally share one Model. The Claude Client page
links back to the shared Worker definitions and exposes its independent 3P
conversation/Cowork role routes, which are also the network entry for bundled
Code. Provider credentials are never copied into a Client page; Slots and
SubAgents reference the global Model Registry.

The Routing page is read-only. It renders the persisted
`Client -> Slot or SubAgent -> Model` relationship and must never imply a
runtime route decision. Model details use only registry fields that actually
exist; pricing, context limits, and recommendation claims are not invented.

The first screen must immediately show:

- Managed/connected agent
- Number of configured fixed Slots
- Number and identity of enabled SubAgents
- Integration errors requiring action

The UI obtains state through application services exposed as Tauri commands.
It does not directly edit YAML, agent settings, or provider secrets.

The default locale is `zh-CN`; user-facing copy is routed through a small i18n
boundary that can later add `en-US`. No enterprise dashboard, workflow canvas,
agent graph, or working fake adapter is included.

## Upstream cc-switch Mapping

The pinned `upstream/cc-switch` clone is a reference source, not a runtime
dependency by default.

Primary source areas:

- Provider presets: `src/config/*ProviderPresets.ts`
- Provider metadata: `src-tauri/src/provider.rs`
- Claude Code takeover: `src-tauri/src/services/proxy.rs`
- Claude Desktop profiles: `src-tauri/src/claude_desktop_config.rs`
- Model mapping: `src-tauri/src/proxy/model_mapper.rs`
- Protocol adapters: `src-tauri/src/proxy/providers/`
- Codex OAuth: `src-tauri/src/proxy/providers/codex_oauth_auth.rs`

Code is ported by capability slice with its relevant upstream tests. Unrelated
cc-switch features such as sync, usage dashboards, MCP management, sessions,
failover UI, and other coding-agent configuration are not copied into the MVP.

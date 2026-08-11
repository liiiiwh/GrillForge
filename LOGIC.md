# GrillForge Logic

Status: Canonical behavior and invariants

## Core State

GrillForge keeps these state concepts separate:

```text
Agent connection
Main model selection
Worker model pool
Provider availability
Model registry
```

Changing one concept must not silently rewrite another.

The GUI begins with a Coding Agent Client. Models and Providers are global
assets referenced by the selected Client's Slots.

## Model Slot Logic

Each Client Adapter declares its supported Slots. Core selection state stores
only neutral Slot IDs and Model Registry IDs; agent-specific environment keys
and file formats remain inside the Adapter.

A single-model Slot has either one managed model or no GrillForge override. No
override preserves the client's native/inherited value. A multi-model Slot is
an explicit model pool. GrillForge does not interpret a list in a fixed Slot as
fallback or retry order.

Claude Code Phase 1 exposes `main`, `sonnet`, `opus`, `fable`, and `haiku` as
single-model Slots and the SubAgent Worker Pool as a multi-model Slot. The
cc-switch-derived fixed mappings are:

| Slot | Claude Code key |
| --- | --- |
| Main | `ANTHROPIC_MODEL` |
| Sonnet | `ANTHROPIC_DEFAULT_SONNET_MODEL` |
| Opus | `ANTHROPIC_DEFAULT_OPUS_MODEL` |
| Fable | `ANTHROPIC_DEFAULT_FABLE_MODEL` |
| Haiku | `ANTHROPIC_DEFAULT_HAIKU_MODEL` |

All configured Slot models must exist and use enabled Providers. Apply snapshots
every key it may change, verifies the resulting state, and restores exact prior
values on Disable.

## Main Model Logic

Each enabled Agent Adapter has one main-model selection:

```text
Native/Default
or
Managed(model_id)
```

### Native/Default

- GrillForge does not create a fake native model record.
- The Agent Adapter preserves or restores the coding agent's normal model
  behavior.
- The UI may display a detected native model name, but detection does not make
  it a managed registry entry.

### Managed Model

A managed main selection is valid only when:

- The model exists in the registry.
- Its Provider exists and is usable.
- The Agent Adapter can expose a compatible route.

Selecting it causes the Agent Adapter to apply the minimum configuration needed
to route main requests to that model. Switching back to Native/Default restores
the pre-GrillForge main-model behavior.

Main-model switching and Worker enablement are independent. A model may be the
main selection without being enabled as a Worker.

## Worker Model Pool Logic

The Worker Pool is the set of configured external models currently available
for SubAgent work.

For the MVP, an effective Worker model must satisfy all of:

```text
worker mode is ON
model is enabled for Worker use
model exists
provider exists and is enabled
provider configuration is valid
```

Capability tags such as `coding`, `refactor`, `review`, and `reasoning` describe
selection suitability. They do not grant tools or permissions.

### Enable/Disable Rules

- Any configured external model may be enabled or disabled for Worker use.
- Turning Worker mode ON requires at least one valid enabled Worker model.
- Disabling the last Worker while Worker mode is ON is rejected, unless the
  same action also turns Worker mode OFF.
- Turning Worker mode OFF keeps the user's model selections for later reuse but
  makes the effective pool empty.
- If the effective Worker Pool is empty, the Agent behaves normally and uses
  native SubAgent behavior.
- Provider deletion is rejected while referenced by a main selection or
  enabled Worker, unless the user first removes those references in the same
  operation.

## Worker Selection

The `grillforge-model-selector` Skill exposes only effective Worker models and
capability metadata. Its credential-free public result contains the stable
model ID, display name, capability tags, generated Claude Agent name, and
agent-safe route alias. It never returns an API key.

The selector is the only Skill that resolves GrillForge configuration. Consumer
workflow Skills must not parse `~/.grillforge` YAML, duplicate Provider/model
validation, or implement routing/fallback logic.

The coding agent's Main Agent remains responsible for deciding whether to
create a SubAgent and which Worker is suitable. A delegated SubAgent does not
invoke the selector again and does not select its own Provider.

Selection flow:

```text
Task
  -> Coding agent/Grill Skill evaluates capabilities
  -> chooses GrillForge Worker model ID
  -> invokes the generated native Claude Agent by subagent_type
  -> that Agent definition emits its agent-safe model route
  -> local gateway resolves route to Provider + upstream model
  -> cc-switch-derived protocol bridge forwards the request
```

GrillForge does not schedule work or create a parallel custom Agent runtime.

Selector failure semantics:

- Selector not installed: the external integration is unavailable and an
  existing consumer workflow may keep its historical native-only behavior.
- Selector installed with no effective Workers: use native Claude Code
  SubAgent behavior.
- Selector installed but configuration, validation, or routing fails: return
  the error immediately; do not silently fall back to native or another model.
- Capability tags advise selection; they do not grant tools, authorize actions,
  trigger delegation, or enable automatic fallback.

### Claude Code SubAgent Override

`CLAUDE_CODE_SUBAGENT_MODEL` forces one model for all Claude Code SubAgents and
has precedence over per-invocation selection. Therefore:

- It may be used when the user explicitly chooses a single forced Worker.
- It must not be set globally when multiple Worker models need to remain
  selectable.
- Claude Code 2.1.226 accepts arbitrary model IDs in a custom Agent definition,
  but the Agent tool's per-invocation `model` field accepts only the built-in
  Sonnet/Opus/Haiku/Fable choices.
- Multi-model Worker Pool selection therefore generates one native Claude
  Agent definition per effective Worker. Each definition contains that
  Worker's stable GrillForge route alias, and the Main Agent chooses the
  generated `subagent_type`; it does not pass an arbitrary model override.

This behavior must be covered by a Claude Code integration test rather than
assumed from configuration alone.

## Claude Authentication and First-Party Behavior

Mixed Native-main plus external-Worker routing changes only
`ANTHROPIC_BASE_URL`. It must not inject cc-switch's `PROXY_MANAGED`
`ANTHROPIC_AUTH_TOKEN`, because that environment variable has precedence over
the user's Claude subscription OAuth credential.

The gateway handles authentication by route:

- Native Claude route: forward the inbound Claude Authorization without
  persisting or logging it.
- External Worker route: discard the inbound Claude Authorization and inject
  only the selected Provider credential.

Claude Code treats a custom base URL as non-first-party. While Worker routing
is active, Remote Control is unavailable and optimistic Tool Search is disabled
unless a gateway explicitly supports and forwards it. Those features are
outside the MVP and the GUI must state the limitation. Full first-party
behavior returns only after GrillForge is disabled and the original base URL is
restored.

## Route Resolution

Agent-facing route aliases are stable GrillForge identifiers. They must not
contain credentials or depend on a Provider display name.

Conceptual resolution:

```text
request.model
  -> AgentAdapter.decode_route()
  -> ModelRegistry[model_id]
  -> ProviderRegistry[provider_id]
  -> Provider protocol mode
  -> upstream model ID
```

Main and Worker requests are resolved independently:

- A recognized Worker alias routes to that Worker model.
- A managed-main alias routes to the selected main model.
- A non-GrillForge/native model route follows the adapter's native main route.
- Unknown GrillForge aliases fail closed with a clear local error; they never
  fall through to an arbitrary Provider.

## Provider Protocol Logic

Provider protocol mode is explicit and derived from cc-switch-compatible
presets/configuration:

```text
anthropic
openai_responses
openai_chat
```

Request path:

### Anthropic

```text
Anthropic request -> narrow provider normalization -> upstream -> pass-through
```

### OpenAI Responses

```text
Anthropic request
  -> Anthropic-to-Responses conversion
  -> Responses upstream
  -> Responses JSON/SSE-to-Anthropic conversion
```

### OpenAI Chat Compatible

```text
Anthropic request
  -> Anthropic-to-Chat conversion
  -> Chat Completions upstream
  -> Chat JSON/SSE-to-Anthropic conversion
```

Transformations for tools, reasoning/thinking, images, documents, streaming,
usage, model mapping, and Provider-specific normalization are ported with tests
from cc-switch. GrillForge does not create alternative transforms without first
checking upstream.

Connection testing may report that a selected protocol is unsupported and
offer another configured mode. It must not treat 401, 403, quota errors, or
model-not-found errors as evidence that the protocol should be silently
changed.

## Agent Adapter Workflow

The common application workflow is:

```text
1. Detect agent
2. Read current integration status
3. Validate desired main/Worker/provider state
4. Install or update integration assets
5. Snapshot agent-owned configuration
6. Build adapter-specific configuration from neutral desired state
7. Write atomically
8. Verify effective configuration and gateway health
9. Report status
```

Disable/restore workflow:

```text
1. Stop accepting new managed routes
2. Restore the last valid pre-GrillForge snapshot
3. Remove only GrillForge-owned configuration markers
4. Verify native behavior is active
5. Stop the gateway if no enabled adapter needs it
```

Restoration must not delete unrelated user changes. If the current file has
diverged since GrillForge applied it, the adapter must merge only owned fields or
stop with a recoverable conflict instead of overwriting the file wholesale.

## Claude Code CLI Effective Integration Matrix

| Main selection | Effective Workers | Claude Adapter behavior |
|---|---:|---|
| Native/Default | 0 | Restore native configuration; gateway not required |
| Managed model | 0 | Route main model only |
| Native/Default | 1+ | Preserve native main route; route Worker aliases through gateway |
| Managed model | 1+ | Route main and Worker aliases independently through gateway |

The mixed Native/Default-main plus external-Worker case applies to the
standalone Claude Code CLI. Claude account authentication must be forwarded
without being persisted or exposed, while external Provider credentials
replace it only on the external route.

Claude Client is a separate host boundary. Its bundled Code runtime ignores the
standalone CLI network setting because Claude Client injects host-managed
authentication and `ANTHROPIC_BASE_URL`. External Workers are therefore
selectable only while the Client is running GrillForge's 3P profile. That
profile routes conversation, Cowork, and bundled Code through the same gateway;
it cannot safely mix an official subscription main route with a third-party
Worker. In official Client mode the selector must fail before delegation.

## GUI Behavior

### 概览

Shows:

- Detected and connected Agent
- Active main selection
- Enabled Worker count and names
- Gateway/integration health

### Coding Agent 客户端

For MVP, Claude Code and Claude Client are configurable. Claude Client Code and
background development tasks reuse Claude Code's Agent definitions, selector
Skill, Slots, and SubAgent records, but not its network environment. The Client
page owns the separate 3P profile needed to route conversation, Cowork, and
bundled Code. Future Client cards are never presented as working toggles.

### Claude Code Slots and SubAgents

The Slot page exposes exactly the four Claude Code model-family mappings:
Sonnet, Opus, Fable, and Haiku. Each is a single-model Slot and may follow the
native default.

The SubAgent page stores independent named definitions. Each definition owns a
stable ID, one Model reference, zero or more capability tags, and an enabled
state. Users may add any number of definitions, and multiple definitions may
bind the same Model with different capabilities. Applying the Adapter generates
one Claude Agent file per enabled definition; GrillForge still does not execute
or schedule those Agents. A failed mutation or Apply preserves the previous
effective configuration.

### 配置关系

Shows only persisted configuration edges:

```text
Client -> fixed model Slot or named SubAgent -> configured Model
```

No edge represents fallback order, load balancing, retries, or a scheduled
SubAgent invocation.

### 模型资产

Supports add, edit, delete, and connection test using cc-switch-derived fields
and presets. Destructive changes display affected model references before
confirmation.

## Failure and Safety Rules

- Configuration validation happens before any agent-owned file is changed.
- Invalid configuration and runtime state fail immediately; the first error is
  returned to the caller instead of being accumulated behind fallback layers.
- Agent configuration writes are atomic and backed up.
- Provider credentials never appear in Agent configuration, route aliases,
  logs, or error messages.
- A gateway startup failure rolls back the adapter configuration that points to
  it.
- A Provider failure affects only routes assigned to that Provider.
- Provider failures are not silently retried through another Provider.
- Protocol errors do not trigger an automatic protocol downgrade.
- The MVP has no self-healing loop, repair daemon, or multi-generation backup
  system.
- No effective Worker models means native behavior, not an error loop.
- Unsupported future Agent entries remain disabled data; they do not invoke
  placeholder adapters.

# GrillForge MVP Development Plan

Status: Multi-client MVP complete; Universal macOS package and local signature verified

## Release Goal

Deliver a small, polished desktop control plane that configures every coding
client in the pinned cc-switch support list plus Pi through independent
Adapters and one shared Provider and Model Registry.

The MVP is complete only when users can configure Providers, sync models,
choose Provider -> Model per client-native slot, apply multiple clients
independently, and restore each client's exact prior configuration.

## Non-Negotiable Implementation Rules

- Smallest complete implementation; no speculative framework.
- Fail at the first invalid configuration or runtime condition.
- No silent fallback, automatic failover, unbounded retry, or self-healing loop.
- One source of truth for each piece of state.
- One atomic pre-change snapshot for agent configuration recovery.
- Port cc-switch by tested capability slice and retain its MIT notice.
- Client Adapters stay independent; no client-specific behavior enters Core.

## TDD and Connectivity Strategy

Every behavior is delivered as a vertical cycle:

```text
RED: one public behavior test fails
GREEN: smallest implementation makes it pass
REFACTOR: simplify while all tests remain green
```

Test levels:

1. Domain tests call public Core interfaces with no agent-specific knowledge.
2. Filesystem integration tests use temporary real directories and files.
3. Gateway integration tests use a real local HTTP server and recorded/mock
   upstream boundaries for deterministic protocol coverage.
4. Live tests invoke the installed Claude Code executable, the running
   GrillForge gateway, and explicitly configured real Provider endpoints.
5. GUI end-to-end tests exercise user-visible actions against the real Tauri
   command layer where the desktop test environment supports it.

Mocks and recorded fixtures accelerate development but never satisfy the live
connectivity release gate. Live tests are opt-in, fail fast, and read
credentials only from the user's configured secret source. They never run in
default CI or write credentials to snapshots/logs.

Priority public behaviors, in order:

1. Invalid configuration is rejected without changing active state.
2. Native/Default mode leaves Claude Code unchanged.
3. A model alias reaches the correct Provider and returns a valid streamed
   Anthropic response.
4. Main and Worker aliases reach different models without credential leakage.
5. A native Claude Code SubAgent uses the Worker selected through the
   GrillForge Skill.
6. Disabling integration restores the user's previous Claude configuration.

## Milestone 0 — Bootstrap and Risk Proof

Deliverables:

- Initialize Tauri 2, React, TypeScript, and Rust application structure.
- Add formatting, linting, Rust tests, frontend tests, and one build command.
- Add third-party license/attribution for cc-switch-derived code.
- Implement the smallest local Anthropic Messages test gateway.
- Use mock upstreams to prove model aliases can route main and Worker requests
  independently.
- Invoke the installed Claude Code CLI to prove a native custom Agent definition
  accepts a non-Claude GrillForge route alias and emits it on the real SubAgent
  request. Do not infer this from configuration files or documentation.
- Prove Claude Code configuration can be backed up, applied, verified, and
  restored without overwriting unrelated settings.
- Verify how Native/Default Claude authentication behaves when the local
  gateway is active; surface a hard blocker immediately if subscription
  authentication cannot be preserved safely.

Done when:

- The app opens on macOS.
- One automated test routes two model aliases to two different mock upstreams.
- One integration test round-trips a Claude settings file exactly except for
  GrillForge-owned fields.
- One real Claude Code SubAgent request, launched through a generated Agent
  definition, accepts and emits the intended Worker route alias.
- The mixed Native-main/external-Worker path has a verified implementation
  decision.

## Milestone 1 — Core Configuration and Domain

Deliverables:

- Neutral `Provider`, `Model`, `AgentConfiguration`, and route types.
- Boundary validation with typed, user-actionable errors.
- Atomic YAML stores under `~/.grillforge/`:
  - `config.yaml`
  - `models.yaml`
  - `agents.yaml`
- User-only permissions for files containing Provider credentials where the
  platform supports explicit permissions.
- Application services for Provider, Model Registry, and agent state.

Validation includes:

- Stable unique IDs
- Valid endpoint URLs
- Required credentials for the selected auth mode
- Explicit supported protocol mode
- Existing Provider references
- Existing main/Worker model references
- Worker-mode last-enabled-model invariant

Done when:

- Valid configuration round-trips without information loss.
- Invalid configuration returns the first precise error and writes nothing.
- Core tests contain no Claude-specific paths or environment variables.

## Milestone 2 — Provider Management and GUI Foundation

Deliverables:

- Minimal application shell and navigation:
  - Dashboard
  - Agents
  - Main Model
  - Worker Models
  - Providers
- Port the Claude-facing Provider presets and configuration fields required by
  the MVP from the pinned cc-switch commit.
- Provider add, edit, delete, and explicit connection test.
- Support configuration modes:
  - Anthropic Messages
  - OpenAI Responses
  - OpenAI Chat Completions-compatible
  - Local compatible endpoints
- Display errors next to the action that caused them.

Excluded:

- Usage analytics
- Cloud sync
- Automatic endpoint selection
- Provider failover queues
- Partner banners and promotions
- Session management

Done when:

- A user can create, test, edit, and delete an unreferenced Provider.
- Referenced Provider deletion fails with the exact blocking model names.
- The UI always reflects persisted backend state after a failed operation.

## Milestone 3 — Model Registry and Model Screens

Deliverables:

- Model CRUD using Provider references and upstream model IDs.
- Capability tags and display metadata.
- Main Model page with Native/Default and managed model choices.
- Worker Models page with enable switches and Worker mode control.
- Dashboard summary of connected Agent, main selection, and enabled Workers.
- Optional explicit model-list fetch only for Providers whose cc-switch-derived
  configuration supports it; unsupported endpoints fail clearly and allow
  manual model entry.

Done when:

- Main selection and Worker membership are independent.
- Worker mode cannot be enabled with an empty valid pool.
- Disabling Worker mode produces an empty effective pool without deleting the
  saved selections.
- Native/Default remains an adapter state, not a registry model.

## Milestone 4 — Provider Protocol Bridges

Deliverables:

- Port only the cc-switch modules required for:
  - Anthropic pass-through and narrow normalization
  - Anthropic ↔ OpenAI Responses JSON/SSE
  - Anthropic ↔ OpenAI Chat JSON/SSE
  - Authentication header replacement
  - Model mapping
  - Tool calls
  - Reasoning/thinking blocks
  - Image/document inputs required by Claude Code
- Port the relevant upstream tests with attribution.
- Add GrillForge route resolution before protocol conversion.
- Redact credentials and sensitive content from errors and default logs.

Done when:

- Recorded fixtures pass for streaming and non-streaming responses.
- Tool-call round trips preserve IDs, names, arguments, and stop reasons.
- A Provider protocol error is returned directly without trying another
  protocol or Provider.
- Unknown GrillForge model aliases fail closed locally.

## Milestone 5 — Claude Code Adapter and Main Model Switching

Deliverables:

- Claude Code detection and status.
- Atomic configuration snapshot, apply, verification, and restoration.
- Local gateway lifecycle tied to effective managed routes.
- Native/Default main behavior.
- Managed main-model switching.
- Safe handling of application shutdown:
  - Graceful shutdown restores native configuration when required.
  - After an abnormal exit or application replacement, unchanged applied
    configuration rebuilds its in-memory routes on the next launch.
  - A real managed-file difference is reported by safe key or file name and is
    never silently overwritten.
- Mixed Native-main/Worker configuration changes only `ANTHROPIC_BASE_URL`; it
  does not install an auth placeholder that would mask subscription OAuth.
- The GUI clearly reports that a custom base URL disables Claude Remote Control
  and default optimistic Tool Search behavior while Worker routing is active.

Done when:

- Native/Default with no Workers leaves Claude Code behavior unchanged.
- Selecting a managed main model routes only main traffic to that model.
- A failed gateway start or configuration write keeps the previous valid state.
- Disable restores only GrillForge-owned fields and preserves unrelated user
  edits.

## Milestone 6 — Worker Pool and Skill Integration

Deliverables:

- Install/update `~/.claude/skills/grillforge-model-selector/`.
- Generate a credential-free Worker capability view for the Skill.
- Stable agent-facing route aliases for enabled Worker models.
- Generate one GrillForge-owned native Claude Agent definition per effective
  Worker; each definition pins that Worker's route alias in its model field.
- Native Claude Code SubAgent invocation selects the generated
  `subagent_type`, not an arbitrary Agent-tool model override.
- Single forced-Worker mode may use `CLAUDE_CODE_SUBAGENT_MODEL`.
- Multi-Worker mode leaves the global override unset and uses explicit
  generated Agent selection.

Done when:

- With no effective Workers, Claude Code uses native SubAgents.
- One forced Worker is used consistently.
- With multiple Workers, two different native SubAgent calls can reach two
  different configured Providers/models.
- Main and Worker routes remain independent in all four combinations described
  in `LOGIC.md`.

## Milestone 7 — MVP Polish and Release Verification

Deliverables:

- Clean first-run and empty states.
- Actionable error copy without stack traces or secret leakage.
- macOS and Windows path/configuration tests.
- Focused end-to-end acceptance suite.
- Production Tauri builds and concise installation documentation.
- Final dependency and dead-code review.

Acceptance scenarios:

1. Native main + no Workers: no effective GrillForge takeover.
2. Managed main + no Workers: main model switches successfully.
3. Native main + external Workers: Claude stays main; Workers route externally.
4. Managed main + external Workers: routes remain independent.
5. All Workers disabled: immediate return to native SubAgent behavior.
6. Invalid Provider/model/config: first error is shown and nothing is changed.
7. GrillForge disabled: Claude configuration is restored without losing user
   settings.

Live connectivity matrix:

| Route | Required real verification |
|---|---|
| Claude Code -> native Claude | Main prompt succeeds before and after GrillForge disable |
| Claude Code -> gateway -> Anthropic Provider | Streaming prompt and tool call succeed |
| Claude Code -> gateway -> OpenAI Responses Provider | Streaming prompt and tool call succeed |
| Claude Code -> gateway -> OpenAI Chat-compatible Provider | Streaming prompt and tool call succeed |
| Claude main -> Skill -> native SubAgent -> one Worker | Worker result returns to the main conversation |
| Claude main -> Skill -> two different Workers | Each explicit selection reaches the intended Provider/model |
| GUI -> configuration -> Claude Code | Saved selection becomes effective in a newly started Claude session |
| Disable/quit -> restore | Claude resumes native operation and unrelated settings remain intact |

Verified on 2026-08-09 with the pinned cc-switch DeepSeek defaults and a
process-only API key:

- Anthropic Messages: `deepseek-v4-flash` and `deepseek-v4-pro`, including SSE
  and forced tool use.
- OpenAI Responses: text, opaque reasoning content, tools accepted with
  automatic selection, and a complete SSE lifecycle.
- OpenAI Chat-compatible: text, nullable deltas, tools accepted with automatic
  selection, and a complete SSE lifecycle.
- Installed Claude Code 2.1.226: one native main session invoked generated
  Flash and Pro Worker Agents through distinct GrillForge aliases and received
  both tool results.

DeepSeek V4 thinking mode rejects forced tool choice on its OpenAI endpoints;
the live harness does not weaken that constraint silently. Forced tool
verification uses DeepSeek's preferred Anthropic endpoint.

An unsupported or unavailable real Provider is not reported as passing based
on mocks. It is either removed from the advertised MVP support set or reported
as a release blocker.

Release gate:

- All acceptance scenarios pass.
- No MVP screen or module exists without a real user-facing consumer.
- No automatic failover, workflow runtime, MCP server, future Agent Adapter, or
  placeholder plugin system is present.

## v0.5 Client-Centered GUI Addendum

Completed on 2026-08-10:

- Default `zh-CN` GUI with a minimal `zh-CN` / `en-US` i18n boundary.
- Primary navigation changed to Overview, Coding Agent Clients, and Model
  Assets.
- Claude Code is the primary Agent Adapter. Claude Client is added only for its
  independent conversation/Cowork profile; its Code path reuses Claude Code
  configuration instead of duplicating Agent or Worker state.
- Claude Code exposes Main plus cc-switch-derived Sonnet, Opus, Fable, and
  Haiku single-model Slots and one multi-model SubAgent Worker Pool.
- Slot selections reference the global Model Registry and are validated before
  persistence.
- Apply, verification, gateway activation, and exact restoration cover every
  managed Slot route.

The suggested standalone `slots.yaml` is intentionally deferred. Slot state is
kept in `agents.yaml` so the current three-document transaction and validation
boundary remain small and atomic. Claude Client also references that registry;
its four fixed role selections do not justify a fourth configuration file.

## v0.6 Client Configuration Center Addendum

The v0.6 GUI iteration keeps the existing MVP control-plane contract and makes
it visible through five small, real pages:

- Control Center with the current Claude Code integration, main model, Slot,
  Worker, Provider, and Model counts.
- Coding Agent Clients with tested adapters for Claude Code, Claude Client,
  Codex, Pi, Gemini CLI, Grok Build, OpenCode, OpenClaw, Hermes, and Kimi Code.
- Models with registry-backed details, capability labels, and connection test.
- Providers with cc-switch-derived presets and add/edit/delete/test behavior.
- Configuration Relationships with a read-only
  `Client -> Slot/capability view -> Model` representation.

The final Claude Code configuration surface has two tabs. `Claude 模型槽位`
maps Sonnet, Opus, Fable, and Haiku directly. `SubAgent` manages an unbounded
list of independent named definitions; each binds one registry Model and its
own capability tags. Enabled definitions are published by the selector and
materialized as native Claude Agent files. This does not add an Agent Runtime,
workflow engine, or automatic dispatch.

The visual implementation follows the supplied light macOS/Linear reference:
12px cards, 8px controls and spacing rhythm, a light sidebar, restrained indigo
accent, Chinese copy by default, and a small `zh-CN`/`en-US` translation
boundary. Model Market, workflow canvas, runtime route policies, and fake
working adapters remain out of scope.

Claude Client presents no duplicate Main/SubAgent model pool. Its Code and
background development tasks reuse the Claude Code user/project/local
configuration. The Client-specific page only maps Sonnet, Opus, Fable, and
Haiku safe role identifiers for Claude Client conversation/Cowork traffic,
with a separate four-file recovery snapshot and local bearer-protected gateway.

## v0.7 Independent SubAgent and Provider Gallery Addendum

Completed on 2026-08-10:

- Replaced capability-filtered Coding/Review/Testing views with independent
  SubAgent records and explicit Model bindings.
- Added the cc-switch-style Provider preset gallery before the Provider form,
  branded Provider cards after creation, and Provider logos reused from the
  pinned upstream asset set.
- Locked the WebView root to the native window and moved scrolling into the
  workspace so macOS edge overscroll cannot reveal a white strip.
- Kept legacy Worker fields only as a read-compatibility path for existing v1
  configuration; new UI writes the SubAgent records as the single active source.

## Execution Order

Work proceeds strictly in milestone order. A milestone is not considered done
because its UI exists; its listed behavior and tests must pass. If Milestone 0
finds that mixed Native-main/external-Worker authentication cannot be made safe
through the supported Claude Code gateway contract, implementation stops and
the blocker is reported before broader product code is built.

## Post-MVP Follow-Up — Existing Grill Skill

Only after the MVP release gate passes:

- Use the user's untouched `grill-with-workflow.zip` source archive as the
  rollback artifact. It currently contains `SKILL.md` plus CONTEXT,
  ARCHITECTURE, LOGIC, and ADR format templates.
- Update the user's existing `grill-with-workflow` Skill without changing its
  governance model or documentation ownership rules.
- Preserve its workflow behavior and terminology.
- Replace duplicated/static model knowledge with calls to the installed
  `grillforge-model-selector` Skill and its credential-free capability view.
- Only the Main Agent invokes the selector and chooses the Worker. Delegated
  SubAgents retain the existing `grill` + `tdd` allowlist and do not select or
  route models themselves.
- The workflow Skill never parses `~/.grillforge`, reads credentials, validates
  Providers, or implements routing/fallback.
- Keep the integration optional: when GrillForge has no effective Workers, the
  existing Grill workflow continues with native Claude Code behavior.
- If the selector is installed but reports invalid configuration or a routing
  failure, return that error immediately instead of silently using native
  Claude or another Worker.
- Missing selector installation may retain the Skill's historical native-only
  behavior so the existing workflow remains independently usable.
- Capability tags influence only the model choice after delegation has already
  been justified; they must not cause unnecessary SubAgent creation.
- Verify the real chain:

```text
Grill workflow
  -> GrillForge model-selector Skill
  -> effective Worker selection
  -> native Claude Code SubAgent
  -> GrillForge gateway
  -> configured real Provider/model
  -> result returned to the main Grill workflow
```

- Develop the Skill change with the same one-behavior-at-a-time TDD approach
  and retain the original zip as an untouched rollback artifact.

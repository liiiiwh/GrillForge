# GrillForge Logic

Status: Canonical behavior and invariants

## Core State

GrillForge keeps these state concepts separate:

```text
Agent connection
Native model-slot selection
Extension SubAgent library and client bindings
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

Claude Code exposes `main`, `sonnet`, `opus`, `fable`, `haiku`, and
`subagent_default` as single-model Slots. The
cc-switch-derived fixed mappings are:

| Slot | Claude Code key |
| --- | --- |
| Main | `ANTHROPIC_MODEL` |
| Sonnet | `ANTHROPIC_DEFAULT_SONNET_MODEL` |
| Opus | `ANTHROPIC_DEFAULT_OPUS_MODEL` |
| Fable | `ANTHROPIC_DEFAULT_FABLE_MODEL` |
| Haiku | `ANTHROPIC_DEFAULT_HAIKU_MODEL` |
| Native SubAgent default | `CLAUDE_CODE_SUBAGENT_MODEL` |

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

Main-model switching, the native SubAgent-default Slot, and extension SubAgent
bindings are independent. Following native means GrillForge writes no override
for that Slot.

## Extension SubAgent Selection

Extension SubAgents reference Agents already installed in a supported local
Coding Agent runtime. GrillForge stores the source Agent identity, an optional
managed model, and capability hints. It never owns the Agent loop, tools,
prompt, or context.

Each destination client has an independent allowed list. A non-empty list
mounts a client-scoped MCP endpoint; changing the list updates MCP tools
immediately; removing the last binding restores the client's original MCP
configuration.

Execution flow:

```text
Destination client
  -> client-scoped GrillForge MCP
  -> selected local Agent identity
  -> user-installed Coding Agent runtime
  -> native model or explicit GrillForge model route
  -> local gateway resolves route to Provider + upstream model
  -> cc-switch-derived protocol bridge forwards the request
```

GrillForge does not schedule work or create a parallel custom Agent runtime.

### Run Lifecycle

`run_agent` starts the run and returns a handle, so the delegating Agent keeps
its turn while the child works and can start every branch of a fan-out before
collecting any of them. `get_agent_result` reports the run as running, awaiting
permission, or completed. `stop_agent` cancels a run and its child. Passing
`waitSeconds` to `run_agent` collects the result in the same call for a caller
that only wants the answer.

A collect waits by default, because collecting is the caller's next move after
starting a run: answering at once would report a run that has barely begun as
running to a caller that then treats its turn as finished. The default interval
has to hold for every mounted client, so it stays under the shortest tool-call
budget any of them applies rather than the longest one any of them allows. A
caller that knows its own client passes `waitSeconds`, and `0` looks without
waiting.

A wait also ends the moment the child asks for permission, because that request
is the caller's to answer and the child is blocked until it is.

Every tool result names the runs this client started and has not collected. A
caller only loses a result by believing it has none left, and the place it
decides that is the result of whatever it did next.

### Kept-open Runs

A run is one-shot unless the caller asks to keep it open, because an open
conversation is state the runtime stores on the user's disk. A kept-open run is
opened under an identifier GrillForge chooses and reopened under the same one for
each further turn, so the Agent keeps what it read and decided. `stop_agent`
closes it, and a client that is unmounted leaves none open.

GrillForge holds the identifier and nothing else. The runtime stores the
conversation and reopens it, exactly as it does for a person running the same CLI
by hand, which is why a runtime appears in that table only once its own CLI has
been seen to accept a caller-chosen identifier and reopen it. A runtime that
cannot says so when the run is requested rather than running a one-shot the
caller believes it can continue.

One conversation runs one turn at a time: the previous turn is collected before
the next is sent.

A result is delivered once and then dropped. An uncollected result is dropped
after an hour, and unmounting a client cancels every run still active for it.

Only the caller can collect a result. MCP answers a tool call and nothing more:
there is no way to deliver a result into a turn that has already ended, so a
delegating Agent that reports a runId and stops has thrown the work away. The
tools therefore state the obligation, and every unfinished payload repeats it.

For the same reason a client may mount every broker tool or none of them. A
client allowed to call `run_agent` but not `get_agent_result` starts runs it can
never collect, which is silent loss rather than a failure it could report.

### Permission Requests

A permission prompt raised by a delegated Agent belongs to the Agent that
delegated it. `get_agent_result` reports it as `awaiting_permission` with the
tool name and its input, and `answer_agent_permission` returns allow or deny to
the waiting child.

GrillForge decides nothing here. An unanswered prompt is denied on a timeout,
because a child left waiting is worse than a refused tool call. Only a runtime
whose CLI exposes a prompt hook can raise one; the rest are bound by the
permission mode chosen when they launched.

### Permission Modes

Each delegated run uses a permission mode its own client accepts. A call may name
one from that client's published list; otherwise the client's default applies,
chosen so a delegated Agent is as capable as the same Agent run by hand. A mode
the client does not accept fails before the Agent launches.

A delegated Agent reaches the network by default. Withholding it is a deliberate
request, and it fails on a runtime with no switch that can honour it rather than
silently granting what was refused.

### DeepSeek Harness Layer Ownership

The harness composes plugin layers beneath one user layer per profile, so a file
GrillForge writes there is shared with whatever the user put in it. GrillForge
owns exactly one marked block in that layer and rewrites only that block; every
entry around it survives Apply and Disable, and Disable restores the file it
found. The credential is a reference the harness resolves from its own
credential file, so no secret enters the layer.

The harness reads a second user layer at `$DSH_HOME/cordis.patch.yml`, after the
profile one. That is where its MCP entry lives, so each file keeps a single
owner: the adapter writes the model route into the profile layer, and the shared
mount manager writes the MCP entry into the home layer. Extension SubAgents
therefore mount, unmount, and reconcile for the harness exactly as for every
other client.

### Native Route Hook Scope

Claude Code and Claude Client share one user settings file, so the hook installed
there answers for the client whose session invoked it, identified by that
session's entrypoint. One client's mounted extensions never decide the other's
native Workflow and Agent tools, and the denial names the MCP server that
session actually has.

A GrillForge-launched child is a leaf: it may not open another SubAgent level,
because one invocation would otherwise fan out into an unbounded tree of
runtimes. The rule applies only to the delegating tools; the child keeps every
other tool it was launched with.

MCP configuration, source Agent, runtime, model, Provider, or authentication
errors fail immediately. Capability tags are hints, not tool permissions.

### Claude Code SubAgent Override

`CLAUDE_CODE_SUBAGENT_MODEL` forces one model for all Claude Code SubAgents and
has precedence over per-invocation selection. Therefore:

- It is exposed as the explicit `subagent_default` model slot.
- Following native removes GrillForge's override.
- Claude Code 2.1.226 accepts arbitrary model IDs in a custom Agent definition,
  but the Agent tool's per-invocation `model` field accepts only the built-in
  Sonnet/Opus/Haiku/Fable choices.
- Multiple selectable extension Agents use the client-scoped MCP broker rather
  than generated Claude Agent definitions.

This behavior must be covered by a Claude Code integration test rather than
assumed from configuration alone.

## Extension Runtime Authentication

The destination client's normal model route is independent from an extension
SubAgent invocation. The MCP broker creates a short-lived, route-scoped local
credential for a managed extension model and passes it only to the child
runtime. It removes inherited Claude API/OAuth variables before launching that
child. A native extension model receives no GrillForge model override and uses
the source runtime's own local configuration.

The long-lived client MCP token never doubles as a model-gateway token. Tokens,
Provider credentials, prompts, and tool results are not logged or persisted by
the broker.

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

Native Slots and managed extension requests are resolved independently:

- A recognized extension route alias routes to that extension's configured model.
- A managed Slot alias routes to that Slot's selected model.
- A non-GrillForge/native model route follows the adapter's native main route.
- Unknown GrillForge aliases fail closed with a clear local error; they never
  fall through to an arbitrary Provider.

## Bridge Request Acceptance

A bridge receives requests written for one API and sends them to another, so
what it accepts is a contract with real clients, not a matter of taste. Three
rules decide every field:

- A field the target can express is accepted and forwarded.
- A field that exists only for the source API and is dropped on the way out is
  accepted in any form its own specification allows.
- A field that would change the meaning of the request or the shape of the
  response, and that the target cannot express, is rejected by name.

The third rule is why a request naming `stop_sequences` fails on a Responses
target rather than losing it silently, and the second is why a thinking hint or
a context-management edit is accepted in every documented form even though the
bridge drops it. Rejecting a field the specification allows is a defect, not
strictness: the client is not wrong, and the error arrives only once a delegated
Agent is already running.

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
3. Validate desired Slot/extension/Provider state
4. Install an explicitly approved required client extension, if any
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

## Client-scoped MCP Lifecycle

- Zero enabled extension bindings means no GrillForge MCP entry in that client.
- Enabling the first binding snapshots the relevant client file, mounts
  `/mcp/{client_id}`, and activates the matching broker route list.
- Changing a non-empty binding set updates broker authorization immediately;
  the MCP tool names remain stable (`list_agents` and `run_agent`).
- Removing the final binding deactivates that client's broker and restores the
  exact pre-mount file.
- Startup reconciles persisted desired bindings. Normal exit restores mounted
  client files without deleting desired bindings.
- A mount or reconciliation failure rolls the binding mutation back.

Pi requires community package `pi-mcp-extension`. Missing support is reported
before a Pi binding is saved. One-click installation is explicit, pinned, and
verified by rereading Pi's package registration.

## GUI Behavior

### 概览

Shows:

- Detected and connected Agent
- Active main selection
- Enabled extension SubAgent count and names
- Gateway/integration health

### Coding Agent 客户端

Each client page renders only its verified native model Slots. Clients with a
verified MCP configuration format also render their own extension SubAgent
bindings. Unsupported combinations are not shown as working controls.

### Claude Code Slots and Extension SubAgents

The Slot page exposes Main, Sonnet, Opus, Fable, Haiku, and native SubAgent
default. Each is a single-model Slot and may follow the native default.

Extension definitions live in one global library. A definition references a
discovered source Agent, optional Model, and capability hints. The Claude Code
page only controls which definitions that destination client may use; it does
not generate replacement Claude Agent files.

### 配置关系

Shows only persisted configuration edges:

```text
Client -> native model Slot -> configured Model
Client -> extension binding -> source Agent -> optional configured Model
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
- No extension bindings means native behavior, not an error loop.
- Unsupported future Agent entries remain disabled data; they do not invoke
  placeholder adapters.

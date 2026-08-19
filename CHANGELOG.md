# Changelog

All notable changes to GrillForge are documented in this file.

The project follows [Semantic Versioning](https://semver.org/).

## [0.2.21] - 2026-08-18

### Changed

- Pi carries its own mark instead of the GrillForge logo, and DeepSeek Harness
  carries the DeepSeek mark, so every client in the list is told apart by its own
  brand.

## [0.2.20] - 2026-08-18

### Added

- DeepSeek Harness is a selectable client. It is detected from its installed CLI,
  its model pool is routed through the local gateway's Chat Completions ingress,
  and Apply, Disable, drift reporting, and startup recovery behave as they do for
  every other client.

### Fixed

- The harness layer keeps its `[]` placeholder handling to the placeholder line
  itself, so an inline empty list a user wrote elsewhere in that file survives.
- A child that cannot be given its permission relay now fails with that reason
  instead of launching silently without one.

### Verification

- A real `dsh` 0.1.0-rc.7 composed the layer GrillForge writes on a first Apply
  over the harness default, reporting the GrillForge model route without error,
  and the same installation was detected by version through the adapter.
- The gateway route the harness uses is covered by a test that its own token
  opens it, another client's token does not, and a model outside its pool is
  refused.
- Full Rust tests, strict Clippy, and frontend tests pass.

## [0.2.19] - 2026-08-18

### Added

- A delegated Agent no longer holds the caller's turn. `run_agent` returns a
  `runId` immediately; `get_agent_result` collects it, waiting only as long as it
  is asked to, and `stop_agent` cancels a run and its child process. Passing
  `waitSeconds` to `run_agent` keeps the one-call shape for a caller that only
  wants the answer.
- A permission prompt raised by a delegated Agent is relayed to the Agent that
  delegated it. `get_agent_result` reports `awaiting_permission` with the tool
  and its input, and `answer_agent_permission` returns allow or deny to the
  waiting child. GrillForge carries the question and the answer; it never
  decides, and an unanswered prompt is denied after ten minutes rather than
  leaving the child stuck.
- Every client now publishes the permission modes its CLI actually accepts, read
  from a real installation of each. `list_agents` reports `permissionModes` and
  `defaultPermissionMode`, and `run_agent` accepts `permissionMode`. A mode the
  client does not accept fails before the Agent is launched, naming what is
  available.

### Changed

- A delegated Agent reaches the network by default. Only a deliberate
  `webAccess: false` can fail, and only on a runtime with no switch to honour it.
- A delegated Claude Code Agent starts in its `auto` permission mode, so it can
  edit and run commands like the same Agent run by hand. Without a mode a
  headless child cannot answer a prompt and could only read.

### Fixed

- Claude Code and Claude Client share one user settings file, so the route hook
  they both run now answers for the client whose session invoked it. Unmounting
  one client's extensions no longer leaves the other unable to use its native
  Workflow, and the denial names that client's own MCP server rather than one its
  session does not have.

- A DeepSeek Harness adapter. The harness composes plugin layers under one user
  layer, so GrillForge owns a single marked block in
  `$DSH_HOME/profiles/headless/cordis.patch.yml`: the model route it declares as
  an OpenAI-completions provider against the local gateway, and the MCP server
  carrying its extension SubAgents. The credential is a reference resolved from
  `$DSH_HOME/.env`, so no secret enters the patch layer, and every entry the user
  wrote around the block survives Apply and Disable.

### Verification

- The generated DeepSeek Harness layer was composed by a real `dsh` 0.1.0-rc.7
  installation, which reported the GrillForge model route and MCP server without
  error. Two defects surfaced that way and were fixed: a plugin name beginning
  with `@` needs quoting because `@` is a reserved YAML indicator, and a plugin
  the base profile does not carry must be added through an `insert` list rather
  than an id-targeted patch.
- The permission relay was proven against the installed Claude Code CLI before
  being built on, and again in a test where the child raises a prompt through the
  very configuration GrillForge hands it.
- Permission modes were read from real installations of Claude Code, Codex,
  Gemini CLI, Kimi Code, OpenCode, Hermes, Grok Build, and Pi.
- Full Rust tests, strict Clippy, and frontend tests pass.

## [0.2.18] - 2026-08-18

### Added

- The Model Registry records a model's context window and maximum output tokens,
  the capability metadata the architecture always assigned to it. Provider
  synchronization fills the window from the model list when the provider
  publishes one, and the model asset page accepts it for the providers that do
  not. An empty value keeps the model unknown rather than inventing a number.

### Fixed

- An extension SubAgent no longer fails with `Prompt is too long` on a model
  whose window differs from the client's assumption. Claude Code assumes 200000
  tokens for a model it does not recognize, so a 262144-token model was cut off
  well below its real limit. A managed child now receives the recorded window
  through `CLAUDE_CODE_MAX_CONTEXT_TOKENS`.
- Pi, its extension children, and Grok Build read the same recorded window
  instead of each carrying its own constant. Their previous constants remain
  only as the fallback for a model whose window is still unknown, because those
  clients require the field.
- A re-synchronization no longer discards a context window entered by hand; it
  only fills a gap.

### Changed

- The managed route a child runtime receives is a named record rather than a
  three-element tuple, which keeps the runtime signatures within their argument
  budget while carrying the window.

### Verification

- The override was confirmed against the installed Claude Code CLI before being
  built on: an unset window reports 200000 and a set one reports the exact value.
- Full Rust tests, strict Clippy, and frontend tests pass. A timing budget in the
  parallel-extension test was widened after added load made a correct run fail
  intermittently; the overlap it asserts is still enforced by the child barrier.

## [0.2.17] - 2026-08-18

### Changed

- An extension SubAgent is now a leaf worker. A GrillForge-launched Claude Code
  child may no longer open another SubAgent level, because one invocation would
  otherwise be able to fan out into an unbounded tree of runtimes. This replaces
  the 0.2.15 behavior, which allowed the child its native Agent and Workflow
  tools.
- The denial a child receives now names the actual constraint. It previously
  pointed at the GrillForge broker, which is never mounted into a child, leaving
  the Agent with no reachable alternative.

### Verification

- The leaf rule shares the hook's tool-name gate, so a child keeps Bash, Read,
  Edit, and the rest; a regression test pins that and the fact that a child stays
  a leaf even when nothing is mounted for its parent client.
- The tool list a real child receives was captured from the running Claude Code
  CLI and contains no MCP tools, confirming that the native Agent and Workflow
  tools were the only route to a second level.
- Full Rust tests, strict Clippy, and frontend tests pass.

## [0.2.16] - 2026-08-17

### Fixed

- A failed tool result now survives the Chat bridge. `tool_result.is_error` is a
  standard Anthropic field that appears only when a tool call fails, so a
  multi-turn extension SubAgent run died with a 502 the first time any of its
  tools errored. The Responses bridge already carried the field; the Chat bridge
  now marks the failure in the tool message text, because an OpenAI Chat tool
  message has no error flag of its own.
- The marker both bridges use for a failed tool result is now a single shared
  constant instead of a repeated literal.

### Verification

- The full Chat bridge field allowlist was re-checked against a request envelope
  captured from the current Claude Code, covering the top-level fields, system,
  text, thinking, tool_use, and tool_result blocks.
- Full Rust tests, strict Clippy, and frontend tests pass.

## [0.2.15] - 2026-08-17

### Fixed

- Extension SubAgent calls that route a bridged model now complete. The bridge
  required Claude Code to send `thinking.display`, but the current client sends
  adaptive thinking without that field, so every Anthropic-to-Chat and
  Anthropic-to-Responses request failed with a 502 until the client gave up and
  exited. An absent `display` now means the omitted thinking it already implies,
  while an explicit unsupported value is still rejected.
- A failed Agent runtime now reports the cause its own event stream wrote to
  stdout. Every supported runtime is launched with a machine-readable stream and
  leaves stderr empty, so the previous message ended at an empty colon and
  discarded the real error.
- A GrillForge-launched Claude child runtime keeps its native Agent and Workflow
  tools. The child inherits the route hook but has no broker mounted into it, so
  denying its native tools left it unable to delegate at all.

### Verification

- The real installed Claude Code CLI completed both a streaming turn and a real
  Read-tool loop against the live Kimi Coding API through the Chat bridge; both
  failed before this release.
- Full Rust tests, strict Clippy, and frontend tests pass.

## [0.2.14] - 2026-08-17

### Fixed

- Pi extension SubAgent calls now use a three-hour request boundary instead of
  the upstream extension's 30-second default, while unmount restores the
  user's previous timeout exactly.
- Pi maps standard MCP progress notifications to its native expandable tool
  card without adding intermediate Agent output to the main conversation.
- Claude Code, Claude Client, and Codex receive the same standard MCP progress
  stream when their current host version exposes it; the final Agent result is
  still returned exactly once.
- Parallel extension Agent runs keep their progress isolated and continue to
  use the installed client CLI for each Agent loop.

### Verification

- A real installed Pi 0.84.1 and `pi-mcp-extension` 1.5.0 completed a
  loopback-only MCP tool call that ran for more than 31 seconds.
- Full Rust tests, strict Clippy, frontend tests, and the production build pass.

## [0.2.13] - 2026-08-17

### Fixed

- Claude Code and Claude Client UltraCode now route native Workflow and Agent
  attempts through mounted GrillForge extension SubAgents whenever the client
  has an enabled extension binding.
- The route guard uses Claude Code's official `PreToolUse` hook and is installed
  and removed together with the Claude Code extension mount. Unrelated user
  hooks and settings are preserved.
- Failed unmounts now restore the MCP entry, route hook, credential, and both
  snapshots as one transaction.

### Verification

- Full Rust tests and strict Clippy pass with all features and targets.
- Frontend tests, the production build, and the protocol routing matrix pass.

## [0.2.12] - 2026-08-16

### Fixed

- Unmounting the Codex extension now removes its managed broker credential, so
  a restarted Codex session can no longer retain authorization to an unmounted
  GrillForge MCP server.
- Claude Client and Codex now offer the same restart confirmation after an
  extension mount changes, allowing their cached MCP tool catalogs to refresh
  immediately.

### Verification

- Full Rust tests and strict Clippy pass with all features and targets.
- Frontend tests and the production build pass.

## [0.2.11] - 2026-08-14

### Fixed

- A model that natively supports any one of Anthropic Messages, OpenAI
  Responses, OpenAI Chat Completions, or Gemini Native can now serve all four
  client ingress protocols. Matching protocols remain direct; mismatches use
  the cc-switch-derived bridge.
- Text, streaming text, function calls, tool-result continuation, custom tools,
  Codex tool search, dynamically loaded namespace tools, reasoning dialects,
  and provider-specific Responses fields retain their protocol state across
  bridged turns.
- Stateless Codex tool-result turns restore the previous response history, and
  Gemini thought signatures are replayed only to the matching conversation.

### Verification

- The complete 4 x 4 ingress/native protocol matrix passed both non-streaming
  text/tool-result tests and streaming text/tool tests.
- Real DeepSeek V4 Flash and V4 Pro synchronization and connection tests passed.
- Every model returned by the configured Kimi account passed live protocol
  synchronization; the installed Claude Code CLI then completed both a real
  Kimi completion and a real `Read` tool loop through GrillForge.
- Full Rust tests, strict Clippy, frontend tests, and the production build pass.

## [0.2.10] - 2026-08-14

### Fixed

- Claude Code and Claude Client extension SubAgents can now use Chat providers
  such as Kimi that return `reasoning_content` but do not accept the separate
  OpenAI `reasoning_effort` request parameter.
- The Chat bridge preserves Claude thinking semantics without sending an
  unsupported upstream field, matching the cc-switch routing boundary.

### Verification

- The installed Claude Code CLI completed a real streamed extension SubAgent
  request through GrillForge and Kimi `kimi-for-coding-highspeed` in under two
  seconds.
- Full Rust tests, strict Clippy, frontend tests, and the production build pass.

## [0.2.9] - 2026-08-14

### Fixed

- Claude Code, Claude Client, Codex, Pi, and the other managed clients now
  accept a selected model only after its Provider synchronization has recorded
  at least one verified protocol route.
- Extension SubAgents validate the selected model route when they are created
  or edited. At runtime GrillForge forwards matching protocols directly and
  bridges mismatched client protocols through the verified Provider endpoint.
- Kimi Responses reasoning accepts absent, empty, or null encrypted content
  while preserving non-empty opaque signatures and rejecting invalid types.
- The global Extension SubAgent library no longer presents per-client bindings
  as a source compatibility restriction. Each supported client can authorize
  any extension independently.

### Verification

- Every model discovered from the configured Kimi For Coding account passed a
  fresh live synchronization and Gateway connection test.
- The installed Claude Code CLI completed a real Agent tool loop through an
  OpenAI Chat-only upstream via the GrillForge Anthropic bridge.
- Full Rust tests, strict Clippy, frontend tests, and the production build pass.

## [0.2.8] - 2026-08-14

### Fixed

- Kimi model synchronization now observes `reasoning_content` in real Chat
  Completions responses and persists the required protocol capability before
  routing or connection testing existing and newly discovered models.
- Pi MCP extension installation now exposes the selected Pi runtime's sibling
  Node executable to `env` shebangs without changing the application's global
  environment.
- Deleting a Provider atomically removes its unreferenced models. Models still
  selected by a client or Extension SubAgent continue to block deletion with
  the exact reference in the error.
- Pi installation errors use the available dialog width instead of collapsing
  into a narrow vertical column.

### Verification

- Every model discovered from the configured Kimi For Coding account passed a
  live synchronization and Gateway connection test.
- DeepSeek V4 Flash and V4 Pro passed live protocol synchronization and
  Gateway connection tests.
- Full Rust tests, strict Clippy, frontend tests, and the production build pass.

## [0.2.7] - 2026-08-13

### Changed

- Adding a Provider now discovers its models, checks each model against the
  Provider's supported API surfaces, and saves the verified result atomically.
- Model discovery uses cc-switch's bounded endpoint candidates. Presets such
  as Kimi For Coding that do not expose a model-list endpoint validate their
  pinned model IDs directly instead of failing on `/v1/models`.
- A second credential profile for the same Provider receives deterministic,
  Provider-scoped model routes instead of colliding with existing model IDs.
- Extension SubAgent capability labels preserve user casing while duplicate
  labels are rejected case-insensitively.
- The macOS menu-bar control is now a compact native hierarchical menu instead
  of a large webview, with client, slot, Provider, model, and Extension Agent
  submenus.

### Verification

- Added regressions for Kimi preset discovery fallback, atomic Provider
  creation, duplicate upstream models across credential profiles, and
  case-preserving capability labels.
- Full Rust tests, strict Clippy, frontend tests, and production build pass.

## [0.2.6] - 2026-08-13

### Added

- A compact macOS menu-bar panel lists every installed client and exposes each
  real model slot as linked Provider and model selectors.
- The same panel can apply client configuration, mount or unmount Extension
  SubAgents, and enable each authorized Extension Agent.

### Changed

- Extension mount actions use concise user-facing wording instead of exposing
  the MCP transport implementation.
- MCP instructions now treat workflow and parallel requests as GrillForge
  routes, not as implicit permission to bypass Extension Agents with the
  client's native Workflow or SubAgent tools.

### Verification

- Added frontend regressions for linked Provider/model selectors and Extension
  Agent switches.
- Full Rust tests, strict Clippy, frontend tests, and production build pass.

## [0.2.5] - 2026-08-13

### Changed

- The sidebar now summarizes the number of clients with GrillForge MCP mounted
  instead of showing one Claude Code integration state.
- Quick actions now open the extension SubAgent creation flow directly.

### Verification

- Added frontend regressions for the MCP mount summary and extension SubAgent
  quick action; all frontend tests and the production build pass.

## [0.2.4] - 2026-08-13

### Added

- `run_agent` now emits standard MCP progress notifications during long native
  Agent calls while keeping prompts, tool output, and intermediate Agent text
  out of the primary conversation. The final result is still returned once.
- Claude Client's stdio MCP bridge relays streaming progress notifications and
  retains the three-hour runtime boundary.

### Changed

- The control-center client card now lists every supported client in a compact
  scrollable region and restores the original robot illustration.
- HTTP MCP clients share the same progress implementation, so clients that
  support MCP Progress gain status updates without client-specific state or
  polling APIs.

### Verification

- Added public HTTP/SSE and stdio bridge regressions for progress delivery,
  final-result uniqueness, prompt isolation, and JSON-only compatibility.
- Full Rust tests, strict Clippy, frontend tests, and production build pass.

## [0.2.3] - 2026-08-13

### Changed

- Extension Agent execution uses one synchronous `run_agent` call that returns
  only the native Agent's final result. Claude Code and Claude Client use stdio
  mounts to avoid the one-minute HTTP first-byte timer; each runtime may run for
  up to three hours, and workflows may invoke independent calls concurrently.
- Gemini CLI and Grok Build now expose only Agents reported by their installed
  native CLIs and execute the selected Agent with isolated, process-local model
  routing. Hermes remains a model-configured client rather than an extension
  Agent source because its profile runtime cannot be isolated without copying
  user state.

### Fixed

- Codex-sourced extensions with external models now run through the installed
  Codex CLI and GrillForge Responses gateway instead of Codex's native
  `spawn_agent` model allowlist, which rejected `grillforge/*` before any API
  request was sent.

### Verification

- The ChatGPT-bundled Codex CLI completed a loopback-only external-model task
  through GrillForge and sent the configured upstream model to a local
  Responses service without contacting a paid provider.
- Official Gemini CLI 0.55.1 and Grok Build 1.0.3 completed exact-Agent,
  loopback-only managed-model tasks without changing user configuration.

## [0.2.1] - 2026-08-13

### Added

- Provider synchronization now probes every discovered model through
  Anthropic Messages, OpenAI Responses, OpenAI Chat, and Gemini Native, then
  stores the verified supported and unsupported protocols atomically.
- Gemini protocol bridging and client ingress now participate in the same
  local routing path as Anthropic and OpenAI protocols.
- Extension SubAgent discovery and exact native-runtime execution now cover
  the verified Claude Code, Codex, Pi, Kimi Code, Gemini CLI, and OpenCode
  Agent sources.

### Changed

- The gateway connects directly when a model supports the client protocol and
  otherwise bridges to a protocol verified for that exact Provider/model pair.
- Claude and Codex native model catalogs are read from the installed clients;
  optional client failures remain local to their own cards and Agent sources.
- Kimi Code uses its current `.kimi-code` configuration and Agent layout.
- OpenCode exposes actual SubAgents instead of primary session Agents.
- README now focuses on native cross-client Agent reuse and task-specific
  model routing without release-specific download names.

### Fixed

- DeepSeek V4 Pro no longer uses an unsupported Responses route; bounded live
  probes and connection tests pass for both V4 Pro and V4 Flash.
- Valid incomplete Responses results are returned as partial max-token results
  instead of misleading HTTP 502 errors.
- MCP instructions prioritize GrillForge extension SubAgents for explicit
  delegation, with scoped native web access only when requested.

### Verification

- Full Rust tests, all-target Clippy with warnings denied, frontend tests, and
  the production frontend build pass.
- Installed Claude Code, ChatGPT-bundled Codex, and Pi CLIs passed native
  catalog, configuration, discovery, authentication, MCP, and tool-loop tests.

## [0.2.0] - 2026-08-12

### Added

- A client-scoped MCP broker exposes global extension SubAgents to each bound
  Coding Agent without implementing an Agent Runtime or tool loop.
- Extension SubAgents can use discovered Claude Code or Codex Agents as their
  local runtime source; managed routes preserve the source Agent instructions.
- Pi can detect and install the reviewed `pi-mcp-extension` package with one
  click before mounting its MCP configuration.
- Each client can mount or unmount MCP independently. Extension bindings update
  a mounted MCP tool list immediately without controlling its lifecycle, and
  the saved mount choice is restored automatically when GrillForge starts.
- Claude Client can use its client-scoped MCP in both 1P and 3P inference
  modes; model routing and extension bindings remain independent.

### Changed

- Removed the selector Skill, generated Claude Worker definitions, Worker-mode
  configuration, and their compatibility paths. Configuration format v2 is the
  only supported schema.
- MCP tool descriptions are the complete usage contract; business workflow
  Skills no longer contain GrillForge-specific instructions.
- Claude Code restore snapshots now preserve the original settings file bytes.
- Client model configuration and extension MCP state are separate. Model
  Apply/Disable suspends and remounts MCP transactionally when both layers
  share one client configuration file.
- Claude Code now reads and writes the actual native default, model-family,
  and native SubAgent-default slots while preserving a native-only route.
- Claude Client exposes the shared Claude Code native SubAgent-default slot for
  its built-in Code environment, while its MCP bindings remain independent.
- MCP initialize instructions and always-load metadata make the extension
  Agent list the default delegation path for compatible clients.
- Extension IDs are generated internally; the UI shows the source client and
  source Agent instead of asking users to maintain a slug.
- MCP unmount removes only GrillForge's own server entry and preserves model,
  theme, and other MCP edits made while mounted.

### Verification

- A real installed Claude Code CLI executed its own local Agent and Read-tool
  loop through the authenticated MCP broker using loopback-only dummy services.
- Real Pi and ChatGPT-bundled Codex CLI discovery passed on macOS.
- The reviewed Pi MCP extension was installed successfully with a real Pi CLI
  in an isolated home directory, and its pending, failure, retry, and timeout
  paths are covered by tests.

## [0.1.6] - 2026-08-11

### Fixed

- Claude Client Code now rejects an external GrillForge Worker before
  delegation while the Client is still using its official 1P route, instead of
  sending the `grillforge/*` alias to Anthropic and reporting a misleading
  model-access error.
- The selector validates that Claude Client Code is running an active
  GrillForge 3P profile before returning external Workers.
- The Claude Client page now describes the real boundary: it shares Claude Code
  SubAgent definitions, but its host-managed network route must be applied
  separately and then loaded by restarting Claude Client.
- Applying a Claude Client profile with external Workers now fails fast unless
  their Claude Code Agent definitions have already been applied.

### Verification

- Added a loopback-only end-to-end test using Claude Client's bundled Code
  runtime. It verifies `Client main -> named GrillForge Worker -> Client main`
  without reading account credentials or contacting Anthropic.

## [0.1.5] - 2026-08-11

### Fixed

- An unavailable or slow login shell no longer makes an uninstalled optional
  Coding Agent client fail the entire GrillForge startup.
- Optional shell discovery now reports an absent CLI as not installed while
  preserving actionable errors for real executable candidates that fail their
  version check.
- The behavior applies consistently to every client that uses login-shell CLI
  discovery.

## [0.1.4] - 2026-08-11

### Fixed

- When the native Claude model candidate is disabled and exactly one external
  SubAgent is enabled, automatic Claude delegation now uses that external
  model without requiring an explicit selector Skill invocation.
- The forced default remains available as a named GrillForge Agent, so explicit
  selector-driven delegation and automatic delegation use the same route.
- The automatic standalone Claude Code path is covered by an isolated
  end-to-end test using a loopback-only API server.

### Changed

- The Claude native SubAgent switch now describes its actual role as a native
  model candidate; disabling it does not claim to remove Claude's built-in
  Agent runtime.

## [0.1.3] - 2026-08-11

### Fixed

- Client discovery now checks every matching executable until one successfully
  returns a version, instead of stopping when an earlier stale installation
  fails.
- Interactive login-shell discovery now finds dynamic fnm and similar
  version-manager sessions without hard-coding a Node.js version or session
  directory.
- Repeated paths returned by PATH, standard locations, and the login shell are
  inspected only once.

### Changed

- Applied the same multi-install discovery behavior to Claude Code, Codex, Pi,
  Kimi Code, Gemini CLI, Grok Build, OpenCode, and Hermes.

## [0.1.2] - 2026-08-11

### Fixed

- Claude Client Code and Claude Code SubAgents no longer fail when the client
  sends a legal empty `tools` list to a bridged model route.
- OpenAI Responses, OpenAI Chat, Gemini Native, and Codex-to-Anthropic bridges
  now omit empty tool arrays while still rejecting a dangling tool choice.

### Changed

- Removed superseded requirement drafts, completed MVP planning documents,
  unused Vite/Tauri template assets, and obsolete conditional-compilation lint
  suppressions.
- Updated canonical architecture documentation to match the implemented client
  adapters, protocol bridges, navigation, and notarized macOS release.

## [0.1.1] - 2026-08-11

### Fixed

- Codex now treats its current native configuration as the default route, shows
  the actual configured model, and allows switching among the CLI's available
  native models without presenting native authentication as a Provider.
- Configuring only a Codex SubAgent preserves the current native main model and
  Provider instead of requiring a separate GrillForge main-model selection.

### Documentation

- Clarified that one Coding Agent can map different supported slots to
  different Providers and models while preserving client-native constraints.
- Removed the manually maintained Contributors list from the README files.

## [0.1.0] - 2026-08-11

### Added

- Client-centric Chinese GUI for configuring coding-agent model routes.
- Shared Provider and Model Registry with presets and model discovery.
- 151 protocol-specific Provider presets generated from a fixed cc-switch
  revision, including per-client direct/bridged compatibility metadata.
- Anthropic Messages, OpenAI Responses, OpenAI Chat Compatible, and Gemini
  Native protocol support.
- Adapters for Claude Code, Claude Client, Codex, Pi, Kimi Code, Gemini CLI,
  Grok Build, OpenCode, and Hermes.
- Claude Code model-family mapping and named SubAgent model routing.
- Codex main model, default SubAgent model, and custom Agent model mapping.
- Local authenticated model gateway with streaming and tool-call translation.
- Atomic client configuration, one recovery snapshot, drift reporting, and
  exact disable-time restoration.
- Background client discovery refresh when the Clients page is opened.
- Vetted real-time balance and Coding Plan queries without a local usage
  database.
- Background re-application of persistently enabled clients on launch and
  exact pre-takeover restoration on normal quit.
- Bilingual documentation with Simplified Chinese as the default.
- Universal macOS release signed with Developer ID Application and accepted by
  Apple Notary with a stapled Gatekeeper ticket.

### Fixed

- Pi now authenticates with the real `x-api-key` header used by its Anthropic
  Messages runtime; Pi authorization errors no longer mention Claude Desktop.
- Pi configuration reformatting no longer creates false drift after restart.
- Codex model-catalog timeouts are isolated to the Codex model picker and no
  longer prevent GrillForge from loading.
- Client status badges no longer stretch vertically in long configuration
  pages.
- Restarting GrillForge restores active in-memory routes without rewriting
  unchanged client configuration.
- Client discovery skips stale executable candidates and checks standard
  package-manager, login-shell, and bundled-app locations without assuming a
  specific Node.js version directory.
- Claude Code and Claude Client Code now resolve installed client executables
  even when the GUI application is not on the login-shell PATH.

[0.1.6]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.6
[0.2.1]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.2.1
[0.2.0]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.2.0
[0.1.5]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.5
[0.1.4]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.4
[0.1.3]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.3
[0.1.2]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.2
[0.1.1]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.1
[0.1.0]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.0

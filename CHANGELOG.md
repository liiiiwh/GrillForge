# Changelog

All notable changes to GrillForge are documented in this file.

The project follows [Semantic Versioning](https://semver.org/).

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

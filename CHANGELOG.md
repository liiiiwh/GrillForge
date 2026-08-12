# Changelog

All notable changes to GrillForge are documented in this file.

The project follows [Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-08-12

### Added

- A client-scoped MCP broker exposes global extension SubAgents to each bound
  Coding Agent without implementing an Agent Runtime or tool loop.
- Extension SubAgents can use discovered Claude Code or Codex Agents as their
  local runtime source; managed routes preserve the source Agent instructions.
- Pi can detect and install the reviewed `pi-mcp-extension` package with one
  click before mounting its MCP configuration.
- Extension bindings update the mounted MCP tool list immediately and are
  restored automatically when GrillForge starts.
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
  Kimi Code, Gemini CLI, Grok Build, OpenCode, OpenClaw, and Hermes.

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
  Grok Build, OpenCode, OpenClaw, and Hermes.
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
[0.2.0]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.2.0
[0.1.5]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.5
[0.1.4]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.4
[0.1.3]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.3
[0.1.2]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.2
[0.1.1]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.1
[0.1.0]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.0

# Changelog

All notable changes to GrillForge are documented in this file.

The project follows [Semantic Versioning](https://semver.org/).

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
- Claude Client Code is covered by an isolated end-to-end test using its bundled
  Claude Code binary and a loopback-only API server.

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
- Applying Claude Code configuration now tells users to restart an already-open
  Claude Client Code session so it reloads the shared route environment.

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
- Claude Code and Claude Client Code now resolve the installed selector binary
  even when the GUI application is not on the login-shell PATH.

[0.1.5]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.5
[0.1.4]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.4
[0.1.3]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.3
[0.1.2]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.2
[0.1.1]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.1
[0.1.0]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.0

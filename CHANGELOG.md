# Changelog

All notable changes to GrillForge are documented in this file.

The project follows [Semantic Versioning](https://semver.org/).

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

[0.1.0]: https://github.com/liiiiwh/GrillForge/releases/tag/v0.1.0

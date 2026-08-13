<p align="center">
  <img src="./src-tauri/icons/128x128.png" width="96" height="96" alt="GrillForge Logo">
</p>

<h1 align="center">GrillForge</h1>

<p align="center">
  Let coding agents reuse local native agents and choose models per task
</p>

<p align="center">
  <a href="./README.md">简体中文</a> · <a href="./README_EN.md">English</a>
</p>

<p align="center">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-000000?logo=rust&logoColor=white">
  <img alt="React" src="https://img.shields.io/badge/React-19-149ECA?logo=react&logoColor=white">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-2ea44f">
  <img alt="Status" src="https://img.shields.io/badge/status-MVP-f59e0b">
</p>

GrillForge is a lightweight, local-first model and SubAgent control plane for multiple coding clients. It reuses native coding-agent CLIs, lets clients share local Agents, and selects different Providers and models per task.

> [!IMPORTANT]
> GrillForge does not implement an Agent Runtime, Agent Loop, or workflow engine. MCP provides lightweight discovery and task forwarding; the coding-agent CLI / Runtime already installed on the user's machine still owns the Agent Loop and tools. GrillForge only manages client configuration, SubAgent authorization, and Provider/model routing.

## Core Capabilities

- **Invoke native Agents across clients**: sync local Claude Code, Codex, Pi, Kimi Code, OpenCode, and other Agents, then authorize them for other coding clients through client-scoped MCP.
- **Choose models per scenario**: coding, research, review, and testing SubAgents can each bind a source Agent, Provider, and model.
- **Flexible model slots**: configure the default model, role models, native SubAgent default, custom Agents, and model pools according to each client's real capabilities. Different slots may use different Providers.
- **Reuse native Runtimes**: the user's installed CLI / Runtime still owns the Agent Loop, tools, and context. GrillForge implements neither an Agent Runtime nor a workflow engine.
- **Local model control**: share one Provider / Model Registry and bridge Anthropic, OpenAI Responses, OpenAI Chat, and Gemini protocols with atomic configuration and fail-fast errors.

## Current Support

### Coding Agent Clients

| Client | Supported configuration | Status |
| --- | --- | --- |
| Claude Code | Default, Sonnet / Opus / Fable / Haiku, and native SubAgent-default slots; extension SubAgent MCP | Implemented and verified with a real CLI and local Agent tool loop |
| Claude Client | Safe conversation / Cowork role routes; extension SubAgent MCP in both 1P and 3P | Implemented and verified through the local configuration path |
| Codex | Main model, built-in SubAgent default, and per-custom-Agent models; standalone and ChatGPT-bundled CLI support | Implemented and verified with real CLI configuration |
| Pi | Default model, available model pool, and extension SubAgents through community `pi-mcp-extension` | Implemented with real CLI, extension-install, authentication, and gateway verification |
| Kimi Code | Default model, SubAgent model pool, `agent` / `coder` / `explore` / `plan`, and custom Agents | Implemented with the current `~/.kimi-code/config.toml`, `mcp.json`, and Agent directory structure |
| Gemini CLI | Default model | Implemented |
| Grok Build | Default model | Implemented |
| OpenCode | Default model and model pool | Implemented |
| Hermes | Default model and model pool | Implemented |

Client discovery covers PATH, standard installation locations, and common Node version managers. When duplicate CLIs exist, GrillForge validates each candidate and uses the first working version. Opening Clients refreshes status in the background.

### Providers and Protocols

| Protocol | Capabilities |
| --- | --- |
| Anthropic Messages | Native requests, streaming, tool calls, images, and Thinking |
| OpenAI Responses | Request/response translation, SSE, tools, Reasoning, images, and documents |
| OpenAI Chat Compatible | Request/response translation, SSE, tools, explicit reasoning fields, and images |
| Gemini Native | Direct Gemini CLI configuration plus streaming/tool translation from Claude and Pi inbound requests |
| Local models | Unauthenticated loopback endpoints such as Ollama or a local compatible gateway |

Providers support protocol presets, custom endpoints, API keys, automatic/manual model sync, and connection tests. Sync probes the protocols each model actually supports; the gateway connects directly when protocols match and otherwise uses a tested bridge. Supported Providers can query live balances or Coding Plan quotas; GrillForge keeps no local traffic ledger.

## How It Works

```mermaid
flowchart LR
    UI["GrillForge GUI"] --> Core["Control Plane"]
    Core --> Adapter["Client Adapter"]
    Core --> Registry["Provider / Model Registry"]
    Adapter --> Client["Coding Agent Client"]
    Client --> Gateway["Local Gateway"]
    Client --> MCP["Client-scoped MCP"]
    MCP --> Runtime["User-installed Agent Runtime"]
    Gateway --> Bridge["Protocol Bridge"]
    Bridge --> Provider["Anthropic / OpenAI Compatible / Local"]
```

- **Client Adapter** detects a client, reads state, writes configuration, installs a required client extension where applicable, and restores the pre-takeover state.
- **Provider Layer** owns endpoints, authentication placement, and API protocols.
- **Model Registry** stores upstream model IDs, display names, task capabilities, and transport capabilities.
- **Local Gateway** performs authentication replacement, model routing, and protocol translation. It never executes agent tools.

Extension invocation path: `Primary Agent → GrillForge MCP → Extension SubAgent → local native CLI / Runtime → Provider model`.

## Download

Download from [GitHub Releases](https://github.com/liiiiwh/GrillForge/releases/latest).

## Quick Start

### Prerequisites

- Node.js 20.19+ or 22.12+
- pnpm 10+
- Rust 1.85+ and Cargo
- The [Tauri 2 system prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform

### Run from Source

```bash
git clone https://github.com/liiiiwh/GrillForge.git
cd GrillForge
pnpm install
pnpm tauri dev
```

### Basic Workflow

1. Choose a preset or add a custom Provider on the Providers page.
2. Sync the model list, or manually add a model with its exact upstream model ID.
3. Run a connection test to validate the Provider, endpoint, credential, and model.
4. Open Clients, choose a Provider first, then choose the model or model pool.
5. Apply the configuration. Multiple clients can be saved and applied independently.
6. Keep GrillForge running while a gateway-backed client is in use. Disabling it restores the pre-takeover configuration.

### Extension SubAgents

1. Sync and choose a local Agent on Extension SubAgents.
2. Keep the source Agent's native model or bind a GrillForge model.
3. Mount the client-scoped MCP on the destination client, then enable the extensions it may use. Binding changes update the mounted Agent list immediately; removing all bindings does not unmount MCP.
4. Pi connects through community `pi-mcp-extension`; GrillForge installs a pinned version only after user confirmation.

Model configuration, MCP mounting, and Extension SubAgent bindings remain independent. MCP exposes only fixed Agent-list and invocation entries; the source client's local Runtime always executes the Agent Loop and tools.

## Configuration and Security

Control-plane data is stored under:

```text
~/.grillforge/
├── config.yaml
├── models.yaml
├── agents.yaml
└── *.snapshot.json
```

- Configuration files are written with user-only permissions; credentials are excluded from public frontend state.
- Writes use atomic replacement. Multi-file configuration is fully validated before commit.
- Each adapter keeps one recovery snapshot rather than an unbounded backup history.
- Difference reports contain only field or file names—not credentials or configuration values.
- GrillForge does not automatically retry, downgrade protocols, or fall back across Providers.
- Unauthenticated Providers are restricted to loopback addresses; remote endpoints require HTTPS.

> [!WARNING]
> A custom `ANTHROPIC_BASE_URL` can disable Claude Remote Control and default Optimistic Tool Search. Disabling GrillForge restores the original Claude configuration.

> [!NOTE]
> `pi-mcp-extension` is a community extension and has the same local process access as other Pi extensions. GrillForge never installs it silently. Installation requires user confirmation and uses the pinned source `npm:pi-mcp-extension@1.5.0`.

## Development

### Common Commands

```bash
# Frontend build
pnpm build

# Development app
pnpm tauri dev

# Rust formatting
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check

# Complete test suite
cargo test --manifest-path src-tauri/Cargo.toml --all-targets --all-features

# Strict linting
cargo clippy --manifest-path src-tauri/Cargo.toml \
  --all-targets --all-features -- -D warnings
```

### Optional Live Route Test

Default tests never use a real API key. Live Provider tests read credentials only from the process environment:

```bash
GRILLFORGE_LIVE_API_KEY='...' \
GRILLFORGE_LIVE_PROTOCOL=anthropic_messages \
GRILLFORGE_LIVE_ENDPOINT=https://api.example.com/anthropic \
GRILLFORGE_LIVE_MODEL=your-model-id \
GRILLFORGE_LIVE_API_KEY_PLACEMENT=bearer \
cargo test --manifest-path src-tauri/Cargo.toml \
  --test live_provider -- --ignored
```

Never place real credentials in source files, fixtures, snapshots, shell history, or default CI jobs.

### macOS Packaging

```bash
pnpm tauri build --target universal-apple-darwin --bundles app
```

The bundle is written to:

```text
src-tauri/target/universal-apple-darwin/release/bundle/macos/GrillForge.app
```

## Contributing

Issues and pull requests are welcome. Before contributing, read:

- [AGENTS.md](./AGENTS.md) for the Small and Beautiful, Fail Fast, and TDD engineering creed
- [CONTEXT.md](./CONTEXT.md) for product scope and non-goals
- [ARCHITECTURE.md](./ARCHITECTURE.md) for module boundaries
- [LOGIC.md](./LOGIC.md) for configuration and routing invariants

Keep changes narrow, add tests at public boundaries, and ensure `build`, `fmt`, `test`, and `clippy` all pass. A new client must be backed by its real installation, configuration, and runtime path; UI-only placeholder adapters are not accepted.

## Acknowledgements

- [cc-switch](https://github.com/farion1231/cc-switch) for important references covering Provider presets, client behavior, and protocol bridges. Ported code retains its MIT attribution and third-party notices.
- [Tauri](https://tauri.app/), [React](https://react.dev/), and the Rust ecosystem.
- The coding-agent projects and their public configuration specifications.

See [THIRD_PARTY_LICENSES](./THIRD_PARTY_LICENSES/) and `src-tauri/src/bridge/LICENSE.cc-switch` for third-party notices.

## License

GrillForge is open source under the [MIT License](./LICENSE). Third-party code remains governed by its original licenses.

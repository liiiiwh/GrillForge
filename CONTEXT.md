# GrillForge Context

Status: Canonical project context

## Purpose

GrillForge is the universal model control plane for AI coding agents.

It connects coding agents to a shared ecosystem of model providers and gives
users one place to manage:

- Coding agent connections
- Main model selection
- Native model Slots and extension SubAgent bindings
- Provider endpoints and credentials
- Model identities and capabilities, including the context window a client needs
  to know before it can size a conversation correctly

GrillForge is not itself a coding agent. Coding agents retain ownership of their
agent loop, context, tools, permissions, and task execution.

## Target Users

The initial users are developers who use one or more supported coding clients
and want to:

- Keep the normal Claude experience available
- Switch the main model when desired
- Make external models available to local Agents without replacing their runtime
- Reuse Anthropic, OpenAI-compatible, and local model providers
- Configure providers and models through a small desktop GUI

The long-term audience includes users of Codex, Pi, Kimi Code, Hermes,
OpenCode, and other coding agents that should share the same provider and model
registry.

## Product Positioning

Current product:

> Coding Agent Client configuration center + Provider Layer + Model Registry

Long-term product:

> The Control Plane for AI Coding Agents.

GrillForge must not be described as only a Claude Code model switcher or only a
Claude Code SubAgent extension. Claude Code is the first supported agent, not
the owner of the core domain.

## Current Development Stage

The client-first MVP is implemented and its model-routing release matrix is
verified.

The repository contains the Tauri desktop application, Provider and Model
Registry, client adapters, local gateway, Anthropic/Responses/Chat/Gemini
protocol bridges, a client-scoped MCP broker, deterministic tests, and the
pinned read-only cc-switch reference. A real installed Claude Code runtime has
executed its own Agent and Read-tool loop through the broker against a local
deterministic upstream. GrillForge did not implement that loop or execute the
tool.

The first release implements Claude Code, Claude Client, Codex, Pi, Gemini CLI,
Grok Build, OpenCode, Hermes, and Kimi Code adapters. Kimi Code uses its current
official `~/.kimi-code/config.toml` default-model and model-pool configuration
and exposes its current built-in and scoped custom Agents.

The current macOS artifact is a Universal x86_64/arm64 App bundle signed with a
Developer ID Application identity, notarized by Apple, stapled, and accepted by
Gatekeeper.

## Engineering Philosophy

GrillForge must remain small and beautiful.

- Implement the smallest complete behavior.
- Prefer direct, readable code and a single source of truth.
- Validate configuration immediately and fail fast with an actionable error.
- Do not silently repair invalid input or hide runtime failures.
- Do not add unlimited retries, automatic fallback chains, duplicate state, or
  speculative compatibility layers.
- Preserve only narrow safety mechanisms required to avoid data loss: atomic
  writes, credential redaction, and one recoverable configuration snapshot.

These rules are expanded in the repository-root `AGENTS.md` and apply to every
future implementation and review.

## MVP Scope

The MVP implements:

- A Tauri 2 desktop application with a React frontend and Rust backend
- macOS and Windows support
- Provider add/edit/delete and connection testing
- Provider configuration and presets adapted from cc-switch
- Main model selection, including a Native/Default state
- Native client Slots and extension SubAgent capability descriptions
- Independent adapters for every current cc-switch coding client plus Pi
- Claude Code detection, configuration backup/apply/restore, and status
- Extension SubAgent library and per-client MCP bindings
- A local Anthropic-compatible protocol gateway when model routing requires it
- Anthropic Messages, OpenAI Responses, OpenAI Chat-compatible, and Gemini
  Native upstreams

## Non-Goals

The MVP does not implement:

- An Agent Runtime
- A custom SubAgent framework
- An agent loop
- A workflow engine or workflow editor
- A task scheduler
- An Agent Runtime hidden behind its MCP server
- An agent marketplace
- A graph editor
- Other clients without a tested current adapter
- A dynamic Agent Adapter plugin framework
- User-specific Grill workflows

The local gateway may translate model API payloads, tool-call descriptions, and
streaming events. It never executes tools and never owns the agent lifecycle.

Keeping a started run reachable is not a task scheduler. GrillForge holds a
handle to a run the delegating Agent started, so that Agent can collect it later
instead of waiting; it never decides what runs, when, or in what order. Relaying
a permission prompt to the Agent that delegated the work is likewise not a
policy decision: GrillForge carries the question and the answer and makes
neither.

## Extension SubAgent Boundary

GrillForge may mount a small MCP broker into a supported destination client.
The broker resolves a user-approved local Agent, starts its already-installed
Coding Agent runtime, and applies an optional model route. The source runtime
still owns the Agent loop, prompt, context, tools, and permissions. GrillForge
does not bundle or own the user's workflow.

Pi has no native MCP integration. GrillForge may, after explicit user
confirmation, install pinned community package `pi-mcp-extension@1.5.0` through
the detected valid Pi CLI. It never installs this package silently.

## Upstream Reuse Policy

All model-provider compatibility work must first be learned from and adapted
from cc-switch. This includes provider presets, configuration fields,
authentication behavior, endpoint construction, model mapping, protocol
conversion, streaming, tool calls, reasoning data, multimodal data, and Codex
OAuth.

Pinned reference:

```text
repository: https://github.com/farion1231/cc-switch
commit: 413c09e0790c304506888ae24b9be72820aca126
local path: upstream/cc-switch
license: MIT
```

Copied or substantially derived code must retain the upstream MIT notice.

## Requirement Precedence

Later requirements supersede conflicting earlier statements.

In particular, the original requirement that GrillForge never manage a main
model has been replaced. GrillForge now supports cc-switch-style main model
management, while preserving Native/Default as the state in which it does not
override the coding agent's main model.

The canonical implementation documents, in precedence order, are:

1. `CONTEXT.md`
2. `ARCHITECTURE.md`
3. `LOGIC.md`

Release history belongs in `CHANGELOG.md`; superseded requirement drafts are
not retained as competing sources of truth.

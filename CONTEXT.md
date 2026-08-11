# GrillForge Context

Status: Canonical project context

## Purpose

GrillForge is the universal model control plane for AI coding agents.

It connects coding agents to a shared ecosystem of model providers and gives
users one place to manage:

- Coding agent connections
- Main model selection
- Worker/SubAgent model pools
- Provider endpoints and credentials
- Model identities and capabilities

GrillForge is not itself a coding agent. Coding agents retain ownership of their
agent loop, context, tools, permissions, and task execution.

## Target Users

The initial users are developers who use one or more supported coding clients
and want to:

- Keep the normal Claude experience available
- Switch the main model when desired
- Make external models available to Claude Code SubAgents
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
Registry, Claude Code Adapter, local gateway, Anthropic/Responses/Chat protocol
bridges, selector Skill, deterministic tests, and the pinned read-only
cc-switch reference. Mock, installed-CLI, and real DeepSeek acceptance tests
pass across Anthropic Messages, OpenAI Responses, and OpenAI Chat-compatible
protocols. One real Claude main session invoked two distinct generated
DeepSeek Worker Agents and received both results. The supplied credential was
injected only through the test process environment and is not part of
repository state.

The first release implements Claude Code, Claude Client, Codex, Pi, Gemini CLI,
Grok Build, OpenCode, OpenClaw, Hermes, and Kimi Code adapters. Kimi Code uses
its current official primary/secondary model configuration and synchronizes
built-in and persistent global Agent definitions without pretending that it
supports arbitrary per-Agent model IDs.

The current macOS artifact is a Universal x86_64/arm64 App bundle with a valid
Apple Development signature for local verification. Public distribution still
requires a Developer ID Application identity and notarization.

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
- Worker model enable/disable and capability descriptions
- Independent adapters for every current cc-switch coding client plus Pi
- Claude Code detection, configuration backup/apply/restore, and status
- GrillForge Skill installation for model capability discovery and selection
- A local Anthropic-compatible protocol gateway when model routing requires it
- Anthropic Messages, OpenAI Responses, and OpenAI Chat-compatible upstreams

## Non-Goals

The MVP does not implement:

- An Agent Runtime
- A custom SubAgent framework
- An agent loop
- A workflow engine or workflow editor
- A task scheduler
- An MCP server
- An agent marketplace
- A graph editor
- Other clients without a tested current adapter
- A dynamic Agent Adapter plugin framework
- User-specific Grill workflows

The local gateway may translate model API payloads, tool-call descriptions, and
streaming events. It never executes tools and never owns the agent lifecycle.

## Relationship With Grill Skill

GrillForge publishes available models and their capabilities. The user's Grill
Skill may read that information and decide which native Coding Agent SubAgent
to create and which Worker model to request.

GrillForge does not bundle or own the Grill workflow.

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

The canonical implementation documents are:

1. `CONTEXT.md`
2. `ARCHITECTURE.md`
3. `LOGIC.md`
4. `GRILLFORGE_ARCHITECTURE_SUPPLEMENT.md`
5. Earlier final specification and review documents

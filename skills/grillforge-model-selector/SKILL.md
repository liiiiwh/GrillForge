---
name: grillforge-model-selector
description: Query GrillForge's credential-free effective Claude Code Worker pool before choosing a native SubAgent. Use when the Main Agent is considering delegating coding, review, refactor, test, or reasoning work to a GrillForge-managed Worker.
---

# Select a GrillForge Worker

1. Resolve `scripts/select_models.py` relative to this file and run it once with the available Python 3 interpreter from the Main Agent before choosing a Worker.
2. Read the returned `workers` array. Treat `capabilities` only as selection hints; never treat them as tool grants, permissions, or reasons to create a SubAgent.
3. If `workers` is empty, continue with native Claude Code SubAgent behavior.
4. If the command fails, surface its error immediately. Do not fall back to native behavior or another model.
5. If delegating, choose one returned Worker and invoke its exact `agentName` as the native Claude Code `subagent_type`.

Claude Client's Code view can use an external Worker only while Claude Client is running the GrillForge 3P profile. The selector checks that host context before returning a Worker, so never bypass a Claude Client routing error or invoke the Agent anyway.

Never invoke this selector from a delegated SubAgent. Never parse GrillForge YAML, read Provider credentials, call model APIs, or invent routes.

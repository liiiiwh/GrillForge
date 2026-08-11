# GrillForge
## Claude Code External SubAgent Model Extension Layer

Version: Final Draft

---

# 1. Project Positioning

GrillForge is a cross-platform desktop application that manages optional external models for Claude Code SubAgents.

The core principle:

Claude Code remains unchanged.

Claude remains the main brain.

Claude Code native SubAgent system remains responsible for:

- Agent lifecycle
- Context management
- Tool calling
- Permissions
- Execution flow

GrillForge only provides:

- External model provider management
- Model capability registry
- Claude Code skill installation
- External SubAgent model availability information

---

# 2. Design Principles

## Must keep

Claude Code native behavior.

If GrillForge is disabled or no external model is enabled:

Claude Code works exactly as before.

Native Claude SubAgents are untouched.

---

## Must NOT implement

Do not build:

- Agent Runtime
- Custom SubAgent system
- MCP Agent server
- Workflow engine
- Task scheduler
- Tool calling proxy
- Claude protocol emulator

Do not:

- Patch Claude Code binary
- Hook private APIs
- Fake Claude signatures

---

# 3. Product Definition

GrillForge is similar to cc-switch.

Difference:

cc-switch:
- Switches main model configuration

GrillForge:
- Manages optional external SubAgent models for Claude Code

---

# 4. Architecture

```
                 Human

                   |

             Claude Code

                   |

          Main Claude Model

             (Brain)

                   |

       Native Claude SubAgent System

                   |

        Optional GrillForge Skill

                   |

       External Model Registry

                   |

     OpenAI / DeepSeek / Qwen / Others

```

---

# 5. Core Logic

Claude Default is NOT managed by GrillForge.

There is no:

- claude-default model entry
- anthropic default mapping
- Claude model selection

Claude Code native behavior remains default.

---

## External Model Enabled Logic

Example:

User configures:

- GPT-5-Codex
- DeepSeek-R1
- Qwen3-Coder

User enables:

- GPT-5-Codex
- DeepSeek-R1


Active external model pool:

```
GPT-5-Codex
DeepSeek-R1
```

Claude Code can use these models through the installed skill/configuration.

---

## No External Models Enabled

If:

```
enabled external models = 0
```

Then:

```
GrillForge does nothing.

Claude Code uses native Claude SubAgents.
```

---

# 6. Model Pool Design

The system uses an enabled model pool.

Any configured model can be enabled or disabled.

Example:

```
[x] GPT-5-Codex

[x] DeepSeek-R1

[ ] Qwen3-Coder
```

Requirement:

At least one model must remain enabled if external model mode is activated.

---

# 7. Provider Layer

## Important

Do not rebuild provider compatibility.

Reuse/adapt cc-switch provider and endpoint related code.

Reuse:

- Provider definitions
- Endpoint handling
- API key management
- OpenAI compatible support

Goal:

Minimum code and minimum maintenance.

---

# 8. Provider Support

Priority:

OpenAI Compatible API.

Support:

- OpenAI
- DeepSeek
- Qwen
- OpenRouter
- SiliconFlow
- Ollama
- vLLM
- NewAPI

Configuration:

```
provider:
  type: openai-compatible
  base_url:
  api_key:
```

---

# 9. Configuration

Location:

```
~/.grillforge/
```

Example:

```
config.yaml
models.yaml
```

models.yaml:

```yaml
models:

  gpt-5-codex:
    provider: openai
    model: gpt-5-codex
    enabled: true
    capabilities:
      - coding
      - refactor


  deepseek-r1:
    provider: deepseek
    model: deepseek-r1
    enabled: true
    capabilities:
      - reasoning
      - review


  qwen3-coder:
    provider: qwen
    model: qwen3-coder
    enabled: false
    capabilities:
      - coding
```

---

# 10. Desktop Console

Technology:

Recommended:

- Tauri 2
- React
- Rust backend

Platforms:

- macOS
- Windows

---

# 11. GUI Requirements

## Provider Management

Functions:

- Add provider
- Edit provider
- Delete provider
- Test connection


## Model Management

Fields:

- Model name
- Provider
- Model ID
- Capability tags
- Enable switch


## External Model Switch

Example:

```
External SubAgent Models

[ON]


Models:

[x] GPT-5-Codex

[x] DeepSeek-R1

[ ] Qwen3-Coder
```

Disable:

```
External SubAgent Models

[OFF]
```

Behavior:

Claude Code returns to native behavior.

---

# 12. Claude Code Skill

Install:

```
~/.claude/skills/grillforge-model-selector/
```

Purpose:

Only provide external model capability information.

Skill responsibilities:

1. Read:

```
~/.grillforge/models.yaml
```

2. Find enabled models.

3. Provide model capability information.

4. Let Claude Code decide suitable SubAgent usage.

---

# 13. Skill Restrictions

Skill must NOT:

- Call external model APIs
- Create custom agents
- Manage workflows
- Replace Claude Code

---

# 14. Grill Skill Relationship

Grill is user's personal workflow skill.

GrillForge does not include Grill.

Relationship:

```
Claude Code

 |

Grill Skill

 |

Read GrillForge model registry

 |

Create native Claude Code SubAgents

```

---

# 15. Installation

Desktop application:

1. Detect Claude Code installation

2. Install:

```
model-selector skill
```

3. Create:

```
~/.grillforge/
```

No MCP required.

---

# 16. Minimal Development Requirement

Important:

Do the smallest implementation possible.

Reuse existing solutions.

Avoid unnecessary abstraction.

Do NOT create:

- AgentManager
- WorkflowManager
- RuntimeScheduler
- TaskEngine

These are outside scope.

---

# 17. Development Phases

## Phase 1

Desktop console:

- Provider management
- Model management
- Configuration storage


## Phase 2

Claude integration:

- Detect Claude Code
- Install skill
- Generate configuration


## Phase 3

External model enable/disable:

- Model pool management
- Skill reads enabled models


---

# 18. Acceptance Criteria

Scenario 1:

No external models enabled.

Result:

Claude Code works normally.

---

Scenario 2:

User enables:

```
GPT-5-Codex
DeepSeek-R1
```

Claude Code can access external model capability information.

---

Scenario 3:

User disables all external models.

Result:

Claude Code returns to native behavior.

---

# Final Definition

GrillForge is:

"Claude Code External SubAgent Model Extension Layer"

It is NOT:

"Agent Framework"

Claude remains the brain.

External models are optional workers.

The product focuses on:

- provider reuse
- model pool management
- skill-based integration
- minimum implementation complexity

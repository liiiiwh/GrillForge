import { invoke } from "@tauri-apps/api/core";
import { FormEvent, ReactNode, useEffect, useMemo, useState } from "react";
import "./App.css";
import { DEFAULT_LOCALE, createTranslator } from "./i18n";
import grillforgeLogo from "./assets/grillforge-logo.png";
import aihubmixIcon from "./assets/provider-icons/aihubmix-color.svg";
import alibabaIcon from "./assets/provider-icons/alibaba.svg";
import anthropicIcon from "./assets/provider-icons/anthropic.svg";
import apinebulaIcon from "./assets/provider-icons/apinebula_icon.png";
import atlascloudIcon from "./assets/provider-icons/atlascloud_icon.png";
import azureIcon from "./assets/provider-icons/azure.svg";
import baiduIcon from "./assets/provider-icons/baidu.svg";
import bytedanceIcon from "./assets/provider-icons/bytedance.svg";
import cloudflareIcon from "./assets/provider-icons/cloudflare.svg";
import code0Icon from "./assets/provider-icons/code0.png";
import cohereIcon from "./assets/provider-icons/cohere.svg";
import deepseekIcon from "./assets/provider-icons/deepseek.svg";
import doubaoIcon from "./assets/provider-icons/doubao.svg";
import fennoIcon from "./assets/provider-icons/fenno-icon.webp";
import geminiIcon from "./assets/provider-icons/gemini.svg";
import googleIcon from "./assets/provider-icons/google.svg";
import grokIcon from "./assets/provider-icons/grok.svg";
import hermesIcon from "./assets/provider-icons/hermes.png";
import huggingfaceIcon from "./assets/provider-icons/huggingface.svg";
import hunyuanIcon from "./assets/provider-icons/hunyuan.svg";
import kimiIcon from "./assets/provider-icons/kimi.svg";
import longcatIcon from "./assets/provider-icons/longcat-color.svg";
import minimaxIcon from "./assets/provider-icons/minimax.svg";
import mistralIcon from "./assets/provider-icons/mistral.svg";
import modelscopeIcon from "./assets/provider-icons/modelscope-color.svg";
import novitaIcon from "./assets/provider-icons/novita.svg";
import nvidiaIcon from "./assets/provider-icons/nvidia.svg";
import ollamaIcon from "./assets/provider-icons/ollama.svg";
import piIcon from "./assets/provider-icons/pi.svg";
import openaiIcon from "./assets/provider-icons/openai.svg";
import opencodeIcon from "./assets/provider-icons/opencode-logo-light.svg";
import openrouterIcon from "./assets/provider-icons/openrouter.svg";
import qwenIcon from "./assets/provider-icons/qwen.svg";
import siliconflowIcon from "./assets/provider-icons/siliconflow.svg";
import stepfunIcon from "./assets/provider-icons/stepfun.svg";
import xaiIcon from "./assets/provider-icons/xai.svg";
import xiaomiIcon from "./assets/provider-icons/xiaomimimo.svg";
import zhipuIcon from "./assets/provider-icons/zhipu.svg";

const t = createTranslator(DEFAULT_LOCALE);

type View = "overview" | "clients" | "extension_subagents" | "providers" | "routes";
type ClientTab = "slots" | "extension_subagents";
type Protocol =
  | "anthropic_messages"
  | "open_ai_responses"
  | "open_ai_chat_completions"
  | "gemini_native";
type ApiKeyPlacement = "none" | "bearer" | "x_api_key";
type Takeover = "inactive" | "active" | "reapply_required" | "drifted";
type ProtocolCapability =
  | "reasoning_items"
  | "reasoning_content"
  | "reasoning_effort";
type NativeProtocol =
  | "anthropic_messages"
  | "openai_responses"
  | "openai_chat"
  | "gemini_native";

type Provider = {
  id: string;
  name: string;
  protocol: Protocol;
  endpoint: string;
  endpointMode: "base_url" | "exact_url";
  apiKeyPlacement: ApiKeyPlacement;
  enabled: boolean;
  credentialSet: boolean;
  modelsUrl: string | null;
  protocolEndpoints: Array<{
    protocol: NativeProtocol;
    endpoint: string;
    endpointMode: "base_url" | "exact_url";
    apiKeyPlacement: ApiKeyPlacement;
  }>;
};

type Model = {
  id: string;
  name: string;
  upstreamId: string;
  providerId: string;
  capabilities: string[];
  protocolCapabilities: ProtocolCapability[];
  nativeProtocols: NativeProtocol[];
  unsupportedNativeProtocols: NativeProtocol[];
  routeAlias: string;
  contextWindow?: number;
  maxOutputTokens?: number;
};

type ExtensionSubAgent = {
  id: string;
  name: string;
  sourceClientId: string;
  sourceAgentId: string;
  modelId: string | null;
  capabilities: string[];
};

type LocalAgent = {
  runtime: string;
  agentId: string;
  description: string;
};

type LocalAgentDiscovery = {
  agents: LocalAgent[];
  errors: Array<{ runtime: string; message: string }>;
};

type ControlPlaneState = {
  providers: Provider[];
  models: Model[];
  agentEnabled: boolean;
  mainModelId: string | null;
  modelSlots: Record<string, string>;
  claudeNativeModelSlots: Record<string, string>;
  claudeDesktopModelSlots: Record<string, string>;
  piEnabled: boolean;
  piMainModelId: string | null;
  piEnabledModelIds: string[];
  codexMainModelId: string | null;
  codexNativeModelSlots: Record<string, string>;
  codexAgentModelIds: Record<string, string>;
  clientConfigurations: Record<string, ClientConfiguration>;
  extensionSubagents: ExtensionSubAgent[];
  clientExtensionSubagentIds: Record<string, string[]>;
  mcpMountedClientIds: string[];
};

type ClientMcpStatus = {
  clientId: string;
  desiredMounted: boolean;
  mounted: boolean;
  configurationChanged: boolean;
};

type ClaudeDesktopIntegrationCommand =
  | "apply_claude_desktop"
  | "disable_claude_desktop";

type DesktopRestartClient = "claude_desktop" | "codex";

export function claudeClientRestartRequired(
  command: ClaudeDesktopIntegrationCommand,
) {
  return (
    command === "apply_claude_desktop" ||
    command === "disable_claude_desktop"
  );
}

export function desktopClientRestartAfterMcpChange(clientId: string) {
  return clientId === "claude_desktop" || clientId === "codex";
}

export function extensionMountCopy(status?: ClientMcpStatus) {
  const mounted = Boolean(status?.mounted);
  const needsReapply = Boolean(status?.configurationChanged);

  return {
    badge: needsReapply
      ? "扩展配置有变化"
      : mounted
        ? "扩展已挂载"
        : status?.desiredMounted
          ? "扩展等待恢复"
          : "扩展未挂载",
    action: needsReapply
      ? "重新挂载扩展"
      : mounted || Boolean(status?.desiredMounted)
        ? "卸载扩展"
        : "挂载扩展",
  };
}

type ClientConfiguration = {
  mainModelId: string | null;
  enabledModelIds: string[];
};

type KimiCodeAgent = {
  name: string;
  description: string;
};

type ClientIntegrationStatus = {
  installed: boolean;
  executablePath: string | null;
  version: string | null;
  snapshotPresent: boolean;
  takeover: Takeover;
  configuredModelIds: string[];
  mainModelId: string | null;
  agents?: KimiCodeAgent[];
};

const additionalClients = [
  {
    id: "gemini",
    name: "Gemini CLI",
    mark: "GM",
    status: "gemini_status",
    apply: "apply_gemini",
    disable: "disable_gemini",
    pool: false,
    protocol: "gemini",
  },
  {
    id: "grok_build",
    name: "Grok Build",
    mark: "GB",
    status: "grok_build_status",
    apply: "apply_grok_build",
    disable: "disable_grok_build",
    pool: false,
    protocol: "responses",
  },
  {
    id: "dsh",
    name: "DeepSeek Harness",
    mark: "DH",
    status: "dsh_status",
    apply: "apply_dsh",
    disable: "disable_dsh",
    pool: true,
    protocol: "gateway",
  },
  {
    id: "opencode",
    name: "OpenCode",
    mark: "OC",
    status: "opencode_status",
    apply: "apply_opencode",
    disable: "disable_opencode",
    pool: true,
    protocol: "gateway",
  },
  {
    id: "hermes",
    name: "Hermes",
    mark: "HE",
    status: "hermes_status",
    apply: "apply_hermes",
    disable: "disable_hermes",
    pool: true,
    protocol: "gateway",
  },
  {
    id: "kimi_code",
    name: "Kimi Code",
    mark: "KM",
    status: "kimi_code_status",
    apply: "apply_kimi_code",
    disable: "disable_kimi_code",
    pool: true,
    protocol: "gateway",
  },
] as const;

type IntegrationStatus = {
  snapshotPresent: boolean;
  takeover: Takeover;
  differences: string[];
  managedMainAlias: string | null;
  nativeModelSlots: Record<string, string>;
  supportedModelSlots: string[];
  nativeModels: { id: string; name: string }[];
  nativeModelsError: string | null;
  nativeCurrentModel: string | null;
};

type ClaudeCliStatus = {
  installed: boolean;
  path: string | null;
  version: string | null;
};

type ClaudeDesktopStatus = {
  installed: boolean;
  executablePath: string | null;
  snapshotPresent: boolean;
  takeover: Takeover;
  differences: string[];
  configuredRoutes: string[];
  supportedModelSlots: string[];
  nativeModels: { id: string; name: string }[];
  nativeModelsError: string | null;
  nativeCurrentModel: string | null;
};

type PiStatus = {
  installed: boolean;
  executablePath: string | null;
  version: string | null;
  snapshotPresent: boolean;
  takeover: Takeover;
  configuredModelIds: string[];
  defaultModelId: string | null;
};

type PiMcpExtensionStatus = {
  installed: boolean;
  packageSource: string;
};

type CodexStatus = {
  installed: boolean;
  executablePath: string | null;
  version: string | null;
  snapshotPresent: boolean;
  takeover: Takeover;
  configuredModelId: string | null;
  currentConfigModel: string | null;
  currentConfigProvider: string | null;
  supportedProtocols: Protocol[];
  nativeModels: { id: string; name: string }[];
  nativeModelsError: string | null;
  customAgents: {
    name: string;
    description: string;
    configuredModel: string | null;
  }[];
};

type PresetParameter = {
  id: string;
  label: string;
  placeholder: string;
  required: boolean;
  default_value?: string | null;
};

type PresetEndpoint =
  | { kind: "literal"; url: string }
  | { kind: "parameterized"; template: string; parameters: PresetParameter[] };

type ProviderPreset = {
  id: string;
  name: string;
  protocol:
    | "anthropic_messages"
    | "openai_responses"
    | "openai_chat_completions"
    | "gemini_native";
  auth: "bearer" | "x_api_key";
  endpoint: PresetEndpoint;
  suggested_models: string[];
  models_url: string | null;
};

type ProviderPresetCatalog = {
  schema_version: number;
  presets: ProviderPreset[];
};

type ProviderInput = {
  id: string;
  name: string;
  protocol: Protocol;
  endpoint: string;
  endpointMode: Provider["endpointMode"];
  apiKeyPlacement: ApiKeyPlacement;
  apiKey: string | null;
  enabled: boolean;
  modelsUrl: string | null;
};

type ProviderDraft = {
  presetId: string;
  name: string;
  protocol: Protocol;
  endpoint: string;
  endpointMode: Provider["endpointMode"];
  apiKeyPlacement: ApiKeyPlacement;
  apiKey: string;
  modelsUrl: string;
  parameters: Record<string, string>;
};

type ModelDraft = {
  name: string;
  upstreamId: string;
  providerId: string;
  capabilities: string;
  protocolCapabilities: ProtocolCapability[];
  nativeProtocols: NativeProtocol[];
  contextWindow: string;
};

type ExtensionSubAgentDraft = {
  name: string;
  sourceClientId: string;
  sourceAgentId: string;
  providerId: string;
  modelId: string;
  capabilities: string;
};

type ConnectionResult = {
  modelId: string;
  providerId: string;
  upstreamId: string;
};

type UsageSnapshot = {
  preset: string;
  kind: "balance" | "coding_plan";
  queriedAtUnixMs: number;
  items: Array<{
    label: string;
    total: number | null;
    used: number | null;
    remaining: number | null;
    unit: string | null;
    utilizationPercent: number | null;
    valid: boolean | null;
  }>;
};

type DeleteTarget = { kind: "provider" | "model"; id: string } | null;

const EMPTY_PROVIDER: ProviderDraft = {
  presetId: "",
  name: "",
  protocol: "anthropic_messages",
  endpoint: "",
  endpointMode: "base_url",
  apiKeyPlacement: "bearer",
  apiKey: "",
  modelsUrl: "",
  parameters: {},
};

const EMPTY_MODEL: ModelDraft = {
  name: "",
  upstreamId: "",
  providerId: "",
  capabilities: "",
  protocolCapabilities: [],
  nativeProtocols: [],
  contextWindow: "",
};

const EMPTY_EXTENSION_SUBAGENT: ExtensionSubAgentDraft = {
  name: "",
  sourceClientId: "",
  sourceAgentId: "",
  providerId: "",
  modelId: "",
  capabilities: "",
};

const protocolFeatures: Array<{ id: ProtocolCapability; label: string }> = [
  { id: "reasoning_items", label: "推理条目" },
  { id: "reasoning_content", label: "推理内容" },
  { id: "reasoning_effort", label: "推理强度" },
];
const nativeProtocolLabels: Record<NativeProtocol, string> = {
  anthropic_messages: "Anthropic Messages",
  openai_responses: "OpenAI Responses",
  openai_chat: "OpenAI Chat",
  gemini_native: "Gemini Native",
};

const views: Array<{ id: View; label: string; icon: string }> = [
  { id: "overview", label: t("overview"), icon: "⌂" },
  { id: "clients", label: t("clients"), icon: "◇" },
  { id: "extension_subagents", label: "扩展 SubAgent", icon: "✦" },
  { id: "providers", label: t("providers"), icon: "◎" },
  { id: "routes", label: "路由策略", icon: "⌘" },
];

const clientTabs: Array<{ id: ClientTab; label: string; capability?: string }> =
  [
    { id: "slots", label: "模型配置" },
    { id: "extension_subagents", label: "扩展 SubAgent" },
  ];

const modelSlotLabels: Record<string, string> = {
  sonnet: t("sonnetSlot"),
  opus: t("opusSlot"),
  fable: t("fableSlot"),
  haiku: t("haikuSlot"),
  subagent_default: "原生 SubAgent 默认模型",
};

const claudeNativeFallbackModels = [
  { id: "default", name: "默认（Claude 自动选择）" },
  { id: "fable", name: "Fable（最新）" },
  { id: "opus", name: "Opus（最新）" },
  { id: "sonnet", name: "Sonnet（最新）" },
  { id: "haiku", name: "Haiku（最新）" },
];

function claudeNativeModelOptions(
  current?: string,
  discovered: Array<{ id: string; name: string }> = [],
) {
  const labels: Record<string, string> = {
    default: "默认（Claude 自动选择）",
    sonnet: "Sonnet（最新）",
    opus: "Opus（最新）",
    fable: "Fable（最新）",
    haiku: "Haiku（最新）",
  };
  const options = new Map(
    [...claudeNativeFallbackModels, ...discovered].map((model) => [
      model.id,
      model.name,
    ]),
  );
  if (current && !options.has(current)) options.set(current, current);
  return Array.from(options, ([id, name]) => ({
    id,
    label: `${labels[id] ?? name}${id === current ? " · 当前选择" : ""}`,
  }));
}

export function ClaudeClientCodeSubagentSlot({
  disabled,
  selectedProviderId,
  managedModelId,
  nativeModel,
  nativeModels = [],
  providers,
  models,
  onProviderChange,
  onManagedModelChange,
  onNativeModelChange,
}: {
  disabled: boolean;
  selectedProviderId: string;
  managedModelId: string;
  nativeModel?: string;
  nativeModels?: Array<{ id: string; name: string }>;
  providers: Array<{ id: string; name: string }>;
  models: Array<{ id: string; name: string; upstreamId: string }>;
  onProviderChange: (providerId: string) => unknown | Promise<unknown>;
  onManagedModelChange: (modelId: string) => unknown | Promise<unknown>;
  onNativeModelChange: (model: string) => unknown | Promise<unknown>;
}) {
  return (
    <section className="client-config-section">
      <div className="config-heading">
        <div>
          <p className="kicker">内置 Code</p>
          <h2>SubAgent 默认模型</h2>
          <p>与 Claude Code 共用本机 Code 配置；可跟随原生或选择第三方模型。</p>
        </div>
      </div>
      <div className="slot-card slot-card--cascade">
        <span>SubAgent 默认模型</span>
        <label>
          <small>供应商</small>
          <select
            aria-label="SubAgent 默认供应商"
            disabled={disabled}
            value={selectedProviderId}
            onChange={(event) => void onProviderChange(event.target.value)}
          >
            <option value="">跟随原生</option>
            {providers.map((provider) => (
              <option key={provider.id} value={provider.id}>
                {provider.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          <small>模型</small>
          <select
            aria-label="SubAgent 默认模型"
            disabled={disabled}
            value={selectedProviderId ? managedModelId : (nativeModel ?? "default")}
            onChange={(event) =>
              void (selectedProviderId
                ? onManagedModelChange(event.target.value)
                : onNativeModelChange(event.target.value))
            }
          >
            {selectedProviderId ? <option value="">选择模型</option> : null}
            {(selectedProviderId
              ? models.map((model) => ({
                  id: model.id,
                  label: `${model.name} · ${model.upstreamId}`,
                }))
              : claudeNativeModelOptions(nativeModel, nativeModels)
            ).map((model) => (
              <option key={model.id} value={model.id}>
                {model.label}
              </option>
            ))}
          </select>
        </label>
      </div>
    </section>
  );
}

const protocolLabels: Record<Protocol, string> = {
  anthropic_messages: "Anthropic Messages",
  open_ai_responses: "OpenAI Responses",
  open_ai_chat_completions: "OpenAI Chat",
  gemini_native: "Gemini Native",
};

const clientLabels: Record<string, string> = {
  claude_code: "Claude Code",
  claude_desktop: "Claude Client",
  codex: "Codex",
  pi: "Pi",
  gemini: "Gemini CLI",
  grok_build: "Grok Build",
  opencode: "OpenCode",
  hermes: "Hermes",
  kimi_code: "Kimi Code",
};

function slug(value: string) {
  return value
    .normalize("NFKD")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function errorMessage(error: unknown) {
  const message =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : String(error);
  if (
    message.includes("Cannot read properties of undefined") &&
    message.includes("invoke")
  ) {
    return "当前页面未连接 GrillForge 桌面后端。请从桌面客户端打开。";
  }
  return message;
}

const unavailableIntegration: IntegrationStatus = {
  snapshotPresent: false,
  takeover: "inactive",
  differences: [],
  managedMainAlias: null,
  nativeModelSlots: {},
  supportedModelSlots: [],
  nativeModels: [],
  nativeModelsError: null,
  nativeCurrentModel: null,
};

const unavailableClaudeCli: ClaudeCliStatus = {
  installed: false,
  path: null,
  version: null,
};

const unavailableClaudeDesktop: ClaudeDesktopStatus = {
  installed: false,
  executablePath: null,
  snapshotPresent: false,
  takeover: "inactive",
  differences: [],
  configuredRoutes: [],
  supportedModelSlots: [],
  nativeModels: [],
  nativeModelsError: null,
  nativeCurrentModel: null,
};

const unavailablePi: PiStatus = {
  installed: false,
  executablePath: null,
  version: null,
  snapshotPresent: false,
  takeover: "inactive",
  configuredModelIds: [],
  defaultModelId: null,
};

const unavailableCodex: CodexStatus = {
  installed: false,
  executablePath: null,
  version: null,
  snapshotPresent: false,
  takeover: "inactive",
  configuredModelId: null,
  currentConfigModel: null,
  currentConfigProvider: null,
  supportedProtocols: [],
  nativeModels: [],
  nativeModelsError: null,
  customAgents: [],
};

const unavailableClient: ClientIntegrationStatus = {
  installed: false,
  executablePath: null,
  version: null,
  snapshotPresent: false,
  takeover: "inactive",
  configuredModelIds: [],
  mainModelId: null,
};

export async function loadOptionalClientSnapshot() {
  const errors: Record<string, string> = {};
  const fixed = await Promise.allSettled([
    invoke<IntegrationStatus>("integration_status"),
    invoke<ClaudeCliStatus>("detect_claude_code"),
    invoke<ClaudeDesktopStatus>("claude_desktop_status"),
    invoke<PiStatus>("pi_status"),
    invoke<PiMcpExtensionStatus>("pi_mcp_extension_status"),
    invoke<CodexStatus>("codex_status"),
    invoke<ClientMcpStatus[]>("client_mcp_statuses"),
  ]);
  const additional = await Promise.allSettled(
    additionalClients.map((client) =>
      invoke<ClientIntegrationStatus>(client.status),
    ),
  );

  function result<T>(
    settled: PromiseSettledResult<T>,
    fallback: T,
    clientId: string,
  ): T {
    if (settled.status === "fulfilled") return settled.value;
    errors[clientId] ??= errorMessage(settled.reason);
    return fallback;
  }

  const integration = result(
    fixed[0] as PromiseSettledResult<IntegrationStatus>,
    unavailableIntegration,
    "claude_code",
  );
  const claudeCli = result(
    fixed[1] as PromiseSettledResult<ClaudeCliStatus>,
    unavailableClaudeCli,
    "claude_code",
  );
  const claudeDesktop = result(
    fixed[2] as PromiseSettledResult<ClaudeDesktopStatus>,
    unavailableClaudeDesktop,
    "claude_desktop",
  );
  const piStatus = result(
    fixed[3] as PromiseSettledResult<PiStatus>,
    unavailablePi,
    "pi",
  );
  const piMcpExtension = result(
    fixed[4] as PromiseSettledResult<PiMcpExtensionStatus>,
    { installed: false, packageSource: "pi-mcp-extension@1.5.0" },
    "pi",
  );
  const codexStatus = result(
    fixed[5] as PromiseSettledResult<CodexStatus>,
    unavailableCodex,
    "codex",
  );
  const mcpStatuses = result(
    fixed[6] as PromiseSettledResult<ClientMcpStatus[]>,
    [],
    "mcp",
  );
  const clientStatuses = Object.fromEntries(
    additionalClients.map((client, index) => [
      client.id,
      result(
        additional[index] as PromiseSettledResult<ClientIntegrationStatus>,
        unavailableClient,
        client.id,
      ),
    ]),
  ) as Record<string, ClientIntegrationStatus>;

  return {
    integration,
    claudeCli,
    claudeDesktop,
    piStatus,
    piMcpExtension,
    codexStatus,
    mcpStatuses,
    clientStatuses,
    errors,
  };
}

function routeSupportLabel(protocol: Protocol) {
  switch (protocol) {
    case "anthropic_messages":
      return "Claude、Pi 及 Anthropic 客户端可直接使用";
    case "open_ai_responses":
      return "Codex 使用 Responses；Claude、Pi 由本地路由转换";
    case "open_ai_chat_completions":
      return "Codex、Claude、Pi 均由本地路由转换";
    case "gemini_native":
      return "Gemini CLI 原生使用；其他客户端暂不开放";
  }
}

function providerProtocol(protocol: ProviderPreset["protocol"]): Protocol {
  if (protocol === "openai_responses") return "open_ai_responses";
  if (protocol === "openai_chat_completions") return "open_ai_chat_completions";
  if (protocol === "gemini_native") return "gemini_native";
  return "anthropic_messages";
}

const usageQueryHosts = new Set([
  "api.deepseek.com",
  "api.stepfun.com",
  "api.siliconflow.cn",
  "api.siliconflow.com",
  "openrouter.ai",
  "api.novita.ai",
  "api.kimi.com",
  "open.bigmodel.cn",
  "api.z.ai",
  "api.minimaxi.com",
  "api.minimax.io",
]);

function supportsUsageQuery(provider: Provider) {
  try {
    return usageQueryHosts.has(new URL(provider.endpoint).hostname);
  } catch {
    return false;
  }
}

function usageItemText(item: UsageSnapshot["items"][number]) {
  if (item.utilizationPercent != null) {
    return `${item.label} 已用 ${item.utilizationPercent.toFixed(1)}%`;
  }
  if (item.remaining != null) {
    return `${item.label} ${Number(item.remaining.toFixed(4))} ${item.unit ?? ""}`.trim();
  }
  return item.label;
}

function resolveEndpoint(
  preset: ProviderPreset | undefined,
  draft: ProviderDraft,
) {
  if (!preset || preset.endpoint.kind === "literal") return draft.endpoint;
  return preset.endpoint.parameters.reduce((endpoint, parameter) => {
    const value =
      draft.parameters[parameter.id] || parameter.default_value || "";
    return endpoint.split(`{${parameter.id}}`).join(value);
  }, preset.endpoint.template);
}

function takeoverLabel(takeover: Takeover) {
  return {
    inactive: "未应用",
    active: "已应用",
    reapply_required: "正在恢复",
    drifted: "配置已被修改",
  }[takeover];
}

function takeoverTone(takeover: Takeover): "neutral" | "good" | "warn" {
  if (takeover === "active") return "good";
  if (takeover === "drifted") return "warn";
  return "neutral";
}

function takeoverActionLabel(takeover: Takeover) {
  return takeover === "drifted" ? "重新应用" : "应用配置";
}

function takeoverDetail(takeover: Takeover, differences: string[] = []) {
  if (takeover === "drifted") {
    return differences.length > 0
      ? `有差异：${differences.join("、")}`
      : "受管配置与上次应用不一致";
  }
  if (takeover === "reapply_required") return "正在恢复路由";
  return takeover === "active" ? "路由运行正常" : "尚未应用";
}

function recommendedUse(model: Model) {
  if (model.capabilities.length === 0)
    return "尚未标注任务能力；可在编辑模型时补充。";
  return `适合 ${model.capabilities.join("、")} 等任务。`;
}

const providerBrandIcons: Array<[string[], string]> = [
  [["anthropic", "claude"], anthropicIcon],
  [["openai", "codex"], openaiIcon],
  [["deepseek"], deepseekIcon],
  [["qwen", "dashscope"], qwenIcon],
  [["alibaba", "aliyun", "bailian"], alibabaIcon],
  [["openrouter"], openrouterIcon],
  [["ollama"], ollamaIcon],
  [["gemini"], geminiIcon],
  [["google"], googleIcon],
  [["azure"], azureIcon],
  [["kimi", "moonshot"], kimiIcon],
  [["minimax"], minimaxIcon],
  [["nvidia"], nvidiaIcon],
  [["siliconflow"], siliconflowIcon],
  [["stepfun"], stepfunIcon],
  [["zhipu", "glm"], zhipuIcon],
  [["xai", "grok"], xaiIcon],
  [["modelscope"], modelscopeIcon],
  [["novita"], novitaIcon],
  [["baidu", "qianfan"], baiduIcon],
  [["doubao"], doubaoIcon],
  [["byteplus", "volcengine"], bytedanceIcon],
  [["hunyuan", "tencent"], hunyuanIcon],
  [["huggingface"], huggingfaceIcon],
  [["cloudflare"], cloudflareIcon],
  [["mistral"], mistralIcon],
  [["cohere"], cohereIcon],
  [["aihubmix"], aihubmixIcon],
  [["longcat"], longcatIcon],
  [["xiaomi", "mimo"], xiaomiIcon],
  [["apinebula"], apinebulaIcon],
  [["atlascloud"], atlascloudIcon],
  [["code0"], code0Icon],
  [["fenno"], fennoIcon],
];

const clientBrandIcons: Record<string, string> = {
  claude_code: anthropicIcon,
  claude_desktop: anthropicIcon,
  codex: openaiIcon,
  pi: piIcon,
  gemini: geminiIcon,
  grok_build: grokIcon,
  opencode: opencodeIcon,
  hermes: hermesIcon,
  kimi_code: kimiIcon,
  dsh: deepseekIcon,
};

function AppLogo({ className = "" }: { className?: string }) {
  return <img className={className} src={grillforgeLogo} alt="" />;
}

function ClientLogo({ clientId, name }: { clientId: string; name: string }) {
  const icon = clientBrandIcons[clientId] ?? grillforgeLogo;
  return (
    <span className="client-mark" title={name}>
      <img src={icon} alt="" />
    </span>
  );
}

function AgentLogo({ sourceClientId }: { sourceClientId: string }) {
  const icon = clientBrandIcons[sourceClientId] ?? grillforgeLogo;
  return (
    <span className="subagent-icon">
      <img src={icon} alt="" />
    </span>
  );
}

function BrandLogo({
  identity,
  name,
  size = "normal",
}: {
  identity: string;
  name: string;
  size?: "normal" | "large";
}) {
  const normalizedIdentity = identity.toLowerCase();
  const icon = providerBrandIcons.find(([aliases]) =>
    aliases.some((alias) => normalizedIdentity.includes(alias)),
  )?.[1];
  return (
    <span className={`provider-logo provider-logo--${size}`} title={name}>
      <img src={icon ?? grillforgeLogo} alt="" />
    </span>
  );
}

function ProviderLogo({
  provider,
  size = "normal",
}: {
  provider: Provider;
  size?: "normal" | "large";
}) {
  return (
    <BrandLogo
      identity={`${provider.id} ${provider.name} ${provider.endpoint}`}
      name={provider.name}
      size={size}
    />
  );
}

function Badge({
  children,
  tone = "neutral",
}: {
  children: ReactNode;
  tone?: "neutral" | "good" | "warn";
}) {
  return <span className={`badge badge--${tone}`}>{children}</span>;
}

type DashboardClient = {
  id: string;
  name: string;
  detail: string;
  tone: "neutral" | "good" | "warn";
  status: string;
};

export function DashboardClientList({
  clients,
  onSelect,
}: {
  clients: DashboardClient[];
  onSelect: (clientId: string) => void;
}) {
  return (
    <div className="dashboard-client-list" data-testid="dashboard-client-list">
      {clients.map((client) => (
        <button key={client.id} onClick={() => onSelect(client.id)}>
          <ClientLogo clientId={client.id} name={client.name} />
          <div>
            <strong>{client.name}</strong>
            <small>{client.detail}</small>
          </div>
          <Badge tone={client.tone}>{client.status}</Badge>
        </button>
      ))}
    </div>
  );
}

export function DashboardMascot() {
  return (
    <div className="agent-orb" aria-hidden="true">
      <span>⌁</span>
      <i>••</i>
    </div>
  );
}

export function SidebarServiceStatus({
  ready,
  mountedClientCount,
}: {
  ready: boolean;
  mountedClientCount: number;
}) {
  return (
    <div className="sidebar-foot">
      <span
        className={`status-dot ${ready ? "status-dot--good" : "status-dot--error"}`}
      />
      <div>
        <strong>{ready ? "GrillForge 已就绪" : "服务不可用"}</strong>
        <small>
          {ready
            ? `${mountedClientCount} 个客户端已挂载扩展 SubAgent`
            : "请重新启动应用"}
        </small>
      </div>
    </div>
  );
}

export function DashboardQuickActions({
  onClients,
  onNewProvider,
  onProviders,
  onNewExtension,
}: {
  onClients: () => void;
  onNewProvider: () => void;
  onProviders: () => void;
  onNewExtension: () => void;
}) {
  return (
    <article className="quick-action-card">
      <p className="kicker">快速操作</p>
      <button onClick={onClients}>
        <span>◇</span>
        <div>
          <strong>配置客户端</strong>
          <small>选择模型和模型池</small>
        </div>
        <i>›</i>
      </button>
      <button onClick={onNewProvider}>
        <span>◎</span>
        <div>
          <strong>添加供应商</strong>
          <small>从供应商预设开始</small>
        </div>
        <i>›</i>
      </button>
      <button onClick={onProviders}>
        <span>◉</span>
        <div>
          <strong>同步供应商模型</strong>
          <small>自动同步或手动添加</small>
        </div>
        <i>›</i>
      </button>
      <button onClick={onNewExtension}>
        <span>✦</span>
        <div>
          <strong>添加扩展 SubAgent</strong>
          <small>复用本机 Coding Agent</small>
        </div>
        <i>›</i>
      </button>
    </article>
  );
}

export function ProviderProtocolFacts({
  provider,
  model,
}: {
  provider: Pick<Provider, "protocolEndpoints">;
  model?: Pick<Model, "nativeProtocols" | "unsupportedNativeProtocols">;
}) {
  const providerProtocols = provider.protocolEndpoints.map(
    (surface) => surface.protocol,
  );
  if (!model) {
    return (
      <>
        {providerProtocols.length > 0 ? (
          providerProtocols.map((protocol) => (
            <Badge key={protocol} tone="good">
              {nativeProtocolLabels[protocol]}
            </Badge>
          ))
        ) : (
          <Badge tone="neutral">尚未探测调用方式</Badge>
        )}
      </>
    );
  }
  return (
    <>
      {model.nativeProtocols.map((protocol) => (
        <Badge key={protocol} tone="good">
          {nativeProtocolLabels[protocol]}
        </Badge>
      ))}
      {model.unsupportedNativeProtocols
        .filter((protocol) => providerProtocols.includes(protocol))
        .map((protocol) => (
          <Badge key={`unsupported-${protocol}`} tone="warn">
            不支持 {nativeProtocolLabels[protocol]}
          </Badge>
        ))}
    </>
  );
}

export function ClientDetectionStatus({
  name,
  installed,
  detail,
  error,
}: {
  name: string;
  installed: boolean;
  detail: string;
  error?: string;
}) {
  return (
    <>
      <div>
        <strong>{name}</strong>
        <small
          className={error ? "client-detection-error" : undefined}
          role={error ? "alert" : undefined}
        >
          {error ?? detail}
        </small>
      </div>
      <Badge tone={error ? "warn" : installed ? "good" : "warn"}>
        {error ? "检测失败" : installed ? "可用" : "未安装"}
      </Badge>
    </>
  );
}

function Toggle({
  checked,
  label,
  disabled,
  onChange,
}: {
  checked: boolean;
  label: string;
  disabled?: boolean;
  onChange: () => void;
}) {
  return (
    <button
      className={`toggle ${checked ? "toggle--on" : ""}`}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={onChange}
    >
      <span />
    </button>
  );
}

function SectionTitle({
  kicker,
  title,
  detail,
  action,
}: {
  kicker: string;
  title: string;
  detail: string;
  action?: ReactNode;
}) {
  return (
    <header className="section-title">
      <div>
        <p className="kicker">{kicker}</p>
        <h1>{title}</h1>
        <p className="section-detail">{detail}</p>
      </div>
      {action}
    </header>
  );
}

export function PiMcpInstallControl({
  disabled,
  label,
  buttonClassName = "button",
  onInstall,
}: {
  disabled: boolean;
  label: string;
  buttonClassName?: string;
  onInstall: () => Promise<void>;
}) {
  const [confirming, setConfirming] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState("");

  async function confirmInstall() {
    if (installing) return;
    setInstalling(true);
    setInstallError("");
    try {
      await onInstall();
      setConfirming(false);
    } catch (cause) {
      setConfirming(false);
      setInstallError(errorMessage(cause));
    } finally {
      setInstalling(false);
    }
  }

  return (
    <>
      <button
        className={buttonClassName}
        type="button"
        disabled={disabled || installing}
        onClick={() => {
          setInstallError("");
          setConfirming(true);
        }}
      >
        {installing ? "正在安装…" : label}
      </button>
      {installError && (
        <span className="pi-mcp-install-error" role="alert">
          {installError}
        </span>
      )}
      {confirming && (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={() => {
            if (!installing) setConfirming(false);
          }}
        >
          <section
            className="pi-mcp-install-confirm"
            role="dialog"
            aria-modal="true"
            aria-labelledby="pi-mcp-install-title"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <p className="kicker">社区扩展</p>
            <h2 id="pi-mcp-install-title">安装 Pi MCP 扩展</h2>
            <p>
              将通过当前 Pi CLI 安装 pi-mcp-extension 1.5.0。该扩展可访问本机文件和命令。
            </p>
            <footer>
              <button
                className="button button--secondary"
                type="button"
                disabled={installing}
                onClick={() => setConfirming(false)}
              >
                取消
              </button>
              <button
                className="button"
                type="button"
                disabled={installing}
                onClick={() => void confirmInstall()}
              >
                {installing ? "正在安装…" : "确认安装"}
              </button>
            </footer>
          </section>
        </div>
      )}
    </>
  );
}

function App() {
  const [view, setView] = useState<View>("overview");
  const [selectedClient, setSelectedClient] = useState<string | null>(null);
  const [clientTab, setClientTab] = useState<ClientTab>("slots");
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);
  const [state, setState] = useState<ControlPlaneState | null>(null);
  const [integration, setIntegration] = useState<IntegrationStatus | null>(
    null,
  );
  const [claudeCli, setClaudeCli] = useState<ClaudeCliStatus | null>(null);
  const [claudeDesktop, setClaudeDesktop] =
    useState<ClaudeDesktopStatus | null>(null);
  const [piStatus, setPiStatus] = useState<PiStatus | null>(null);
  const [piMcpExtension, setPiMcpExtension] =
    useState<PiMcpExtensionStatus | null>(null);
  const [codexStatus, setCodexStatus] = useState<CodexStatus | null>(null);
  const [clientStatuses, setClientStatuses] = useState<Record<
    string,
    ClientIntegrationStatus
  > | null>(null);
  const [clientStatusErrors, setClientStatusErrors] = useState<
    Record<string, string>
  >({});
  const [mcpStatuses, setMcpStatuses] = useState<Record<string, ClientMcpStatus>>(
    {},
  );
  const [catalog, setCatalog] = useState<ProviderPresetCatalog | null>(null);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [restartPromptClient, setRestartPromptClient] =
    useState<DesktopRestartClient | null>(null);
  const [restartClientError, setRestartClientError] = useState("");
  const [showProviderForm, setShowProviderForm] = useState(false);
  const [providerPickerOpen, setProviderPickerOpen] = useState(false);
  const [presetSearch, setPresetSearch] = useState("");
  const [showModelForm, setShowModelForm] = useState(false);
  const [providerDraft, setProviderDraft] =
    useState<ProviderDraft>(EMPTY_PROVIDER);
  const [modelDraft, setModelDraft] = useState<ModelDraft>(EMPTY_MODEL);
  const [editingProviderId, setEditingProviderId] = useState<string | null>(
    null,
  );
  const [editingModelId, setEditingModelId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget>(null);
  const [connections, setConnections] = useState<
    Record<string, ConnectionResult>
  >({});
  const [providerUsage, setProviderUsage] = useState<
    Record<string, UsageSnapshot>
  >({});
  const [providerSearch, setProviderSearch] = useState("");
  const [managingProviderId, setManagingProviderId] = useState<string | null>(
    null,
  );
  const [modelSearch, setModelSearch] = useState("");
  const [slotProviderSelections, setSlotProviderSelections] = useState<
    Record<string, string>
  >({});
  const [claudeMainProviderSelection, setClaudeMainProviderSelection] =
    useState("");
  const [desktopSlotProviderSelections, setDesktopSlotProviderSelections] =
    useState<Record<string, string>>({});
  const [piDefaultProviderSelection, setPiDefaultProviderSelection] =
    useState("");
  const [codexProviderSelection, setCodexProviderSelection] = useState("");
  const [codexAgentProviderSelections, setCodexAgentProviderSelections] =
    useState<Record<string, string>>({});
  const [clientProviderSelections, setClientProviderSelections] = useState<
    Record<string, string>
  >({});
  const [refreshingClients, setRefreshingClients] = useState(false);
  const [localAgents, setLocalAgents] = useState<LocalAgent[]>([]);
  const [discoveringLocalAgents, setDiscoveringLocalAgents] = useState(false);
  const [localAgentError, setLocalAgentError] = useState("");
  const [extensionDraft, setExtensionDraft] =
    useState<ExtensionSubAgentDraft>(EMPTY_EXTENSION_SUBAGENT);
  const [editingExtensionId, setEditingExtensionId] = useState<string | null>(
    null,
  );
  const [showExtensionForm, setShowExtensionForm] = useState(false);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const [loaded, presets] = await Promise.all([
          invoke<ControlPlaneState>("load_state"),
          invoke<ProviderPresetCatalog>("provider_presets"),
        ]);
        if (!active) return;
        setState(loaded);
        setCatalog(presets);

        const optional = await loadOptionalClientSnapshot();
        if (!active) return;
        setIntegration(optional.integration);
        setClaudeCli(optional.claudeCli);
        setClaudeDesktop(optional.claudeDesktop);
        setPiStatus(optional.piStatus);
        setPiMcpExtension(optional.piMcpExtension);
        setCodexStatus(optional.codexStatus);
        setMcpStatuses(
          Object.fromEntries(
            optional.mcpStatuses.map((status) => [status.clientId, status]),
          ),
        );
        setClientStatuses(optional.clientStatuses);
        setClientStatusErrors(optional.errors);
      } catch (cause) {
        if (active) setError(errorMessage(cause));
      } finally {
        if (active) setLoading(false);
      }
    })();
    return () => {
      active = false;
    };
  }, []);

  const providers = state?.providers ?? [];
  const models = state?.models ?? [];
  const enabledProviderIds = useMemo(
    () =>
      new Set(
        providers
          .filter((provider) => provider.enabled)
          .map((provider) => provider.id),
      ),
    [providers],
  );
  const availableModels = models.filter((model) =>
    enabledProviderIds.has(model.providerId),
  );
  const gatewayProviders = providers.filter((provider) => provider.enabled);
  const extensionSubagents = state?.extensionSubagents ?? [];
  const localAgentRuntimes = Array.from(
    new Set(localAgents.map((agent) => agent.runtime)),
  ).sort();
  const selectedSourceAgents = localAgents.filter(
    (agent) => agent.runtime === extensionDraft.sourceClientId,
  );
  const selectedPreset = catalog?.presets.find(
    (preset) => preset.id === providerDraft.presetId,
  );
  const providerEndpoint = resolveEndpoint(selectedPreset, providerDraft);
  const editingProvider = providers.find(
    (provider) => provider.id === editingProviderId,
  );
  const selectedModel =
    models.find((model) => model.id === selectedModelId) ?? null;
  const selectedModelProvider = selectedModel
    ? providers.find((provider) => provider.id === selectedModel.providerId)
    : null;
  const managingProvider =
    providers.find((provider) => provider.id === managingProviderId) ?? null;
  const managingProviderModels = models.filter(
    (model) => model.providerId === managingProviderId,
  );
  const visibleProviders = providers.filter((provider) => {
    const query = providerSearch.trim().toLowerCase();
    return (
      !query ||
      [
        provider.name,
        provider.id,
        provider.endpoint,
        protocolLabels[provider.protocol],
      ].some((value) => value.toLowerCase().includes(query))
    );
  });
  const visiblePresets = (catalog?.presets ?? []).filter((preset) => {
    const query = presetSearch.trim().toLowerCase();
    return (
      !query ||
      [preset.name, preset.id, ...preset.suggested_models].some((value) =>
        value.toLowerCase().includes(query),
      )
    );
  });

  function modelProviderId(modelId: string | undefined) {
    return models.find((model) => model.id === modelId)?.providerId ?? "";
  }

  function modelsForProvider(providerId: string) {
    return availableModels.filter((model) => model.providerId === providerId);
  }

  function reportError(message: string) {
    setNotice("");
    setError(message);
  }

  function begin(command: string) {
    if (pending) return false;
    setPending(command);
    setError("");
    setNotice("");
    return true;
  }

  async function commit(
    command: string,
    args: Record<string, unknown>,
    success: string,
  ) {
    if (!begin(command)) return false;
    try {
      const persisted = await invoke<ControlPlaneState>(command, args);
      setState(persisted);
      setNotice(success);
      return true;
    } catch (cause) {
      reportError(errorMessage(cause));
      return false;
    } finally {
      setPending("");
    }
  }

  async function runIntegration(
    command: "apply_claude_code" | "disable_claude_code",
    success: string,
  ) {
    if (!begin(command)) return false;
    try {
      const persisted = await invoke<IntegrationStatus>(command);
      setIntegration(persisted);
      setNotice(success);
      return true;
    } catch (cause) {
      reportError(errorMessage(cause));
      return false;
    } finally {
      setPending("");
    }
  }

  async function runDesktopIntegration(
    command: ClaudeDesktopIntegrationCommand,
    success: string,
  ) {
    if (!begin(command)) return false;
    try {
      const persisted = await invoke<ClaudeDesktopStatus>(command);
      setClaudeDesktop(persisted);
      setNotice(success);
      if (claudeClientRestartRequired(command)) {
        setRestartClientError("");
        setRestartPromptClient("claude_desktop");
      }
      return true;
    } catch (cause) {
      reportError(errorMessage(cause));
      return false;
    } finally {
      setPending("");
    }
  }

  async function runPiIntegration(
    command: "apply_pi" | "disable_pi",
    success: string,
  ) {
    if (!begin(command)) return false;
    try {
      const persisted = await invoke<PiStatus>(command);
      setPiStatus(persisted);
      setNotice(success);
      return true;
    } catch (cause) {
      reportError(errorMessage(cause));
      return false;
    } finally {
      setPending("");
    }
  }

  async function installPiMcpExtension() {
    if (!begin("install_pi_mcp_extension")) {
      throw new Error("另一项操作正在进行，请完成后重试。");
    }
    try {
      const status = await invoke<PiMcpExtensionStatus>(
        "install_pi_mcp_extension",
      );
      setPiMcpExtension(status);
      setNotice("Pi MCP 扩展已安装，现在可以绑定扩展 SubAgent。");
    } finally {
      setPending("");
    }
  }

  async function runCodexIntegration(
    command: "apply_codex" | "disable_codex",
    success: string,
  ) {
    if (!begin(command)) return false;
    try {
      const persisted = await invoke<CodexStatus>(command);
      setCodexStatus(persisted);
      setNotice(success);
      return true;
    } catch (cause) {
      reportError(errorMessage(cause));
      return false;
    } finally {
      setPending("");
    }
  }

  async function runClientIntegration(
    clientId: string,
    command: string,
    success: string,
  ) {
    if (!begin(command)) return false;
    try {
      const persisted = await invoke<ClientIntegrationStatus>(command);
      setClientStatuses((current) => ({
        ...(current ?? {}),
        [clientId]: persisted,
      }));
      setNotice(success);
      return true;
    } catch (cause) {
      reportError(errorMessage(cause));
      return false;
    } finally {
      setPending("");
    }
  }

  async function refreshClients() {
    if (refreshingClients) return;
    setRefreshingClients(true);
    try {
      const loaded = await loadOptionalClientSnapshot();
      if (!loaded.errors.claude_code) {
        setIntegration(loaded.integration);
        setClaudeCli(loaded.claudeCli);
      }
      if (!loaded.errors.claude_desktop)
        setClaudeDesktop(loaded.claudeDesktop);
      if (!loaded.errors.pi) {
        setPiStatus(loaded.piStatus);
        setPiMcpExtension(loaded.piMcpExtension);
      }
      if (!loaded.errors.codex) setCodexStatus(loaded.codexStatus);
      if (!loaded.errors.mcp) {
        setMcpStatuses(
          Object.fromEntries(
            loaded.mcpStatuses.map((status) => [status.clientId, status]),
          ),
        );
      }
      setClientStatuses((current) => {
        const next = { ...(current ?? {}) };
        for (const client of additionalClients) {
          if (!loaded.errors[client.id])
            next[client.id] = loaded.clientStatuses[client.id];
        }
        return next;
      });
      setClientStatusErrors(loaded.errors);
    } catch (cause) {
      reportError(errorMessage(cause));
    } finally {
      setRefreshingClients(false);
    }
  }

  async function refreshLocalAgents() {
    if (discoveringLocalAgents) return;
    setDiscoveringLocalAgents(true);
    setLocalAgentError("");
    try {
      const discovered = await invoke<LocalAgentDiscovery>(
        "discover_local_agents",
      );
      setLocalAgents(discovered.agents);
      setLocalAgentError(
        discovered.errors
          .map(
            ({ runtime, message }) =>
              `${clientLabels[runtime] ?? runtime}：${message}`,
          )
          .join("；"),
      );
    } catch (cause) {
      setLocalAgentError(errorMessage(cause));
    } finally {
      setDiscoveringLocalAgents(false);
    }
  }

  async function testConnection(model: Model) {
    if (!begin(`test:${model.id}`)) return;
    try {
      const result = await invoke<ConnectionResult>("test_model_connection", {
        id: model.id,
      });
      setConnections((current) => ({ ...current, [model.id]: result }));
      setNotice(`${model.name} （${result.upstreamId}）连接测试通过。`);
    } catch (cause) {
      reportError(errorMessage(cause));
    } finally {
      setPending("");
    }
  }

  async function queryProviderUsage(provider: Provider) {
    if (!begin(`usage:${provider.id}`)) return;
    try {
      const snapshot = await invoke<UsageSnapshot>("query_provider_usage", {
        id: provider.id,
      });
      setProviderUsage((current) => ({
        ...current,
        [provider.id]: snapshot,
      }));
      setNotice(`${provider.name} 用量已更新。`);
    } catch (cause) {
      reportError(errorMessage(cause));
    } finally {
      setPending("");
    }
  }

  function selectView(next: View) {
    setView(next);
    setError("");
    setNotice("");
    setDeleteTarget(null);
    if (next === "clients") void refreshClients();
    if (next === "extension_subagents") void refreshLocalAgents();
    requestAnimationFrame(() => {
      document
        .querySelector<HTMLElement>(".workspace")
        ?.scrollTo({ top: 0, left: 0 });
    });
  }

  function selectClient(clientId: string) {
    setSelectedClient(clientId);
    setClientTab("slots");
  }

  function closeProviderForm() {
    setShowProviderForm(false);
    setEditingProviderId(null);
    setProviderDraft(EMPTY_PROVIDER);
  }

  function openNewProvider() {
    closeModelForm();
    setEditingProviderId(null);
    setProviderDraft(EMPTY_PROVIDER);
    setPresetSearch("");
    setProviderPickerOpen(true);
    setShowProviderForm(false);
  }

  function chooseCustomProvider() {
    setProviderDraft(EMPTY_PROVIDER);
    setProviderPickerOpen(false);
    setShowProviderForm(true);
  }

  function chooseGeminiProvider() {
    setProviderDraft({
      ...EMPTY_PROVIDER,
      name: "Google Gemini API",
      protocol: "gemini_native",
      endpoint: "https://generativelanguage.googleapis.com",
      endpointMode: "base_url",
      apiKeyPlacement: "x_api_key",
      modelsUrl: "https://generativelanguage.googleapis.com/v1beta/models",
    });
    setProviderPickerOpen(false);
    setShowProviderForm(true);
  }

  function chooseAnthropicProvider() {
    setProviderDraft({
      ...EMPTY_PROVIDER,
      name: "Anthropic API",
      protocol: "anthropic_messages",
      endpoint: "https://api.anthropic.com",
      endpointMode: "base_url",
      apiKeyPlacement: "x_api_key",
      modelsUrl: "https://api.anthropic.com/v1/models",
    });
    setProviderPickerOpen(false);
    setShowProviderForm(true);
  }

  function chooseOpenAiProvider() {
    setProviderDraft({
      ...EMPTY_PROVIDER,
      name: "OpenAI API",
      protocol: "open_ai_responses",
      endpoint: "https://api.openai.com/v1",
      endpointMode: "base_url",
      apiKeyPlacement: "bearer",
      modelsUrl: "https://api.openai.com/v1/models",
    });
    setProviderPickerOpen(false);
    setShowProviderForm(true);
  }

  function chooseProviderPreset(id: string) {
    selectPreset(id);
    setProviderPickerOpen(false);
    setShowProviderForm(true);
  }

  function editProvider(provider: Provider) {
    closeModelForm();
    setProviderPickerOpen(false);
    setEditingProviderId(provider.id);
    setProviderDraft({
      presetId: "",
      name: provider.name,
      protocol: provider.protocol,
      endpoint: provider.endpoint,
      endpointMode: provider.endpointMode,
      apiKeyPlacement: provider.apiKeyPlacement,
      apiKey: "",
      modelsUrl: provider.modelsUrl ?? "",
      parameters: {},
    });
    setShowProviderForm(true);
  }

  function closeModelForm() {
    setShowModelForm(false);
    setEditingModelId(null);
    setModelDraft(EMPTY_MODEL);
  }

  function openNewModel(providerId = managingProviderId ?? "") {
    closeProviderForm();
    setEditingModelId(null);
    setModelDraft({ ...EMPTY_MODEL, providerId });
    setShowModelForm(true);
  }

  function editModel(model: Model) {
    closeProviderForm();
    setEditingModelId(model.id);
    setModelDraft({
      name: model.name,
      upstreamId: model.upstreamId,
      providerId: model.providerId,
      capabilities: model.capabilities.join(", "),
      protocolCapabilities: model.protocolCapabilities ?? [],
      nativeProtocols: model.nativeProtocols ?? [],
      contextWindow: model.contextWindow ? String(model.contextWindow) : "",
    });
    setShowModelForm(true);
  }

  function openProviderModels(provider: Provider) {
    closeProviderForm();
    closeModelForm();
    setManagingProviderId(provider.id);
    setModelSearch("");
  }

  function closeProviderModels() {
    closeModelForm();
    setManagingProviderId(null);
  }

  async function syncProviderModels(provider: Provider) {
    await commit(
      "sync_provider_models",
      { providerId: provider.id },
      `${provider.name} 的模型与协议支持已同步。`,
    );
  }

  function toggleProtocolFeature(id: ProtocolCapability) {
    setModelDraft((current) => ({
      ...current,
      protocolCapabilities: current.protocolCapabilities.includes(id)
        ? current.protocolCapabilities.filter((feature) => feature !== id)
        : [...current.protocolCapabilities, id],
    }));
  }

  function selectPreset(id: string) {
    const preset = catalog?.presets.find((item) => item.id === id);
    if (!preset) {
      setProviderDraft((current) => ({
        ...current,
        presetId: "",
        parameters: {},
      }));
      return;
    }
    const parameters =
      preset.endpoint.kind === "parameterized"
        ? Object.fromEntries(
            preset.endpoint.parameters.map((parameter) => [
              parameter.id,
              parameter.default_value || "",
            ]),
          )
        : {};
    setProviderDraft({
      presetId: preset.id,
      name: preset.name,
      protocol: providerProtocol(preset.protocol),
      endpoint: preset.endpoint.kind === "literal" ? preset.endpoint.url : "",
      endpointMode: "base_url",
      apiKeyPlacement: preset.auth,
      apiKey: "",
      modelsUrl: preset.models_url ?? "",
      parameters,
    });
  }

  async function toggleProvider(provider: Provider) {
    const input: ProviderInput = {
      id: provider.id,
      name: provider.name,
      protocol: provider.protocol,
      endpoint: provider.endpoint,
      endpointMode: provider.endpointMode,
      apiKeyPlacement: provider.apiKeyPlacement,
      apiKey: null,
      enabled: !provider.enabled,
      modelsUrl: provider.modelsUrl,
    };
    await commit(
      "save_provider",
      { input },
      `${provider.name} 已${provider.enabled ? "停用" : "启用"}。`,
    );
  }

  async function addProvider(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const name = providerDraft.name.trim();
    const id = editingProviderId ?? slug(name);
    if (!id) return reportError("供应商名称必须能生成小写拉丁字母标识。");
    if (selectedPreset?.endpoint.kind === "parameterized") {
      const missing = selectedPreset.endpoint.parameters.find(
        (parameter) =>
          parameter.required &&
          !(providerDraft.parameters[parameter.id] || parameter.default_value),
      );
      if (missing) return reportError(`${missing.label} 不能为空。`);
    }
    if (
      providerDraft.apiKeyPlacement !== "none" &&
      !providerDraft.apiKey.trim() &&
      (!editingProvider || !editingProvider.credentialSet)
    ) {
      return reportError("供应商 API Key 不能为空。");
    }
    const input: ProviderInput = {
      id,
      name,
      protocol: providerDraft.protocol,
      endpoint: providerEndpoint.trim(),
      endpointMode: providerDraft.endpointMode,
      apiKeyPlacement: providerDraft.apiKeyPlacement,
      apiKey:
        providerDraft.apiKeyPlacement === "none" || !providerDraft.apiKey.trim()
          ? null
          : providerDraft.apiKey,
      enabled: true,
      modelsUrl: providerDraft.modelsUrl.trim() || null,
    };
    const command = editingProviderId
      ? "update_provider"
      : "save_provider_with_model_check";
    if (
      await commit(
        command,
        { input },
        `${name} 已${editingProviderId ? "更新" : "保存并完成模型检查"}。`,
      )
    ) {
      closeProviderForm();
    }
  }

  async function addModel(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const name = modelDraft.name.trim();
    const id = editingModelId ?? slug(name);
    if (!id) return reportError("模型名称必须能生成小写拉丁字母标识。");
    const declared = modelDraft.contextWindow.trim();
    // Left blank the model stays unknown; a client then keeps its own default
    // instead of being handed a number nobody verified.
    let contextWindow: number | undefined;
    if (declared) {
      contextWindow = Number(declared);
      if (!Number.isSafeInteger(contextWindow) || contextWindow <= 0)
        return reportError("上下文长度必须是正整数 token 数，留空表示未知。");
    }
    const input = {
      id,
      name,
      upstreamId: modelDraft.upstreamId.trim(),
      providerId: modelDraft.providerId,
      capabilities: modelDraft.capabilities
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
      protocolCapabilities: modelDraft.protocolCapabilities,
      nativeProtocols: modelDraft.nativeProtocols,
      contextWindow,
    };
    if (input.nativeProtocols.length === 0)
      return reportError("请至少选择一种模型原生支持的 API 协议。");
    const command = editingModelId
      ? "update_model_with_native_protocols"
      : "save_model_with_native_protocols";
    if (
      await commit(
        command,
        { input },
        `${name} 已${editingModelId ? "更新" : "保存"}。`,
      )
    ) {
      closeModelForm();
    }
  }

  function openNewExtension() {
    setEditingExtensionId(null);
    setExtensionDraft(EMPTY_EXTENSION_SUBAGENT);
    setShowExtensionForm(true);
    if (localAgents.length === 0) void refreshLocalAgents();
  }

  function editExtension(extension: ExtensionSubAgent) {
    setEditingExtensionId(extension.id);
    setExtensionDraft({
      name: extension.name,
      sourceClientId: extension.sourceClientId,
      sourceAgentId: extension.sourceAgentId,
      providerId: modelProviderId(extension.modelId ?? undefined),
      modelId: extension.modelId ?? "",
      capabilities: extension.capabilities.join(", "),
    });
    setShowExtensionForm(true);
    if (localAgents.length === 0) void refreshLocalAgents();
  }

  function closeExtensionForm() {
    setEditingExtensionId(null);
    setExtensionDraft(EMPTY_EXTENSION_SUBAGENT);
    setShowExtensionForm(false);
  }

  async function saveExtension(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const name = extensionDraft.name.trim();
    if (!extensionDraft.sourceClientId || !extensionDraft.sourceAgentId) {
      return reportError("请选择本机 Agent 来源。");
    }
    const input = {
      id: editingExtensionId ?? "",
      name,
      sourceClientId: extensionDraft.sourceClientId,
      sourceAgentId: extensionDraft.sourceAgentId,
      modelId: extensionDraft.modelId || null,
      capabilities: extensionDraft.capabilities
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
    };
    const command = editingExtensionId
      ? "update_extension_subagent"
      : "save_extension_subagent";
    if (
      await commit(
        command,
        { input },
        `${name} 已${editingExtensionId ? "更新" : "添加"}。`,
      )
    ) {
      closeExtensionForm();
    }
  }

  async function setExtensionBinding(
    clientId: string,
    extension: ExtensionSubAgent,
    enabled: boolean,
  ) {
    await commit(
      "set_client_extension_binding",
      { clientId, extensionSubagentId: extension.id, enabled },
      `${extension.name} 已${enabled ? "允许" : "停止"}供该客户端使用。`,
    );
  }

  async function setClientMcpMounted(clientId: string, mounted: boolean) {
    const command = mounted ? "mount_client_mcp" : "unmount_client_mcp";
    if (!begin(`${command}:${clientId}`)) return;
    try {
      const status = await invoke<ClientMcpStatus>(command, { clientId });
      setMcpStatuses((current) => ({ ...current, [clientId]: status }));
      setState((current) =>
        current
          ? {
              ...current,
              mcpMountedClientIds: status.desiredMounted
                ? Array.from(
                    new Set([...current.mcpMountedClientIds, clientId]),
                  ).sort()
                : current.mcpMountedClientIds.filter((id) => id !== clientId),
            }
          : current,
      );
      setNotice(`扩展 SubAgent 已${mounted ? "挂载" : "卸载"}。`);
      if (desktopClientRestartAfterMcpChange(clientId)) {
        setRestartClientError("");
        setRestartPromptClient(clientId as DesktopRestartClient);
      }
    } catch (cause) {
      reportError(errorMessage(cause));
    } finally {
      setPending("");
    }
  }

  async function restartDesktopClient() {
    if (!restartPromptClient) return;
    const clientName = restartPromptClient === "codex" ? "Codex" : "Claude Client";
    const command =
      restartPromptClient === "codex"
        ? "restart_codex_client"
        : "restart_claude_client";
    if (!begin(command)) return;
    try {
      await invoke(command);
      setRestartPromptClient(null);
      setNotice(`${clientName} 已重新打开，最新配置将在新进程中加载。`);
    } catch (cause) {
      setRestartClientError(errorMessage(cause));
    } finally {
      setPending("");
    }
  }

  function mcpMountActions(clientId: string) {
    const status = mcpStatuses[clientId];
    const mounted = Boolean(status?.mounted);
    const needsReapply = Boolean(status?.configurationChanged);
    const copy = extensionMountCopy(status);
    return (
      <>
        <Badge
          tone={mounted ? "good" : status?.configurationChanged ? "warn" : "neutral"}
        >
          {copy.badge}
        </Badge>
        <button
          className={
            mounted && !needsReapply ? "button button--secondary" : "button"
          }
          disabled={Boolean(pending)}
          onClick={() =>
            void setClientMcpMounted(
              clientId,
              needsReapply || !(mounted || Boolean(status?.desiredMounted)),
            )
          }
        >
          {copy.action}
        </button>
      </>
    );
  }

  function extensionBindingsFor(clientId: string, clientName: string) {
    const enabledIds = state?.clientExtensionSubagentIds[clientId] ?? [];
    return (
      <section className="subagent-section extension-bindings">
        <div className="config-heading">
          <div>
            <p className="kicker">扩展 SubAgent</p>
            <h2>{clientName} 可用的扩展 SubAgent</h2>
            <p>已挂载时，绑定变化会实时同步到该客户端。</p>
          </div>
          <div className="agent-actions">
            {mcpMountActions(clientId)}
            <button
              className="button button--secondary"
              onClick={() => selectView("extension_subagents")}
            >
              管理扩展 SubAgent
            </button>
          </div>
        </div>
        {extensionSubagents.length === 0 ? (
          <div className="subagent-empty">
            <strong>还没有扩展 SubAgent</strong>
            <button
              className="text-link"
              onClick={() => selectView("extension_subagents")}
            >
              前往添加 →
            </button>
          </div>
        ) : (
          <div className="subagent-list">
            {extensionSubagents.map((extension) => {
              const enabled = enabledIds.includes(extension.id);
              const model = models.find((item) => item.id === extension.modelId);
              return (
                <article className="subagent-card" key={extension.id}>
                  <AgentLogo sourceClientId={extension.sourceClientId} />
                  <div className="subagent-main">
                    <div>
                      <h3>{extension.name}</h3>
                      <code>{extension.sourceAgentId}</code>
                    </div>
                    <p>
                      {clientLabels[extension.sourceClientId] ??
                        extension.sourceClientId}
                      {" · "}
                      {model?.name ?? "跟随来源原生"}
                    </p>
                  </div>
                  <Toggle
                    checked={enabled}
                    disabled={Boolean(pending)}
                    label={`允许 ${clientName} 使用 ${extension.name}`}
                    onChange={() =>
                      void setExtensionBinding(clientId, extension, !enabled)
                    }
                  />
                </article>
              );
            })}
          </div>
        )}
      </section>
    );
  }

  async function confirmDelete(target: NonNullable<DeleteTarget>) {
    const item =
      target.kind === "provider"
        ? providers.find((provider) => provider.id === target.id)
        : models.find((model) => model.id === target.id);
    const command =
      target.kind === "provider" ? "delete_provider" : "delete_model";
    if (
      await commit(
        command,
        { id: target.id },
        `${item?.name ?? target.id} 已删除。`,
      )
    ) {
      setDeleteTarget(null);
    }
  }

  if (loading) {
    return (
      <main className="startup">
        <span className="brand-mark"><AppLogo /></span>
        <strong>正在加载 GrillForge…</strong>
      </main>
    );
  }

  const ready =
    state &&
    integration &&
    claudeCli &&
    claudeDesktop &&
    piStatus &&
    codexStatus &&
    clientStatuses &&
    catalog;
  const integrationTone = takeoverTone(integration?.takeover ?? "inactive");
  const desktopTone = takeoverTone(claudeDesktop?.takeover ?? "inactive");
  const piTone = takeoverTone(piStatus?.takeover ?? "inactive");
  const codexTone = takeoverTone(codexStatus?.takeover ?? "inactive");
  const selectedAdditionalClient =
    additionalClients.find((client) => client.id === selectedClient) ?? null;
  const selectedAdditionalStatus =
    selectedAdditionalClient && clientStatuses
      ? clientStatuses[selectedAdditionalClient.id]
      : null;
  const selectedClientConfiguration =
    selectedAdditionalClient && state
      ? state.clientConfigurations[selectedAdditionalClient.id]
      : null;
  const selectedClientProviders = selectedAdditionalClient
    ? providers.filter((provider) => provider.enabled)
    : [];
  const dashboardClients: DashboardClient[] = ready
    ? [
        {
          id: "claude_code",
          name: "Claude Code",
          detail: `${Object.keys(state.modelSlots).length} 个槽位 · ${(state.clientExtensionSubagentIds.claude_code ?? []).length} 个扩展`,
          tone: integrationTone,
          status: takeoverLabel(integration.takeover),
        },
        {
          id: "claude_desktop",
          name: "Claude Client",
          detail: `${Object.keys(state.claudeDesktopModelSlots).length} 个对话角色`,
          tone: desktopTone,
          status: takeoverLabel(claudeDesktop.takeover),
        },
        {
          id: "codex",
          name: "Codex",
          detail: state.codexMainModelId ? "已设默认模型" : "跟随原生",
          tone: codexTone,
          status: takeoverLabel(codexStatus.takeover),
        },
        {
          id: "pi",
          name: "Pi",
          detail: `${state.piEnabledModelIds.length} 个可用模型`,
          tone: piTone,
          status: takeoverLabel(piStatus.takeover),
        },
        ...additionalClients.map((client) => {
          const status = clientStatuses[client.id];
          const configuration = state.clientConfigurations[client.id];
          return {
            id: client.id,
            name: client.name,
            detail: configuration.mainModelId
              ? `${configuration.enabledModelIds.length || 1} 个模型`
              : "跟随原生",
            tone: takeoverTone(status.takeover),
            status: takeoverLabel(status.takeover),
          };
        }),
      ]
    : [];
  const mountedClientCount = Object.values(mcpStatuses).filter(
    (status) => status.mounted,
  ).length;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark"><AppLogo /></span>
          <div>
            <strong>GrillForge</strong>
            <small>Coding Agent 配置中心</small>
          </div>
        </div>
        <nav aria-label="主导航">
          {views.map((item) => (
            <button
              key={item.id}
              className={view === item.id ? "active" : ""}
              onClick={() => selectView(item.id)}
            >
              <span>{item.icon}</span>
              {item.label}
            </button>
          ))}
        </nav>
        <SidebarServiceStatus
          ready={Boolean(ready)}
          mountedClientCount={mountedClientCount}
        />
      </aside>

      <main className={`workspace workspace--${view}`}>
        {(error || notice) && (
          <div
            className={`feedback ${error ? "feedback--error" : "feedback--notice"}`}
            role={error ? "alert" : "status"}
            aria-live={error ? "assertive" : "polite"}
            aria-atomic="true"
          >
            <strong>{error ? "操作已停止" : "操作完成"}</strong>
            <span>{error || notice}</span>
            <button
              onClick={() => {
                setError("");
                setNotice("");
              }}
              aria-label="关闭提示"
            >
              ×
            </button>
          </div>
        )}
        {!ready ? (
          <section className="load-failure">
            <p className="kicker">加载失败</p>
            <h1>无法加载应用配置。</h1>
            <p>请处理上方错误后重新启动 GrillForge。</p>
          </section>
        ) : (
          <>
            {view === "overview" && (
              <>
                <SectionTitle
                  kicker="控制中心"
                  title="欢迎回来 👋"
                  detail="在这里统一管理 Coding Agent 客户端的模型配置。"
                  action={
                    <button
                      className="button"
                      onClick={() => {
                        selectView("clients");
                        selectClient("claude_code");
                      }}
                    >
                      客户端设置
                    </button>
                  }
                />
                <section className="overview-hero-grid">
                  <article className="active-client-card active-client-card--multi">
                    <div>
                      <p className="kicker">客户端</p>
                      <h2>客户端配置</h2>
                      <DashboardClientList
                        clients={dashboardClients}
                        onSelect={(clientId) => {
                          selectView("clients");
                          selectClient(clientId);
                        }}
                      />
                    </div>
                    <DashboardMascot />
                  </article>
                  <DashboardQuickActions
                    onClients={() => selectView("clients")}
                    onNewProvider={openNewProvider}
                    onProviders={() => selectView("providers")}
                    onNewExtension={() => {
                      selectView("extension_subagents");
                      openNewExtension();
                    }}
                  />
                </section>
                <section className="metric-grid metric-grid--four">
                  <article className="metric-card">
                    <p>支持客户端</p>
                    <strong>{4 + additionalClients.length}</strong>
                    <span>已支持</span>
                  </article>
                  <article className="metric-card">
                    <p>扩展 SubAgent</p>
                    <strong>{extensionSubagents.length}</strong>
                    <span>本机 Agent</span>
                  </article>
                  <article className="metric-card">
                    <p>已同步模型</p>
                    <strong>{availableModels.length}</strong>
                    <span>{models.length} 个已注册</span>
                  </article>
                  <article className="metric-card">
                    <p>供应商</p>
                    <strong>{providers.length}</strong>
                    <span>
                      {providers.filter((provider) => provider.enabled).length}{" "}
                      个已启用
                    </span>
                  </article>
                </section>
                <section className="split-grid">
                  <article className="panel route-panel">
                    <div className="panel-head">
                      <div>
                        <p className="kicker">最近使用</p>
                        <h2>客户端状态</h2>
                      </div>
                      <button
                        className="text-link"
                        onClick={() => selectView("clients")}
                      >
                        查看全部 →
                      </button>
                    </div>
                    <div className="recent-client">
                      <ClientLogo clientId="claude_code" name="Claude Code" />
                      <div>
                        <strong>Claude Code CLI</strong>
                        <small>
                          {Object.keys(state.modelSlots).length} 个槽位 ·{" "}
                          {(state.clientExtensionSubagentIds.claude_code ?? []).length} 个扩展
                        </small>
                      </div>
                      <Badge tone={integrationTone}>
                        {takeoverLabel(integration.takeover)}
                      </Badge>
                    </div>
                    <div className="recent-client">
                      <ClientLogo clientId="claude_desktop" name="Claude Client" />
                      <div>
                        <strong>Claude Client</strong>
                        <small>
                          {Object.keys(state.claudeDesktopModelSlots).length}{" "}
                          个对话角色
                        </small>
                      </div>
                      <Badge tone={desktopTone}>
                        {takeoverLabel(claudeDesktop.takeover)}
                      </Badge>
                    </div>
                  </article>
                  <article className="panel principle-panel">
                    <p className="kicker">模型路由</p>
                    <h2>路由概览</h2>
                    <p>查看各客户端当前选择的供应商和模型。</p>
                    <button
                      className="text-link"
                      onClick={() => selectView("routes")}
                    >
                      查看模型路由 →
                    </button>
                  </article>
                </section>
              </>
            )}

            {view === "extension_subagents" && (
              <>
                <SectionTitle
                  kicker="扩展 SubAgent"
                  title="本机 Agent"
                  detail="将本机已有的 Agent 保存为扩展 SubAgent，再授权给需要使用它的客户端。"
                  action={
                    <div className="section-actions">
                      <button
                        className="button button--secondary"
                        disabled={discoveringLocalAgents}
                        onClick={() => void refreshLocalAgents()}
                      >
                        {discoveringLocalAgents ? "正在同步…" : "同步本机 Agent"}
                      </button>
                      <button
                        className="button"
                        disabled={localAgents.length === 0}
                        onClick={showExtensionForm ? closeExtensionForm : openNewExtension}
                      >
                        {showExtensionForm ? "取消" : "+ 添加扩展 SubAgent"}
                      </button>
                    </div>
                  }
                />
                {localAgentError && (
                  <div className="inline-error" role="alert">
                    <strong>部分客户端 Agent 未同步</strong>
                    <span>{localAgentError}</span>
                    <button
                      className="action-button"
                      onClick={() => void refreshLocalAgents()}
                    >
                      重试
                    </button>
                  </div>
                )}
                {showExtensionForm && (
                  <form className="subagent-form extension-form" onSubmit={saveExtension}>
                    <label>
                      名称
                      <input
                        required
                        value={extensionDraft.name}
                        onChange={(event) =>
                          setExtensionDraft((current) => ({
                            ...current,
                            name: event.target.value,
                          }))
                        }
                        placeholder="代码审查"
                      />
                    </label>
                    <label>
                      来源客户端
                      <select
                        required
                        value={extensionDraft.sourceClientId}
                        onChange={(event) =>
                          setExtensionDraft((current) => ({
                            ...current,
                            sourceClientId: event.target.value,
                            sourceAgentId: "",
                          }))
                        }
                      >
                        <option value="" disabled>
                          选择本机客户端
                        </option>
                        {extensionDraft.sourceClientId &&
                          !localAgentRuntimes.includes(extensionDraft.sourceClientId) && (
                            <option value={extensionDraft.sourceClientId}>
                              {clientLabels[extensionDraft.sourceClientId] ??
                                extensionDraft.sourceClientId}（当前未检测到）
                            </option>
                          )}
                        {localAgentRuntimes.map((runtime) => (
                          <option value={runtime} key={runtime}>
                            {clientLabels[runtime] ?? runtime}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label>
                      Agent
                      <select
                        required
                        disabled={!extensionDraft.sourceClientId}
                        value={extensionDraft.sourceAgentId}
                        onChange={(event) =>
                          setExtensionDraft((current) => ({
                            ...current,
                            sourceAgentId: event.target.value,
                          }))
                        }
                      >
                        <option value="" disabled>
                          选择 Agent
                        </option>
                        {extensionDraft.sourceAgentId &&
                          !selectedSourceAgents.some(
                            (agent) => agent.agentId === extensionDraft.sourceAgentId,
                          ) && (
                            <option value={extensionDraft.sourceAgentId}>
                              {extensionDraft.sourceAgentId}（当前未检测到）
                            </option>
                          )}
                        {selectedSourceAgents.map((agent) => (
                          <option value={agent.agentId} key={agent.agentId}>
                            {agent.agentId} · {agent.description}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label>
                      模型供应商
                      <select
                        value={extensionDraft.providerId}
                        onChange={(event) =>
                          setExtensionDraft((current) => ({
                            ...current,
                            providerId: event.target.value,
                            modelId: "",
                          }))
                        }
                      >
                        <option value="">跟随来源原生</option>
                        {gatewayProviders.map((provider) => (
                          <option value={provider.id} key={provider.id}>
                            {provider.name}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label>
                      模型
                      <select
                        required={Boolean(extensionDraft.providerId)}
                        disabled={!extensionDraft.providerId}
                        value={extensionDraft.modelId}
                        onChange={(event) =>
                          setExtensionDraft((current) => ({
                            ...current,
                            modelId: event.target.value,
                          }))
                        }
                      >
                        <option value="">
                          {extensionDraft.providerId ? "选择模型" : "跟随来源原生"}
                        </option>
                        {modelsForProvider(extensionDraft.providerId).map((model) => (
                          <option value={model.id} key={model.id}>
                            {model.name} · {model.upstreamId}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className="field-wide">
                      能力标签
                      <input
                        value={extensionDraft.capabilities}
                        onChange={(event) =>
                          setExtensionDraft((current) => ({
                            ...current,
                            capabilities: event.target.value,
                          }))
                        }
                        placeholder="coding, review, testing"
                      />
                    </label>
                    <div className="subagent-form-actions">
                      <span>能力标签使用英文逗号分隔。</span>
                      <button className="button" type="submit" disabled={Boolean(pending)}>
                        {editingExtensionId ? "保存修改" : "添加"}
                      </button>
                    </div>
                  </form>
                )}
                <section className="extension-summary">
                  <article>
                    <small>本机 Agent</small>
                    <strong>{localAgents.length}</strong>
                  </article>
                  <article>
                    <small>扩展 SubAgent</small>
                    <strong>{extensionSubagents.length}</strong>
                  </article>
                  <article>
                    <small>已绑定客户端</small>
                    <strong>
                      {
                        Object.values(state.clientExtensionSubagentIds).filter(
                          (ids) => ids.length > 0,
                        ).length
                      }
                    </strong>
                  </article>
                </section>
                <div className="subagent-list extension-list">
                  {extensionSubagents.length === 0 ? (
                    <div className="subagent-empty">
                      <span>✦</span>
                      <strong>还没有扩展 SubAgent</strong>
                      <p>
                        {localAgents.length > 0
                          ? "从已发现的本机 Agent 创建第一个扩展 SubAgent。"
                          : "先同步本机 Agent。"}
                      </p>
                      <button
                        className="text-link"
                        disabled={localAgents.length === 0}
                        onClick={openNewExtension}
                      >
                        添加扩展 SubAgent →
                      </button>
                    </div>
                  ) : (
                    extensionSubagents.map((extension) => {
                      const model = models.find(
                        (item) => item.id === extension.modelId,
                      );
                      const provider = providers.find(
                        (item) => item.id === model?.providerId,
                      );
                      const isBound = Object.values(
                        state.clientExtensionSubagentIds,
                      )
                        .some((ids) => ids.includes(extension.id));
                      return (
                        <article className="subagent-card" key={extension.id}>
                          <AgentLogo sourceClientId={extension.sourceClientId} />
                          <div className="subagent-main">
                            <div>
                              <h3>{extension.name}</h3>
                              {extension.capabilities.map((capability) => (
                                <Badge key={capability}>{capability}</Badge>
                              ))}
                            </div>
                            <p>
                              {clientLabels[extension.sourceClientId] ??
                                extension.sourceClientId}
                              {" · "}
                              {extension.sourceAgentId}
                              {" · "}
                              {model && provider
                                ? `${provider.name} / ${model.name}`
                                : "跟随来源原生"}
                            </p>
                          </div>
                          <div className="row-actions">
                            <button
                              className="action-button"
                              onClick={() => editExtension(extension)}
                            >
                              编辑
                            </button>
                            <button
                              className="action-button action-button--danger"
                              disabled={isBound || Boolean(pending)}
                              title={
                                isBound
                                  ? "请先从所有客户端解绑"
                                  : "删除扩展 SubAgent"
                              }
                              onClick={() =>
                                commit(
                                  "delete_extension_subagent",
                                  { id: extension.id },
                                  `${extension.name} 已删除。`,
                                )
                              }
                            >
                              删除
                            </button>
                          </div>
                        </article>
                      );
                    })
                  )}
                </div>
              </>
            )}

            {view === "clients" && (
              <>
                <SectionTitle
                  kicker="客户端"
                  title="Coding Agent 客户端"
                  detail="选择客户端并配置对应模型。"
                />
                <section className="client-grid">
                  <button
                    className={`client-card client-card--available ${selectedClient === "claude_code" ? "client-card--selected" : ""}`}
                    type="button"
                    onClick={() => {
                      selectClient("claude_code");
                      setClientTab("slots");
                    }}
                  >
                    <ClientLogo clientId="claude_code" name="Claude Code" />
                    <ClientDetectionStatus
                      name="Claude Code"
                      installed={claudeCli.installed}
                      detail={
                        claudeCli.installed
                          ? (claudeCli.version ?? "已安装")
                          : "未检测到 Claude CLI"
                      }
                      error={clientStatusErrors.claude_code}
                    />
                  </button>
                  <button
                    className={`client-card client-card--available ${selectedClient === "claude_desktop" ? "client-card--selected" : ""}`}
                    type="button"
                    onClick={() => selectClient("claude_desktop")}
                  >
                    <ClientLogo clientId="claude_desktop" name="Claude Client" />
                    <ClientDetectionStatus
                      name="Claude Client"
                      installed={claudeDesktop.installed}
                      detail={
                        claudeDesktop.installed ? "已检测到" : "未检测到安装"
                      }
                      error={clientStatusErrors.claude_desktop}
                    />
                  </button>
                  <button
                    className={`client-card client-card--available ${selectedClient === "codex" ? "client-card--selected" : ""}`}
                    type="button"
                    onClick={() => selectClient("codex")}
                  >
                    <ClientLogo clientId="codex" name="Codex" />
                    <ClientDetectionStatus
                      name="Codex"
                      installed={codexStatus.installed}
                      detail={
                        codexStatus.installed
                          ? `${codexStatus.executablePath?.includes("/ChatGPT.app/") ? "ChatGPT 内置" : "独立 CLI"} · ${codexStatus.version ?? "已安装"}`
                          : "未检测到 Codex CLI"
                      }
                      error={clientStatusErrors.codex}
                    />
                  </button>
                  <button
                    className={`client-card client-card--available ${selectedClient === "pi" ? "client-card--selected" : ""}`}
                    type="button"
                    onClick={() => selectClient("pi")}
                  >
                    <ClientLogo clientId="pi" name="Pi" />
                    <ClientDetectionStatus
                      name="Pi"
                      installed={piStatus.installed}
                      detail={
                        piStatus.installed
                          ? (piStatus.version ?? "已安装")
                          : "未检测到 Pi CLI"
                      }
                      error={clientStatusErrors.pi}
                    />
                  </button>
                  {additionalClients.map((client) => {
                    const status = clientStatuses[client.id];
                    return (
                      <button
                        className={`client-card client-card--available ${selectedClient === client.id ? "client-card--selected" : ""}`}
                        type="button"
                        key={client.id}
                        onClick={() => selectClient(client.id)}
                      >
                        <ClientLogo clientId={client.id} name={client.name} />
                        <ClientDetectionStatus
                          name={client.name}
                          installed={status.installed}
                          detail={
                            status.installed
                              ? (status.version ?? "已安装")
                              : "未检测到客户端"
                          }
                          error={clientStatusErrors[client.id]}
                        />
                      </button>
                    );
                  })}
                </section>
                {clientStatusErrors.mcp && (
                  <p className="inline-error client-status-error" role="alert">
                    扩展状态读取失败：{clientStatusErrors.mcp}
                  </p>
                )}
                {!selectedClient && (
                  <div className="client-prompt">
                    <strong>请选择客户端</strong>
                    <span>选择客户端后即可配置对应模型。</span>
                  </div>
                )}
                {selectedClient === "claude_code" && (
                  <section
                    className={`client-detail client-detail--${clientTab}`}
                  >
                    <div className="client-detail-head">
                      <div>
                        <p className="kicker">Claude Code</p>
                        <h2>模型配置</h2>
                        <p>配置默认模型、Claude 模型族和 SubAgent。</p>
                      </div>
                      <Badge tone={integrationTone}>
                        {takeoverLabel(integration.takeover)}
                      </Badge>
                    </div>
                    <section className="agent-status-grid">
                      <article>
                        <small>Claude Code CLI</small>
                        <strong>
                          {claudeCli.installed
                            ? (claudeCli.version ?? "已安装")
                            : "未检测到"}
                        </strong>
                        <span>{claudeCli.path ?? "PATH 中没有可执行文件"}</span>
                      </article>
                      <article>
                        <small>配置状态</small>
                        <strong>{takeoverLabel(integration.takeover)}</strong>
                        <span>
                          {takeoverDetail(
                            integration.takeover,
                            integration.differences,
                          )}
                        </span>
                      </article>
                      <article>
                        <small>扩展 SubAgent</small>
                        <strong>
                          {(state.clientExtensionSubagentIds.claude_code ?? []).length} 个可用
                        </strong>
                        <span>{extensionSubagents.length} 个已创建</span>
                      </article>
                    </section>
                    <nav
                      className="client-tabs"
                      aria-label="Claude Code 模型槽位"
                    >
                      {clientTabs.map((tab) => (
                        <button
                          className={clientTab === tab.id ? "active" : ""}
                          key={tab.id}
                          onClick={() => setClientTab(tab.id)}
                        >
                          {tab.label}
                        </button>
                      ))}
                    </nav>
                    <section className="agent-card">
                      <ClientLogo clientId="claude_code" name="Claude Code" />
                      <div className="agent-copy">
                        <div>
                          <h2>Claude Code</h2>
                          <Badge tone={state.agentEnabled ? "good" : "warn"}>
                            {state.agentEnabled ? "已启用" : "未启用"}
                          </Badge>
                        </div>
                        <p>
                          {integration.takeover === "drifted"
                            ? `${takeoverDetail(integration.takeover, integration.differences)}。重新应用将覆盖这些受管项。`
                            : "保存选择后，点击应用配置即可生效。"}
                        </p>
                      </div>
                      <div className="agent-actions">
                        <button
                          className="button"
                          disabled={Boolean(pending)}
                          onClick={() =>
                            runIntegration(
                              "apply_claude_code",
                              "Claude Code 模型配置已应用。",
                            )
                          }
                        >
                          {takeoverActionLabel(integration.takeover)}
                        </button>
                        <button
                          className="button button--secondary"
                          disabled={
                            Boolean(pending) ||
                            (integration.takeover === "inactive" &&
                              !integration.snapshotPresent)
                          }
                          onClick={() =>
                            runIntegration(
                              "disable_claude_code",
                              "Claude Code 模型配置已停用；扩展 SubAgent 绑定保持不变。",
                            )
                          }
                        >
                          停用模型配置
                        </button>
                      </div>
                    </section>
                    {clientTab === "slots" && (
                      <div className="client-config-section">
                        <div className="config-heading">
                          <div>
                            <p className="kicker">模型槽位</p>
                            <h2>Claude Code 模型槽位</h2>
                            <p>先选择供应商，再选择该供应商下的模型。</p>
                          </div>
                        </div>
                        {integration.nativeModelsError && (
                          <div className="inline-error" role="alert">
                            <strong>原生模型列表暂不可用</strong>
                            <span>{integration.nativeModelsError}</span>
                            <button
                              className="button button--secondary"
                              type="button"
                              disabled={refreshingClients}
                              onClick={() => void refreshClients()}
                            >
                              重新加载
                            </button>
                          </div>
                        )}
                        <section className="slot-grid slot-grid--prominent">
                          <div className="slot-card slot-card--cascade">
                            <span>默认模型</span>
                            <label>
                              <small>供应商</small>
                              <select
                                disabled={Boolean(pending)}
                                value={
                                  claudeMainProviderSelection ||
                                  modelProviderId(
                                    state.mainModelId ?? undefined,
                                  )
                                }
                                onChange={(event) => {
                                  const providerId = event.target.value;
                                  setClaudeMainProviderSelection(providerId);
                                  if (!providerId)
                                    void commit(
                                      "set_main_model",
                                      { id: null },
                                      "Claude Code 默认模型已恢复原生。",
                                    );
                                }}
                              >
                                <option value="">跟随原生</option>
                                {gatewayProviders.map((provider) => (
                                  <option key={provider.id} value={provider.id}>
                                    {provider.name}
                                  </option>
                                ))}
                              </select>
                            </label>
                            <label>
                              <small>模型</small>
                              <select
                                disabled={Boolean(pending)}
                                value={
                                  claudeMainProviderSelection ||
                                  modelProviderId(
                                    state.mainModelId ?? undefined,
                                  )
                                    ? modelProviderId(
                                          state.mainModelId ?? undefined,
                                        ) ===
                                      (claudeMainProviderSelection ||
                                        modelProviderId(
                                          state.mainModelId ?? undefined,
                                        ))
                                      ? (state.mainModelId ?? "")
                                      : ""
                                    : (state.claudeNativeModelSlots.main ??
                                      integration.nativeModelSlots.main ??
                                      integration.nativeCurrentModel ??
                                      "default")
                                }
                                onChange={(event) => {
                                  const providerId =
                                    claudeMainProviderSelection ||
                                    modelProviderId(
                                      state.mainModelId ?? undefined,
                                    );
                                  return providerId
                                    ? commit(
                                        "set_main_model",
                                        { id: event.target.value || null },
                                        "Claude Code 默认模型已保存。",
                                      )
                                    : commit(
                                        "set_claude_native_model",
                                        {
                                          slot: "main",
                                          model: event.target.value,
                                        },
                                        "Claude Code 原生默认模型已保存。",
                                      );
                                }}
                              >
                                {claudeMainProviderSelection ||
                                modelProviderId(
                                  state.mainModelId ?? undefined,
                                ) ? (
                                  <option value="">选择模型</option>
                                ) : null}
                                {(claudeMainProviderSelection ||
                                modelProviderId(
                                  state.mainModelId ?? undefined,
                                )
                                  ? modelsForProvider(
                                  claudeMainProviderSelection ||
                                    modelProviderId(
                                      state.mainModelId ?? undefined,
                                    ),
                                    ).map((model) => ({
                                      id: model.id,
                                      label: `${model.name} · ${model.upstreamId}`,
                                    }))
                                  : claudeNativeModelOptions(
                                      state.claudeNativeModelSlots.main ??
                                        integration.nativeModelSlots.main ??
                                        integration.nativeCurrentModel,
                                      integration.nativeModels,
                                    )).map((model) => (
                                  <option key={model.id} value={model.id}>
                                    {model.label}
                                  </option>
                                ))}
                              </select>
                            </label>
                          </div>
                          {integration.supportedModelSlots.map((slot) => {
                            const selectedProviderId =
                              slotProviderSelections[slot] ??
                              modelProviderId(state.modelSlots[slot]);
                            const selectedModelId =
                              state.modelSlots[slot] ?? "";
                            return (
                              <div
                                className="slot-card slot-card--cascade"
                                key={slot}
                              >
                                <span>{modelSlotLabels[slot] ?? slot}</span>
                                <label>
                                  <small>供应商</small>
                                  <select
                                    disabled={Boolean(pending)}
                                    value={selectedProviderId}
                                    onChange={(event) => {
                                      const providerId = event.target.value;
                                      setSlotProviderSelections((current) => ({
                                        ...current,
                                        [slot]: providerId,
                                      }));
                                      if (!providerId)
                                        void commit(
                                          "set_model_slot",
                                          { slot, id: null },
                                          `${modelSlotLabels[slot] ?? slot}已恢复原生。`,
                                        );
                                    }}
                                  >
                                    <option value="">跟随原生</option>
                                    {gatewayProviders.map((provider) => (
                                      <option
                                        key={provider.id}
                                        value={provider.id}
                                      >
                                        {provider.name}
                                      </option>
                                    ))}
                                  </select>
                                </label>
                                <label>
                                  <small>模型</small>
                                  <select
                                    disabled={Boolean(pending)}
                                    value={
                                      selectedProviderId
                                        ? modelProviderId(selectedModelId) ===
                                          selectedProviderId
                                          ? selectedModelId
                                          : ""
                                        : (state.claudeNativeModelSlots[slot] ??
                                          integration.nativeModelSlots[slot] ??
                                          "default")
                                    }
                                    onChange={(event) =>
                                      selectedProviderId
                                        ? commit(
                                            "set_model_slot",
                                            {
                                              slot,
                                              id: event.target.value || null,
                                            },
                                            `${modelSlotLabels[slot] ?? slot}已保存。`,
                                          )
                                        : commit(
                                            "set_claude_native_model",
                                            {
                                              slot,
                                              model: event.target.value,
                                            },
                                            `${modelSlotLabels[slot] ?? slot}原生模型已保存。`,
                                          )
                                    }
                                  >
                                    {selectedProviderId ? (
                                      <option value="">选择模型</option>
                                    ) : null}
                                    {(selectedProviderId
                                      ? modelsForProvider(selectedProviderId).map(
                                          (model) => ({
                                            id: model.id,
                                            label: `${model.name} · ${model.upstreamId}`,
                                          }),
                                        )
                                      : claudeNativeModelOptions(
                                          state.claudeNativeModelSlots[slot] ??
                                            integration.nativeModelSlots[slot],
                                          integration.nativeModels,
                                        )
                                    ).map((model) => (
                                        <option key={model.id} value={model.id}>
                                          {model.label}
                                        </option>
                                      ))}
                                  </select>
                                </label>
                              </div>
                            );
                          })}
                        </section>
                      </div>
                    )}
                    {clientTab === "extension_subagents" && (
                      <section className="subagent-section extension-bindings">
                        <div className="config-heading">
                          <div>
                            <p className="kicker">扩展 SubAgent</p>
                            <h2>Claude Code 可用的扩展 SubAgent</h2>
                            <p>先挂载扩展，再选择允许 Claude Code 使用的扩展 SubAgent。</p>
                          </div>
                          <div className="agent-actions">
                            {mcpMountActions("claude_code")}
                            <button
                              className="button button--secondary"
                              onClick={() => selectView("extension_subagents")}
                            >
                              管理扩展 SubAgent
                            </button>
                          </div>
                        </div>
                        <div className="subagent-list">
                          {extensionSubagents.length === 0 ? (
                            <div className="subagent-empty">
                              <span>✦</span>
                              <strong>还没有扩展 SubAgent</strong>
                              <p>先从本机 Agent 创建扩展 SubAgent。</p>
                              <button
                                className="text-link"
                                onClick={() => selectView("extension_subagents")}
                              >
                                前往添加 →
                              </button>
                            </div>
                          ) : (
                            extensionSubagents.map((extension) => {
                              const enabled = (
                                state.clientExtensionSubagentIds.claude_code ?? []
                              ).includes(extension.id);
                              const model = models.find(
                                (item) => item.id === extension.modelId,
                              );
                              return (
                                <article className="subagent-card" key={extension.id}>
                                  <AgentLogo sourceClientId={extension.sourceClientId} />
                                  <div className="subagent-main">
                                    <div>
                                      <h3>{extension.name}</h3>
                                      <code>{extension.sourceAgentId}</code>
                                      {extension.capabilities.map((capability) => (
                                        <Badge key={capability}>{capability}</Badge>
                                      ))}
                                    </div>
                                    <p>
                                      {clientLabels[extension.sourceClientId] ??
                                        extension.sourceClientId}
                                      {" · "}
                                      {model?.name ?? "跟随来源原生"}
                                    </p>
                                  </div>
                                  <Toggle
                                    checked={enabled}
                                    disabled={Boolean(pending)}
                                    label={`允许 Claude Code 使用 ${extension.name}`}
                                    onChange={() =>
                                      void setExtensionBinding(
                                        "claude_code",
                                        extension,
                                        !enabled,
                                      )
                                    }
                                  />
                                </article>
                              );
                            })
                          )}
                        </div>
                      </section>
                    )}
                    <section className="limitation-card">
                      <div className="limitation-icon">!</div>
                      <div>
                        <p className="kicker">功能提示</p>
                        <h2>第三方模型的功能支持可能不同。</h2>
                        <p>
                          使用第三方模型时，Claude Remote Control 和 Tool Search
                          可能不可用。
                        </p>
                      </div>
                    </section>
                  </section>
                )}
                {selectedClient === "claude_desktop" && (
                  <section
                    className={`client-detail client-detail--${clientTab}`}
                  >
                    <div className="client-detail-head">
                      <div>
                        <p className="kicker">Claude Client</p>
                        <h2>模型配置</h2>
                        <p>
                          配置对话、Cowork 与内置 Code 使用的第三方网关。
                        </p>
                      </div>
                      <Badge tone={desktopTone}>
                        {takeoverLabel(claudeDesktop.takeover)}
                      </Badge>
                    </div>
                    <section className="agent-status-grid">
                      <article>
                        <small>Claude Client</small>
                        <strong>
                          {claudeDesktop.installed ? "已安装" : "未检测到"}
                        </strong>
                        <span>
                          {claudeDesktop.executablePath ?? "未找到 Claude.app"}
                        </span>
                      </article>
                      <article>
                        <small>配置状态</small>
                        <strong>{takeoverLabel(claudeDesktop.takeover)}</strong>
                        <span>
                          {takeoverDetail(
                            claudeDesktop.takeover,
                            claudeDesktop.differences,
                          )}
                        </span>
                      </article>
                      <article>
                        <small>扩展 SubAgent</small>
                        <strong>
                          {(state.clientExtensionSubagentIds.claude_desktop ?? [])
                            .length} 个可用
                        </strong>
                        <span>1P 与 3P 模式均可使用</span>
                      </article>
                    </section>
                    <nav
                      className="client-tabs"
                      aria-label="Claude Client 配置"
                    >
                      {clientTabs.map((tab) => (
                        <button
                          className={clientTab === tab.id ? "active" : ""}
                          key={tab.id}
                          onClick={() => setClientTab(tab.id)}
                        >
                          {tab.label}
                        </button>
                      ))}
                    </nav>
                    {clientTab === "slots" && (
                    <><section className="client-config-section desktop-role-section">
                      <div className="config-heading">
                        <div>
                          <p className="kicker">Client 对话 / Cowork</p>
                          <h2>对话角色模型</h2>
                          <p>
                            跟随原生时显示 Claude Client 当前选择；第三方模式需为全部角色选择模型。
                          </p>
                        </div>
                      </div>
                      <section className="slot-grid">
                        {claudeDesktop.supportedModelSlots.map((slot) => {
                          const selectedProviderId =
                            desktopSlotProviderSelections[slot] ??
                            modelProviderId(
                              state.claudeDesktopModelSlots[slot],
                            );
                          const selectedModelId =
                            state.claudeDesktopModelSlots[slot] ?? "";
                          return (
                            <div
                              className="slot-card slot-card--cascade"
                              key={slot}
                            >
                              <span>{modelSlotLabels[slot] ?? slot}</span>
                              <label>
                                <small>供应商</small>
                                <select
                                  disabled={Boolean(pending)}
                                  value={selectedProviderId}
                                  onChange={(event) => {
                                    const providerId = event.target.value;
                                    setDesktopSlotProviderSelections(
                                      (current) => ({
                                        ...current,
                                        [slot]: providerId,
                                      }),
                                    );
                                    if (!providerId)
                                      void commit(
                                        "set_claude_desktop_model_slot",
                                        { slot, id: null },
                                        `Claude Client ${modelSlotLabels[slot] ?? slot}已恢复原生。`,
                                      );
                                  }}
                                >
                                  <option value="">跟随原生</option>
                                  {gatewayProviders.map((provider) => (
                                    <option
                                      key={provider.id}
                                      value={provider.id}
                                    >
                                      {provider.name}
                                    </option>
                                  ))}
                                </select>
                              </label>
                              <label>
                                <small>模型</small>
                                <select
                                  disabled={Boolean(pending) || !selectedProviderId}
                                  value={
                                    selectedProviderId
                                      ? modelProviderId(selectedModelId) ===
                                        selectedProviderId
                                        ? selectedModelId
                                        : ""
                                      : (claudeDesktop.nativeCurrentModel ??
                                        "default")
                                  }
                                  onChange={(event) =>
                                    commit(
                                      "set_claude_desktop_model_slot",
                                      { slot, id: event.target.value || null },
                                      `Claude Client ${modelSlotLabels[slot] ?? slot}已保存。`,
                                    )
                                  }
                                >
                                  {selectedProviderId ? (
                                    <option value="">选择模型</option>
                                  ) : null}
                                  {(selectedProviderId
                                    ? modelsForProvider(selectedProviderId).map(
                                        (model) => ({
                                          id: model.id,
                                          label: `${model.name} · ${model.upstreamId}`,
                                        }),
                                      )
                                    : claudeNativeModelOptions(
                                        claudeDesktop.nativeCurrentModel ??
                                          "default",
                                        claudeDesktop.nativeModels,
                                      )
                                  ).map((model) => (
                                    <option key={model.id} value={model.id}>
                                      {model.label}
                                    </option>
                                  ))}
                                </select>
                              </label>
                            </div>
                          );
                        })}
                      </section>
                      {integration.nativeModelsError && (
                        <div className="inline-error" role="alert">
                          <strong>Code 原生模型列表暂不可用</strong>
                          <span>{integration.nativeModelsError}</span>
                          <button
                            className="button button--secondary"
                            type="button"
                            disabled={refreshingClients}
                            onClick={() => void refreshClients()}
                          >
                            重新加载
                          </button>
                        </div>
                      )}
                      <ClaudeClientCodeSubagentSlot
                        disabled={Boolean(pending) || !claudeCli.installed}
                        selectedProviderId={
                          slotProviderSelections.subagent_default ??
                          modelProviderId(state.modelSlots.subagent_default)
                        }
                        managedModelId={state.modelSlots.subagent_default ?? ""}
                        nativeModel={
                          state.claudeNativeModelSlots.subagent_default ??
                          integration.nativeModelSlots.subagent_default
                        }
                        nativeModels={integration.nativeModels}
                        providers={gatewayProviders}
                        models={modelsForProvider(
                          slotProviderSelections.subagent_default ??
                            modelProviderId(state.modelSlots.subagent_default),
                        )}
                        onProviderChange={(providerId) => {
                          setSlotProviderSelections((current) => ({
                            ...current,
                            subagent_default: providerId,
                          }));
                          if (!providerId)
                            return commit(
                              "set_model_slot",
                              { slot: "subagent_default", id: null },
                              "Code SubAgent 默认模型已恢复原生。",
                            );
                        }}
                        onManagedModelChange={(id) =>
                          commit(
                            "set_model_slot",
                            { slot: "subagent_default", id: id || null },
                            "Code SubAgent 默认模型已保存。",
                          )
                        }
                        onNativeModelChange={(model) =>
                          commit(
                            "set_claude_native_model",
                            { slot: "subagent_default", model },
                            "Claude Client Code 的原生 SubAgent 默认模型已保存。",
                          )
                        }
                      />
                    </section>
                    <section className="agent-card">
                      <ClientLogo
                        clientId="claude_desktop"
                        name="Claude Client"
                      />
                      <div className="agent-copy">
                        <div>
                          <h2>Claude Client</h2>
                          <Badge tone={desktopTone}>
                            {takeoverLabel(claudeDesktop.takeover)}
                          </Badge>
                        </div>
                        <p>
                          {claudeDesktop.takeover === "drifted"
                            ? `${takeoverDetail(claudeDesktop.takeover, claudeDesktop.differences)}。重新应用将覆盖这些受管文件。`
                            : "应用后 Claude Client 的对话、Cowork 和内置 Code 都会使用该第三方网关；重新启动 Client 后生效。"}
                        </p>
                      </div>
                      <div className="agent-actions">
                        <button
                          className="button"
                          disabled={
                            Boolean(pending) ||
                            !claudeDesktop.installed
                          }
                          onClick={() =>
                            runDesktopIntegration(
                              "apply_claude_desktop",
                              "配置已应用，请保持 GrillForge 运行并重新启动 Claude Client。",
                            )
                          }
                        >
                          {takeoverActionLabel(claudeDesktop.takeover)}
                        </button>
                        <button
                          className="button button--secondary"
                          disabled={
                            Boolean(pending) ||
                            (claudeDesktop.takeover === "inactive" &&
                              !claudeDesktop.snapshotPresent)
                          }
                          onClick={() =>
                            runDesktopIntegration(
                              "disable_claude_desktop",
                              "Claude Client 模型配置已停用；扩展 SubAgent 绑定保持不变。重新启动 Client 后恢复官方模式。",
                            )
                          }
                        >
                          停用模型配置
                        </button>
                      </div>
                    </section></>
                    )}
                    {clientTab === "extension_subagents" &&
                      extensionBindingsFor("claude_desktop", "Claude Client")}
                  </section>
                )}
                {selectedClient === "pi" && (
                  <section
                    className={`client-detail client-detail--${clientTab}`}
                  >
                    <div className="client-detail-head">
                      <div>
                        <p className="kicker">Pi</p>
                        <h2>模型配置</h2>
                        <p>选择默认模型，并管理可用模型池。</p>
                      </div>
                      <Badge tone={piTone}>
                        {takeoverLabel(piStatus.takeover)}
                      </Badge>
                    </div>
                    <section className="agent-status-grid">
                      <article>
                        <small>Pi CLI</small>
                        <strong>
                          {piStatus.installed
                            ? (piStatus.version ?? "已安装")
                            : "未检测到"}
                        </strong>
                        <span>
                          {piStatus.executablePath ??
                            "PATH 中没有 pi 可执行文件"}
                        </span>
                      </article>
                      <article>
                        <small>配置状态</small>
                        <strong>{takeoverLabel(piStatus.takeover)}</strong>
                        <span>{takeoverDetail(piStatus.takeover)}</span>
                      </article>
                      <article>
                        <small>可用模型</small>
                        <strong>{state.piEnabledModelIds.length} 个</strong>
                        <span>
                          {state.piMainModelId
                            ? "已选择默认模型"
                            : "跟随原生"}
                        </span>
                      </article>
                      <article>
                        <small>MCP 扩展</small>
                        <strong>
                          {piMcpExtension?.installed ? "已安装" : "未安装"}
                        </strong>
                        {piMcpExtension?.installed ? (
                          <span>扩展 SubAgent 可通过 Pi 使用</span>
                        ) : (
                          <PiMcpInstallControl
                            buttonClassName="text-link"
                            disabled={Boolean(pending) || !piStatus.installed}
                            label="一键安装"
                            onInstall={installPiMcpExtension}
                          />
                        )}
                      </article>
                    </section>
                    <nav className="client-tabs" aria-label="Pi 配置">
                      {clientTabs.map((tab) => (
                        <button
                          className={clientTab === tab.id ? "active" : ""}
                          key={tab.id}
                          onClick={() => setClientTab(tab.id)}
                        >
                          {tab.label}
                        </button>
                      ))}
                    </nav>
                    {clientTab === "slots" && (
                    <><section className="client-config-section">
                      <div className="config-heading">
                        <div>
                          <p className="kicker">默认模型</p>
                          <h2>Pi 主模型</h2>
                          <p>
                            先选择供应商，再选择模型；选为默认时会自动加入可用模型池。
                          </p>
                        </div>
                      </div>
                      <section className="slot-grid">
                        <div className="slot-card slot-card--cascade">
                          <span>默认模型</span>
                          <label>
                            <small>供应商</small>
                            <select
                              disabled={Boolean(pending)}
                              value={
                                piDefaultProviderSelection ||
                                modelProviderId(
                                  state.piMainModelId ?? undefined,
                                )
                              }
                              onChange={(event) => {
                                const providerId = event.target.value;
                                setPiDefaultProviderSelection(providerId);
                                if (!providerId)
                                  void commit(
                                    "set_pi_main_model",
                                    { id: null },
                                    "Pi 默认模型已恢复原生。",
                                  );
                              }}
                            >
                              <option value="">跟随原生</option>
                              {gatewayProviders.map((provider) => (
                                <option key={provider.id} value={provider.id}>
                                  {provider.name}
                                </option>
                              ))}
                            </select>
                          </label>
                          <label>
                            <small>模型</small>
                            <select
                              disabled={
                                Boolean(pending) ||
                                !(
                                  piDefaultProviderSelection ||
                                  modelProviderId(
                                    state.piMainModelId ?? undefined,
                                  )
                                )
                              }
                              value={
                                modelProviderId(
                                  state.piMainModelId ?? undefined,
                                ) ===
                                (piDefaultProviderSelection ||
                                  modelProviderId(
                                    state.piMainModelId ?? undefined,
                                  ))
                                  ? (state.piMainModelId ?? "")
                                  : ""
                              }
                              onChange={(event) =>
                                commit(
                                  "set_pi_main_model",
                                  { id: event.target.value || null },
                                  "Pi 默认模型已保存。",
                                )
                              }
                            >
                              <option value="">选择模型</option>
                              {modelsForProvider(
                                piDefaultProviderSelection ||
                                  modelProviderId(
                                    state.piMainModelId ?? undefined,
                                  ),
                              ).map((model) => (
                                <option key={model.id} value={model.id}>
                                  {model.name} · {model.upstreamId}
                                </option>
                              ))}
                            </select>
                          </label>
                        </div>
                      </section>
                    </section>
                    <section className="subagent-section">
                      <div className="config-heading">
                        <div>
                          <p className="kicker">模型池</p>
                          <h2>Pi 可用模型</h2>
                          <p>启用希望在 Pi 中使用的模型。</p>
                        </div>
                      </div>
                      <div className="subagent-list">
                        {gatewayProviders.flatMap((provider) =>
                          modelsForProvider(provider.id).map((model) => (
                            <article className="subagent-card" key={model.id}>
                              <ProviderLogo provider={provider} />
                              <div className="subagent-main">
                                <div>
                                  <h3>{model.name}</h3>
                                  <Badge>{provider.name}</Badge>
                                </div>
                                <p>
                                  <code>{model.upstreamId}</code>
                                </p>
                              </div>
                              <Toggle
                                checked={state.piEnabledModelIds.includes(
                                  model.id,
                                )}
                                disabled={
                                  Boolean(pending) ||
                                  state.piMainModelId === model.id
                                }
                                label={`切换 Pi 模型 ${model.name}`}
                                onChange={() =>
                                  commit(
                                    "set_pi_model_enabled",
                                    {
                                      id: model.id,
                                      enabled:
                                        !state.piEnabledModelIds.includes(
                                          model.id,
                                        ),
                                    },
                                    `${model.name} 已${state.piEnabledModelIds.includes(model.id) ? "移出" : "加入"} Pi 模型池。`,
                                  )
                                }
                              />
                            </article>
                          )),
                        )}
                      </div>
                    </section>
                    </>
                    )}
                    {clientTab === "extension_subagents" && (
                    <section className="subagent-section extension-bindings">
                      <div className="config-heading">
                        <div>
                          <p className="kicker">扩展 SubAgent</p>
                          <h2>Pi 可用的扩展 SubAgent</h2>
                          <p>挂载扩展后，由 Pi MCP 扩展调用已授权的本机 Agent。</p>
                        </div>
                        <div className="agent-actions">
                          {mcpMountActions("pi")}
                          <button
                            className="button button--secondary"
                            onClick={() => selectView("extension_subagents")}
                          >
                            管理扩展 SubAgent
                          </button>
                        </div>
                      </div>
                      {!piMcpExtension?.installed ? (
                        <div className="subagent-empty">
                          <strong>需要 Pi MCP 扩展</strong>
                          <p>安装后即可绑定扩展 SubAgent。</p>
                          <PiMcpInstallControl
                            disabled={Boolean(pending) || !piStatus.installed}
                            label="一键安装 pi-mcp-extension"
                            onInstall={installPiMcpExtension}
                          />
                        </div>
                      ) : extensionSubagents.length === 0 ? (
                        <div className="subagent-empty">
                          <strong>还没有扩展 SubAgent</strong>
                          <button
                            className="text-link"
                            onClick={() => selectView("extension_subagents")}
                          >
                            前往添加 →
                          </button>
                        </div>
                      ) : (
                        <div className="subagent-list">
                          {extensionSubagents.map((extension) => {
                            const enabled = (
                              state.clientExtensionSubagentIds.pi ?? []
                            ).includes(extension.id);
                            const model = models.find(
                              (item) => item.id === extension.modelId,
                            );
                            return (
                              <article className="subagent-card" key={extension.id}>
                                <AgentLogo sourceClientId={extension.sourceClientId} />
                                <div className="subagent-main">
                                  <div>
                                    <h3>{extension.name}</h3>
                                    <code>{extension.sourceAgentId}</code>
                                  </div>
                                  <p>
                                    {clientLabels[extension.sourceClientId] ??
                                      extension.sourceClientId}
                                    {" · "}
                                    {model?.name ?? "跟随来源原生"}
                                  </p>
                                </div>
                                <Toggle
                                  checked={enabled}
                                  disabled={Boolean(pending)}
                                  label={`允许 Pi 使用 ${extension.name}`}
                                  onChange={() =>
                                    void setExtensionBinding(
                                      "pi",
                                      extension,
                                      !enabled,
                                    )
                                  }
                                />
                              </article>
                            );
                          })}
                        </div>
                      )}
                    </section>
                    )}
                    {clientTab === "slots" && (
                    <section className="agent-card">
                      <ClientLogo clientId="pi" name="Pi" />
                      <div className="agent-copy">
                        <div>
                          <h2>Pi</h2>
                          <Badge tone={piTone}>
                            {takeoverLabel(piStatus.takeover)}
                          </Badge>
                        </div>
                        <p>
                          {piStatus.takeover === "drifted"
                            ? `${takeoverDetail(piStatus.takeover)}。重新应用将覆盖受管配置。`
                            : "保存模型后，点击应用配置即可生效。"}
                        </p>
                      </div>
                      <div className="agent-actions">
                        <button
                          className="button"
                          disabled={
                            Boolean(pending) ||
                            !piStatus.installed ||
                            state.piEnabledModelIds.length === 0
                          }
                          onClick={() =>
                            runPiIntegration("apply_pi", "Pi 模型配置已应用。")
                          }
                        >
                          {takeoverActionLabel(piStatus.takeover)}
                        </button>
                        <button
                          className="button button--secondary"
                          disabled={
                            Boolean(pending) ||
                            (piStatus.takeover === "inactive" &&
                              !piStatus.snapshotPresent)
                          }
                          onClick={() =>
                            runPiIntegration(
                              "disable_pi",
                              "Pi 模型配置已停用；扩展 SubAgent 绑定保持不变。",
                            )
                          }
                        >
                          停用模型配置
                        </button>
                      </div>
                    </section>
                    )}
                  </section>
                )}
                {selectedClient === "codex" &&
                  (() => {
                    const responseProviders = providers.filter(
                      (provider) =>
                        provider.enabled &&
                        provider.protocol !== "gemini_native",
                    );
                    const nativeMain = state.codexNativeModelSlots.main ?? "";
                    const mainProviderId =
                      codexProviderSelection ||
                      (state.codexMainModelId
                        ? modelProviderId(state.codexMainModelId)
                        : "codex_native");
                    const selectedRoutedProvider = responseProviders.find(
                      (provider) => provider.id === mainProviderId,
                    );
                    const mainValue =
                      mainProviderId === "codex_native"
                        ? nativeMain || (codexStatus.currentConfigModel ?? "")
                        : modelProviderId(
                              state.codexMainModelId ?? undefined,
                            ) === mainProviderId
                          ? (state.codexMainModelId ?? "")
                          : "";
                    const defaultNative =
                      state.codexNativeModelSlots.default_subagent ?? "";
                    const defaultExternal =
                      state.codexAgentModelIds.default_subagent ?? "";
                    const defaultValue =
                      mainProviderId === "codex_native"
                        ? defaultNative
                        : modelProviderId(defaultExternal) === mainProviderId
                          ? defaultExternal
                          : "";
                    const hasMain = Boolean(
                      nativeMain ||
                        state.codexMainModelId ||
                        codexStatus.currentConfigModel,
                    );
                    return (
                      <section
                        className={`client-detail client-detail--${clientTab}`}
                      >
                        <div className="client-detail-head">
                          <div>
                            <p className="kicker">Codex</p>
                            <h2>模型配置</h2>
                            <p>配置主模型、SubAgent 默认模型与自定义 Agent。</p>
                          </div>
                          <Badge tone={codexTone}>
                            {takeoverLabel(codexStatus.takeover)}
                          </Badge>
                        </div>
                        <section className="agent-status-grid">
                          <article>
                            <small>
                              {codexStatus.executablePath?.includes(
                                "/ChatGPT.app/",
                              )
                                ? "ChatGPT 内置 Codex CLI"
                                : "Codex CLI"}
                            </small>
                            <strong>
                              {codexStatus.installed
                                ? (codexStatus.version ?? "已安装")
                                : "未检测到"}
                            </strong>
                            <span>
                              {codexStatus.executablePath ??
                                "未找到独立或 ChatGPT 内置 Codex CLI"}
                            </span>
                          </article>
                          <article>
                            <small>配置状态</small>
                            <strong>
                              {takeoverLabel(codexStatus.takeover)}
                            </strong>
                            <span>{takeoverDetail(codexStatus.takeover)}</span>
                          </article>
                          <article>
                            <small>当前模型</small>
                            <strong>
                              {nativeMain
                                ? (codexStatus.nativeModels.find(
                                    (model) => model.id === nativeMain,
                                  )?.name ?? nativeMain)
                                : state.codexMainModelId
                                  ? (models.find(
                                      (model) =>
                                        model.id === state.codexMainModelId,
                                    )?.name ?? "已选择")
                                  : codexStatus.currentConfigModel
                                    ? (codexStatus.nativeModels.find(
                                        (model) =>
                                          model.id ===
                                          codexStatus.currentConfigModel,
                                      )?.name ??
                                      codexStatus.currentConfigModel)
                                    : "Codex 默认模型"}
                            </strong>
                            <span>
                              {nativeMain
                                ? "Codex 原生模型"
                                : state.codexMainModelId
                                  ? (providers.find(
                                      (provider) =>
                                        provider.id ===
                                        modelProviderId(
                                          state.codexMainModelId ?? undefined,
                                        ),
                                    )?.name ?? "")
                                  : codexStatus.currentConfigProvider
                                    ? `${codexStatus.currentConfigProvider} · 当前 Codex 配置`
                                    : "跟随 Codex 原生配置"}
                            </span>
                          </article>
                          <article>
                            <small>扩展 SubAgent</small>
                            <strong>
                              {(state.clientExtensionSubagentIds.codex ?? [])
                                .length} 个可用
                            </strong>
                            <span>{extensionSubagents.length} 个已创建</span>
                          </article>
                        </section>
                        <nav className="client-tabs" aria-label="Codex 配置">
                          {clientTabs.map((tab) => (
                            <button
                              className={clientTab === tab.id ? "active" : ""}
                              key={tab.id}
                              onClick={() => setClientTab(tab.id)}
                            >
                              {tab.label}
                            </button>
                          ))}
                        </nav>
                        <section className="client-config-section">
                          <div className="config-heading">
                            <div>
                              <p className="kicker">主模型</p>
                              <h2>Codex 默认模型</h2>
                              <p>先选择供应商，再选择模型。</p>
                            </div>
                          </div>
                          {codexStatus.nativeModelsError && (
                            <div className="inline-error">
                              <strong>原生模型列表暂不可用</strong>
                              <span>{codexStatus.nativeModelsError}</span>
                              <button
                                className="button button--secondary"
                                type="button"
                                disabled={refreshingClients}
                                onClick={() => void refreshClients()}
                              >
                                重新加载
                              </button>
                            </div>
                          )}
                          <section className="slot-grid">
                            <div className="slot-card slot-card--cascade">
                              <span>主模型</span>
                              <label>
                                <small>供应商</small>
                                <select
                                  disabled={Boolean(pending)}
                                  value={mainProviderId}
                                  onChange={(event) => {
                                    const providerId = event.target.value;
                                    setCodexProviderSelection(providerId);
                                    if (providerId === "codex_native")
                                      void commit(
                                        "set_codex_native_main_model",
                                        { model: null },
                                        "Codex 主模型已恢复跟随原生。",
                                      );
                                  }}
                                >
                                  <option value="codex_native">跟随原生</option>
                                  {responseProviders.map((provider) => (
                                    <option
                                      key={provider.id}
                                      value={provider.id}
                                    >
                                      {provider.name} ·{" "}
                                      {protocolLabels[provider.protocol]}
                                    </option>
                                  ))}
                                </select>
                              </label>
                              <label>
                                <small>模型</small>
                                <select
                                  disabled={
                                    Boolean(pending) ||
                                    (mainProviderId === "codex_native" &&
                                      codexStatus.nativeModels.length === 0)
                                  }
                                  value={mainValue}
                                  onChange={(event) =>
                                    mainProviderId === "codex_native"
                                      ? commit(
                                          "set_codex_native_main_model",
                                          { model: event.target.value || null },
                                          "Codex 主模型已保存。",
                                        )
                                      : commit(
                                          "set_codex_main_model",
                                          { id: event.target.value || null },
                                          "Codex 主模型已保存。",
                                        )
                                  }
                                >
                                  {mainProviderId === "codex_native" &&
                                    !mainValue && (
                                      <option value="">Codex 默认模型</option>
                                    )}
                                  {mainProviderId === "codex_native" &&
                                    mainValue &&
                                    !codexStatus.nativeModels.some(
                                      (model) => model.id === mainValue,
                                    ) && (
                                      <option value={mainValue}>
                                        当前配置 · {mainValue}
                                      </option>
                                    )}
                                  {mainProviderId !== "codex_native" && (
                                    <option value="">选择模型</option>
                                  )}
                                  {mainProviderId === "codex_native"
                                    ? codexStatus.nativeModels.map((model) => (
                                        <option key={model.id} value={model.id}>
                                          {model.name} · {model.id}
                                        </option>
                                      ))
                                    : modelsForProvider(mainProviderId).map(
                                        (model) => (
                                          <option
                                            key={model.id}
                                            value={model.id}
                                          >
                                            {model.name} · {model.upstreamId}
                                          </option>
                                        ),
                                      )}
                                </select>
                              </label>
                            </div>
                          </section>
                        </section>
                        <section className="client-config-section">
                          <div className="config-heading">
                            <div>
                              <p className="kicker">SubAgent</p>
                              <h2>内置 Agent 默认模型</h2>
                              <p>default、worker、explorer 共用该默认模型。</p>
                            </div>
                          </div>
                          <section className="slot-grid">
                            <div className="slot-card slot-card--cascade">
                              <span>SubAgent 默认</span>
                              <label>
                                <small>供应商</small>
                                <select disabled value={mainProviderId}>
                                  <option value={mainProviderId}>
                                    {mainProviderId === "codex_native"
                                      ? "跟随主模型（Codex 原生）"
                                      : selectedRoutedProvider
                                        ? `${selectedRoutedProvider.name} · ${protocolLabels[selectedRoutedProvider.protocol]}`
                                        : "请先选择主模型"}
                                  </option>
                                </select>
                              </label>
                              <label>
                                <small>模型</small>
                                <select
                                  disabled={Boolean(pending) || !mainProviderId}
                                  value={defaultValue}
                                  onChange={(event) =>
                                    mainProviderId === "codex_native"
                                      ? commit(
                                          "set_codex_native_default_subagent_model",
                                          { model: event.target.value || null },
                                          "Codex SubAgent 默认模型已保存。",
                                        )
                                      : commit(
                                          "set_codex_default_subagent_model",
                                          { id: event.target.value || null },
                                          "Codex SubAgent 默认模型已保存。",
                                        )
                                  }
                                >
                                  <option value="">跟随主模型</option>
                                  {mainProviderId === "codex_native"
                                    ? codexStatus.nativeModels.map((model) => (
                                        <option key={model.id} value={model.id}>
                                          {model.name} · {model.id}
                                        </option>
                                      ))
                                    : modelsForProvider(mainProviderId).map(
                                        (model) => (
                                          <option
                                            key={model.id}
                                            value={model.id}
                                          >
                                            {model.name} · {model.upstreamId}
                                          </option>
                                        ),
                                      )}
                                </select>
                              </label>
                            </div>
                          </section>
                        </section>
                        {codexStatus.customAgents.length > 0 && (
                          <section className="subagent-section">
                            <div className="config-heading">
                              <div>
                                <p className="kicker">自定义 Agents</p>
                                <h2>逐 Agent 模型</h2>
                                <p>已从 Codex Agent 目录同步。</p>
                              </div>
                            </div>
                            <div className="subagent-list">
                              {codexStatus.customAgents.map((agent) => {
                                const native =
                                  state.codexNativeModelSlots[
                                    `agent_${agent.name}`
                                  ] ?? "";
                                const external =
                                  state.codexAgentModelIds[agent.name] ?? "";
                                const providerId =
                                  codexAgentProviderSelections[agent.name] ||
                                  (native
                                    ? "codex_native"
                                    : modelProviderId(external));
                                const selected =
                                  providerId === "codex_native"
                                    ? native
                                    : modelProviderId(external) === providerId
                                      ? external
                                      : "";
                                return (
                                  <article
                                    className="subagent-card"
                                    key={agent.name}
                                  >
                                    <AgentLogo sourceClientId="codex" />
                                    <div className="subagent-main">
                                      <div>
                                        <h3>{agent.name}</h3>
                                        <Badge>
                                          {agent.configuredModel ?? "跟随默认"}
                                        </Badge>
                                      </div>
                                      <p>{agent.description}</p>
                                      <div className="slot-card slot-card--cascade">
                                        <label>
                                          <small>供应商</small>
                                          <select
                                            disabled={Boolean(pending)}
                                            value={providerId}
                                            onChange={(event) => {
                                              const next = event.target.value;
                                              setCodexAgentProviderSelections(
                                                (current) => ({
                                                  ...current,
                                                  [agent.name]: next,
                                                }),
                                              );
                                              if (!next)
                                                void commit(
                                                  "set_codex_native_custom_agent_model",
                                                  {
                                                    name: agent.name,
                                                    model: null,
                                                  },
                                                  `${agent.name} 已恢复默认模型。`,
                                                );
                                            }}
                                          >
                                            <option value="">跟随默认</option>
                                            <option value="codex_native">
                                              Codex 原生模型
                                            </option>
                                            {responseProviders.map(
                                              (provider) => (
                                                <option
                                                  key={provider.id}
                                                  value={provider.id}
                                              >
                                                  {provider.name} ·{" "}
                                                  {
                                                    protocolLabels[
                                                      provider.protocol
                                                    ]
                                                  }
                                                </option>
                                              ),
                                            )}
                                          </select>
                                        </label>
                                        <label>
                                          <small>模型</small>
                                          <select
                                            disabled={
                                              Boolean(pending) || !providerId
                                            }
                                            value={selected}
                                            onChange={(event) =>
                                              providerId === "codex_native"
                                                ? commit(
                                                    "set_codex_native_custom_agent_model",
                                                    {
                                                      name: agent.name,
                                                      model:
                                                        event.target.value ||
                                                        null,
                                                    },
                                                    `${agent.name} 模型已保存。`,
                                                  )
                                                : commit(
                                                    "set_codex_custom_agent_model",
                                                    {
                                                      name: agent.name,
                                                      id:
                                                        event.target.value ||
                                                        null,
                                                    },
                                                    `${agent.name} 模型已保存。`,
                                                  )
                                            }
                                          >
                                            <option value="">选择模型</option>
                                            {providerId === "codex_native"
                                              ? codexStatus.nativeModels.map(
                                                  (model) => (
                                                    <option
                                                      key={model.id}
                                                      value={model.id}
                                                    >
                                                      {model.name} · {model.id}
                                                    </option>
                                                  ),
                                                )
                                              : modelsForProvider(
                                                  providerId,
                                                ).map((model) => (
                                                  <option
                                                    key={model.id}
                                                    value={model.id}
                                                  >
                                                    {model.name} ·{" "}
                                                    {model.upstreamId}
                                                  </option>
                                                ))}
                                          </select>
                                        </label>
                                      </div>
                                    </div>
                                  </article>
                                );
                              })}
                            </div>
                          </section>
                        )}
                        {extensionBindingsFor("codex", "Codex")}
                        <section className="agent-card">
                          <ClientLogo clientId="codex" name="Codex" />
                          <div className="agent-copy">
                            <div>
                              <h2>Codex CLI</h2>
                              <Badge tone={codexTone}>
                                {takeoverLabel(codexStatus.takeover)}
                              </Badge>
                            </div>
                            <p>
                              {codexStatus.takeover === "drifted"
                                ? `${takeoverDetail(codexStatus.takeover)}。重新应用将覆盖受管配置。`
                                : "保存模型后，点击应用配置即可生效。"}
                            </p>
                          </div>
                          <div className="agent-actions">
                            <button
                              className="button"
                              disabled={
                                Boolean(pending) ||
                                !codexStatus.installed ||
                                !hasMain
                              }
                              onClick={() =>
                                runCodexIntegration(
                                  "apply_codex",
                                  "Codex 模型配置已应用。",
                                )
                              }
                            >
                              {takeoverActionLabel(codexStatus.takeover)}
                            </button>
                            <button
                              className="button button--secondary"
                              disabled={
                                Boolean(pending) ||
                                (codexStatus.takeover === "inactive" &&
                                  !codexStatus.snapshotPresent)
                              }
                              onClick={() =>
                                runCodexIntegration(
                                  "disable_codex",
                                  "Codex 模型配置已停用；扩展 SubAgent 绑定保持不变。",
                                )
                              }
                            >
                              停用模型配置
                            </button>
                          </div>
                        </section>
                      </section>
                    );
                  })()}
                {selectedAdditionalClient &&
                  selectedAdditionalStatus &&
                  selectedClientConfiguration &&
                  (() => {
                    const configuredProviderId = modelProviderId(
                      selectedClientConfiguration.mainModelId ?? undefined,
                    );
                    const providerId =
                      clientProviderSelections[selectedAdditionalClient.id] ??
                      configuredProviderId;
                    const statusTone = takeoverTone(
                      selectedAdditionalStatus.takeover,
                    );
                    const compatibleModels = modelsForProvider(providerId);
                    return (
                      <section
                        className={`client-detail client-detail--${clientTab}`}
                      >
                        <div className="client-detail-head">
                          <div>
                            <p className="kicker">
                              {selectedAdditionalClient.name}
                            </p>
                            <h2>模型配置</h2>
                            <p>
                              {selectedAdditionalClient.pool
                                ? "选择默认模型，并管理可用模型池。"
                                : "选择默认使用的模型。"}
                            </p>
                          </div>
                          <Badge tone={statusTone}>
                            {takeoverLabel(selectedAdditionalStatus.takeover)}
                          </Badge>
                        </div>
                        <section className="agent-status-grid">
                          <article>
                            <small>{selectedAdditionalClient.name}</small>
                            <strong>
                              {selectedAdditionalStatus.installed
                                ? (selectedAdditionalStatus.version ?? "已安装")
                                : "未检测到"}
                            </strong>
                            <span>
                              {selectedAdditionalStatus.executablePath ??
                                "PATH 中没有可执行文件"}
                            </span>
                          </article>
                          <article>
                            <small>配置状态</small>
                            <strong>
                              {takeoverLabel(selectedAdditionalStatus.takeover)}
                            </strong>
                            <span>
                              {takeoverDetail(
                                selectedAdditionalStatus.takeover,
                              )}
                            </span>
                          </article>
                          <article>
                            <small>可用模型</small>
                            <strong>
                              {selectedClientConfiguration.enabledModelIds
                                .length ||
                                (selectedClientConfiguration.mainModelId
                                  ? 1
                                  : 0)}
                            </strong>
                            <span>
                              {selectedAdditionalClient.pool
                                ? "默认模型与模型池"
                                : "默认模型"}
                            </span>
                          </article>
                          {["gemini", "opencode", "kimi_code"].includes(
                            selectedAdditionalClient.id,
                          ) && (
                            <article>
                              <small>扩展 SubAgent</small>
                              <strong>
                                {(state.clientExtensionSubagentIds[
                                  selectedAdditionalClient.id
                                ] ?? []).length} 个可用
                              </strong>
                              <span>{extensionSubagents.length} 个已创建</span>
                            </article>
                          )}
                        </section>
                        {["gemini", "opencode", "kimi_code"].includes(
                          selectedAdditionalClient.id,
                        ) && (
                          <nav
                            className="client-tabs"
                            aria-label={`${selectedAdditionalClient.name} 配置`}
                          >
                            {clientTabs.map((tab) => (
                              <button
                                className={clientTab === tab.id ? "active" : ""}
                                key={tab.id}
                                onClick={() => setClientTab(tab.id)}
                              >
                                {tab.label}
                              </button>
                            ))}
                          </nav>
                        )}
                        <section className="client-config-section">
                          <div className="config-heading">
                            <div>
                              <p className="kicker">模型</p>
                              <h2>{selectedAdditionalClient.name} 模型配置</h2>
                              <p>先选择供应商，再选择模型。</p>
                            </div>
                          </div>
                          <section className="slot-grid">
                            <div className="slot-card slot-card--cascade">
                              <span>
                                {selectedAdditionalClient.id === "kimi_code"
                                  ? "默认模型"
                                  : "主模型"}
                              </span>
                              <label>
                                <small>供应商</small>
                                <select
                                  disabled={Boolean(pending)}
                                  value={providerId}
                                  onChange={(event) => {
                                    const next = event.target.value;
                                    setClientProviderSelections((current) => ({
                                      ...current,
                                      [selectedAdditionalClient.id]: next,
                                    }));
                                    if (!next)
                                      void commit(
                                        "set_client_main_model",
                                        {
                                          clientId: selectedAdditionalClient.id,
                                          id: null,
                                        },
                                        `${selectedAdditionalClient.name} 已恢复原生模型。`,
                                      );
                                  }}
                                >
                                  <option value="">跟随原生</option>
                                  {selectedClientProviders.map((provider) => (
                                    <option
                                      key={provider.id}
                                      value={provider.id}
                                    >
                                      {provider.name}
                                    </option>
                                  ))}
                                </select>
                              </label>
                              <label>
                                <small>模型</small>
                                <select
                                  disabled={Boolean(pending) || !providerId}
                                  value={
                                    modelProviderId(
                                      selectedClientConfiguration.mainModelId ??
                                        undefined,
                                    ) === providerId
                                      ? (selectedClientConfiguration.mainModelId ??
                                        "")
                                      : ""
                                  }
                                  onChange={(event) =>
                                    commit(
                                      "set_client_main_model",
                                      {
                                        clientId: selectedAdditionalClient.id,
                                        id: event.target.value || null,
                                      },
                                      `${selectedAdditionalClient.name} 主模型已保存。`,
                                    )
                                  }
                                >
                                  <option value="">选择模型</option>
                                  {compatibleModels.map((model) => (
                                    <option key={model.id} value={model.id}>
                                      {model.name} · {model.upstreamId}
                                    </option>
                                  ))}
                                </select>
                              </label>
                            </div>
                          </section>
                        </section>
                        {selectedAdditionalClient.pool && (
                          <section className="subagent-section">
                            <div className="config-heading">
                              <div>
                                <p className="kicker">模型池</p>
                                <h2>
                                  {selectedAdditionalClient.name} 可用模型
                                </h2>
                                <p>启用该客户端可以使用的模型。</p>
                              </div>
                            </div>
                            <div className="subagent-list">
                              {selectedClientProviders.flatMap((provider) =>
                                modelsForProvider(provider.id).map((model) => (
                                  <article
                                    className="subagent-card"
                                    key={model.id}
                                  >
                                    <ProviderLogo provider={provider} />
                                    <div className="subagent-main">
                                      <div>
                                        <h3>{model.name}</h3>
                                        <Badge>{provider.name}</Badge>
                                      </div>
                                      <p>
                                        <code>{model.upstreamId}</code>
                                      </p>
                                    </div>
                                    <Toggle
                                      checked={selectedClientConfiguration.enabledModelIds.includes(
                                        model.id,
                                      )}
                                      disabled={
                                        Boolean(pending) ||
                                        selectedClientConfiguration.mainModelId ===
                                          model.id
                                      }
                                      label={`切换 ${selectedAdditionalClient.name} 模型 ${model.name}`}
                                      onChange={() =>
                                        commit(
                                          "set_client_model_enabled",
                                          {
                                            clientId:
                                              selectedAdditionalClient.id,
                                            id: model.id,
                                            enabled:
                                              !selectedClientConfiguration.enabledModelIds.includes(
                                                model.id,
                                              ),
                                          },
                                          `${model.name} 已${selectedClientConfiguration.enabledModelIds.includes(model.id) ? "移出" : "加入"}${selectedAdditionalClient.name}模型池。`,
                                        )
                                      }
                                    />
                                  </article>
                                )),
                              )}
                            </div>
                          </section>
                        )}
                        {selectedAdditionalClient.id === "kimi_code" && (
                          <section className="subagent-section">
                            <div className="config-heading">
                              <div>
                                <p className="kicker">可用 Agent</p>
                                <h2>Kimi Code Agent</h2>
                                <p>当前 CLI 可精确选择的内建与自定义 Agent。</p>
                              </div>
                            </div>
                            <div className="subagent-list">
                              {(selectedAdditionalStatus.agents ?? []).map(
                                (agent) => (
                                  <article
                                    className="subagent-card"
                                    key={agent.name}
                                  >
                                    <AgentLogo sourceClientId="kimi_code" />
                                    <div className="subagent-main">
                                      <div>
                                        <h3>{agent.name}</h3>
                                        <Badge tone="good">可调用</Badge>
                                      </div>
                                      <p>{agent.description || "未填写说明"}</p>
                                    </div>
                                  </article>
                                ),
                              )}
                            </div>
                          </section>
                        )}
                        {["gemini", "opencode", "kimi_code"].includes(
                          selectedAdditionalClient.id,
                        ) &&
                          extensionBindingsFor(
                            selectedAdditionalClient.id,
                            selectedAdditionalClient.name,
                          )}
                        <section className="agent-card">
                          <ClientLogo
                            clientId={selectedAdditionalClient.id}
                            name={selectedAdditionalClient.name}
                          />
                          <div className="agent-copy">
                            <div>
                              <h2>{selectedAdditionalClient.name}</h2>
                              <Badge tone={statusTone}>
                                {takeoverLabel(
                                  selectedAdditionalStatus.takeover,
                                )}
                              </Badge>
                            </div>
                            <p>
                              {selectedAdditionalStatus.takeover === "drifted"
                                ? `${takeoverDetail(selectedAdditionalStatus.takeover)}。重新应用将覆盖受管配置。`
                                : "保存模型后，点击应用配置即可生效。"}
                            </p>
                          </div>
                          <div className="agent-actions">
                            <button
                              className="button"
                              disabled={
                                Boolean(pending) ||
                                !selectedAdditionalStatus.installed ||
                                !selectedClientConfiguration.mainModelId
                              }
                              onClick={() =>
                                runClientIntegration(
                                  selectedAdditionalClient.id,
                                  selectedAdditionalClient.apply,
                                  `${selectedAdditionalClient.name} 模型配置已应用。`,
                                )
                              }
                            >
                              {takeoverActionLabel(
                                selectedAdditionalStatus.takeover,
                              )}
                            </button>
                            <button
                              className="button button--secondary"
                              disabled={
                                Boolean(pending) ||
                                (selectedAdditionalStatus.takeover ===
                                  "inactive" &&
                                  !selectedAdditionalStatus.snapshotPresent)
                              }
                              onClick={() =>
                                runClientIntegration(
                                  selectedAdditionalClient.id,
                                  selectedAdditionalClient.disable,
                                  `${selectedAdditionalClient.name} 模型配置已停用；扩展 SubAgent 绑定保持不变。`,
                                )
                              }
                            >
                              停用模型配置
                            </button>
                          </div>
                        </section>
                      </section>
                    );
                  })()}
              </>
            )}

            {view === "providers" && (
              <>
                <SectionTitle
                  kicker="供应商"
                  title="供应商与模型"
                  detail={`选择供应商预设，或添加自定义供应商。当前提供 ${catalog.presets.length} 个预设。`}
                  action={
                    <button
                      className="button"
                      disabled={Boolean(pending)}
                      onClick={
                        showProviderForm ? closeProviderForm : openNewProvider
                      }
                    >
                      {showProviderForm ? "取消" : "+ 添加供应商"}
                    </button>
                  }
                />
                {showProviderForm && (
                  <form className="inline-form" onSubmit={addProvider}>
                    <div className="form-heading">
                      <div>
                        <h2>
                          {editingProviderId ? "编辑供应商" : "新增供应商"}
                        </h2>
                        <p>
                          {editingProviderId
                            ? "API Key 留空将保留当前值。"
                            : "填写供应商连接信息。"}
                        </p>
                      </div>
                      <Badge>
                        {providerDraft.endpointMode === "base_url"
                          ? "Base URL"
                          : "精确 URL"}
                      </Badge>
                    </div>
                    <div className="selected-preset field-wide">
                      <BrandLogo
                        identity={`${selectedPreset?.id ?? "custom"} ${selectedPreset?.name ?? providerDraft.name}`}
                        name={selectedPreset?.name ?? "自定义配置"}
                      />
                      <div>
                        <strong>{selectedPreset?.name ?? "自定义配置"}</strong>
                        <span>
                          {selectedPreset
                            ? "已从供应商预设带入 Endpoint 与协议"
                            : "手动填写完整连接信息"}
                        </span>
                      </div>
                      {!editingProviderId && (
                        <button
                          type="button"
                          className="text-link"
                          onClick={openNewProvider}
                        >
                          重新选择
                        </button>
                      )}
                    </div>
                    <label>
                      供应商标识
                      <input
                        readOnly
                        value={editingProviderId ?? slug(providerDraft.name)}
                        placeholder="根据名称生成"
                      />
                    </label>
                    <label>
                      名称
                      <input
                        required
                        value={providerDraft.name}
                        onChange={(event) =>
                          setProviderDraft((current) => ({
                            ...current,
                            name: event.target.value,
                          }))
                        }
                        placeholder="本地网关"
                        autoFocus
                      />
                    </label>
                    <label>
                      协议
                      <select
                        value={providerDraft.protocol}
                        onChange={(event) => {
                          const protocol = event.target.value as Protocol;
                          setProviderDraft((current) => ({
                            ...current,
                            protocol,
                            apiKeyPlacement:
                              protocol === "gemini_native"
                                ? "x_api_key"
                                : current.apiKeyPlacement,
                            endpointMode:
                              protocol === "gemini_native"
                                ? "base_url"
                                : current.endpointMode,
                          }));
                        }}
                      >
                        <option value="anthropic_messages">
                          Anthropic Messages
                        </option>
                        <option value="open_ai_responses">
                          OpenAI Responses
                        </option>
                        <option value="open_ai_chat_completions">
                          OpenAI Chat
                        </option>
                        <option value="gemini_native">Gemini Native</option>
                      </select>
                    </label>
                    <label>
                      认证
                      <select
                        disabled={providerDraft.protocol === "gemini_native"}
                        value={providerDraft.apiKeyPlacement}
                        onChange={(event) =>
                          setProviderDraft((current) => ({
                            ...current,
                            apiKeyPlacement: event.target
                              .value as ApiKeyPlacement,
                          }))
                        }
                      >
                        <option value="bearer">Bearer</option>
                        <option value="x_api_key">API Key Header</option>
                        <option value="none">无认证 · 仅限本机回环</option>
                      </select>
                    </label>
                    <label>
                      Endpoint 模式
                      <select
                        value={providerDraft.endpointMode}
                        onChange={(event) =>
                          setProviderDraft((current) => ({
                            ...current,
                            endpointMode: event.target
                              .value as Provider["endpointMode"],
                          }))
                        }
                      >
                        <option value="base_url">Base URL</option>
                        <option value="exact_url">精确 URL</option>
                      </select>
                    </label>
                    <label>
                      模型列表 URL
                      <input
                        value={providerDraft.modelsUrl}
                        onChange={(event) =>
                          setProviderDraft((current) => ({
                            ...current,
                            modelsUrl: event.target.value,
                          }))
                        }
                        placeholder="留空则根据 Endpoint 自动推导"
                      />
                    </label>
                    {selectedPreset?.endpoint.kind === "parameterized" &&
                      selectedPreset.endpoint.parameters.map((parameter) => (
                        <label key={parameter.id} className="field-wide">
                          {parameter.label}
                          <input
                            required={parameter.required}
                            value={providerDraft.parameters[parameter.id] ?? ""}
                            onChange={(event) =>
                              setProviderDraft((current) => ({
                                ...current,
                                parameters: {
                                  ...current.parameters,
                                  [parameter.id]: event.target.value,
                                },
                              }))
                            }
                            placeholder={parameter.placeholder}
                          />
                        </label>
                      ))}
                    <label className="field-wide">
                      Endpoint
                      <input
                        required
                        value={providerEndpoint}
                        readOnly={
                          selectedPreset?.endpoint.kind === "parameterized"
                        }
                        onChange={(event) =>
                          setProviderDraft((current) => ({
                            ...current,
                            endpoint: event.target.value,
                          }))
                        }
                        placeholder="https://api.example.com/v1"
                      />
                    </label>
                    {providerDraft.apiKeyPlacement !== "none" && (
                      <label className="field-wide">
                        API Key
                        <input
                          required={!editingProvider?.credentialSet}
                          value={providerDraft.apiKey}
                          onChange={(event) =>
                            setProviderDraft((current) => ({
                              ...current,
                              apiKey: event.target.value,
                            }))
                          }
                          type="password"
                          placeholder={
                            editingProviderId
                              ? "留空保留；输入新值则轮换"
                              : "输入 API Key"
                          }
                          autoComplete="off"
                        />
                      </label>
                    )}
                    {selectedPreset &&
                      selectedPreset.suggested_models.length > 0 && (
                        <div className="preset-models field-wide">
                          <strong>建议模型 ID</strong>
                          <span>
                            {selectedPreset.suggested_models.join(" · ")}
                          </span>
                        </div>
                      )}
                    <div className="form-actions">
                      <span>
                        {providerDraft.apiKeyPlacement === "none"
                          ? "无认证供应商仅允许本机回环地址。"
                          : editingProviderId
                            ? "留空保留现有 Key；输入新值会轮换。"
                            : "API Key 保存后不会再次显示。"}
                      </span>
                      <button
                        className="button"
                        disabled={Boolean(pending)}
                        type="submit"
                      >
                        {editingProviderId
                          ? "更新供应商"
                          : pending === "save_provider_with_model_check"
                            ? "正在检查模型…"
                            : "检查并保存"}
                      </button>
                    </div>
                  </form>
                )}
                <div className="provider-toolbar">
                  <label>
                    <span>⌕</span>
                    <input
                      value={providerSearch}
                      onChange={(event) =>
                        setProviderSearch(event.target.value)
                      }
                      placeholder="搜索名称、协议或 Endpoint"
                    />
                  </label>
                  <div>
                    <strong>
                      {providers.filter((provider) => provider.enabled).length}
                    </strong>
                    <span>已启用</span>
                    <i />
                    <strong>{models.length}</strong>
                    <span>注册模型</span>
                  </div>
                </div>
                <section className="provider-card-grid">
                  {visibleProviders.map((provider) => {
                    const providerModels = models.filter(
                      (model) => model.providerId === provider.id,
                    );
                    const authLabel =
                      provider.apiKeyPlacement === "none"
                        ? "本机免认证"
                        : provider.credentialSet
                          ? "凭据已设置"
                          : "缺少凭据";
                    const deleting =
                      deleteTarget?.kind === "provider" &&
                      deleteTarget.id === provider.id;
                    return (
                      <article
                        className={`provider-card ${provider.enabled ? "provider-card--enabled" : "provider-card--disabled"}`}
                        key={provider.id}
                      >
                        <div className="provider-card-head">
                          <ProviderLogo provider={provider} size="large" />
                          <div>
                            <div>
                              <h3>{provider.name}</h3>
                              <Badge
                                tone={provider.enabled ? "good" : "neutral"}
                              >
                                {provider.enabled ? "已启用" : "已停用"}
                              </Badge>
                            </div>
                            <p>{provider.id}</p>
                          </div>
                          <Toggle
                            checked={provider.enabled}
                            disabled={Boolean(pending)}
                            label={`切换 ${provider.name}`}
                            onChange={() => toggleProvider(provider)}
                          />
                        </div>
                        <div className="provider-endpoint">
                          <span>API</span>
                          <code title={provider.endpoint}>
                            {provider.endpoint}
                          </code>
                        </div>
                        <div className="provider-protocols" aria-label="支持的调用方式">
                          <ProviderProtocolFacts provider={provider} />
                        </div>
                        <dl className="provider-card-meta">
                          <div>
                            <dt>协议</dt>
                            <dd>{protocolLabels[provider.protocol]}</dd>
                          </div>
                          <div>
                            <dt>模型</dt>
                            <dd>{providerModels.length} 个</dd>
                          </div>
                          <div>
                            <dt>认证</dt>
                            <dd
                              className={
                                !provider.credentialSet &&
                                provider.apiKeyPlacement !== "none"
                                  ? "meta-warning"
                                  : ""
                              }
                            >
                              {authLabel}
                            </dd>
                          </div>
                        </dl>
                        {providerUsage[provider.id] && (
                          <div className="provider-usage">
                            <strong>
                              {providerUsage[provider.id].kind === "balance"
                                ? "账户余额"
                                : "套餐用量"}
                            </strong>
                            <span>
                              {providerUsage[provider.id].items
                                .map(usageItemText)
                                .join(" · ")}
                            </span>
                          </div>
                        )}
                        <div className="provider-card-actions">
                          <button
                            className="action-button action-button--primary"
                            disabled={Boolean(pending)}
                            onClick={() => openProviderModels(provider)}
                          >
                            管理模型
                          </button>
                          <button
                            className="action-button action-button--test"
                            disabled={Boolean(pending) || !provider.enabled}
                            onClick={() => {
                              openProviderModels(provider);
                              void syncProviderModels(provider);
                            }}
                          >
                            {pending === "sync_provider_models"
                              ? "同步中…"
                              : "同步模型"}
                          </button>
                          <button
                            className="action-button"
                            disabled={Boolean(pending)}
                            onClick={() => editProvider(provider)}
                          >
                            编辑配置
                          </button>
                          {supportsUsageQuery(provider) && (
                            <button
                              className="action-button"
                              disabled={Boolean(pending) || !provider.enabled}
                              onClick={() => void queryProviderUsage(provider)}
                            >
                              {pending === `usage:${provider.id}`
                                ? "查询中…"
                                : "查询用量"}
                            </button>
                          )}
                          {deleting ? (
                            <>
                              <button
                                className="action-button"
                                disabled={Boolean(pending)}
                                onClick={() => setDeleteTarget(null)}
                              >
                                取消
                              </button>
                              <button
                                className="action-button action-button--danger"
                                disabled={Boolean(pending)}
                                onClick={() => confirmDelete(deleteTarget)}
                              >
                                确认删除
                              </button>
                            </>
                          ) : (
                            <button
                              className="action-button action-button--quiet"
                              disabled={Boolean(pending)}
                              onClick={() =>
                                setDeleteTarget({
                                  kind: "provider",
                                  id: provider.id,
                                })
                              }
                            >
                              删除
                            </button>
                          )}
                        </div>
                      </article>
                    );
                  })}
                  {!providerSearch.trim() && (
                    <button
                      type="button"
                      className="provider-add-card"
                      disabled={Boolean(pending)}
                      onClick={openNewProvider}
                    >
                      <span>＋</span>
                      <strong>添加供应商</strong>
                      <small>选择预设或自定义连接</small>
                    </button>
                  )}
                  {providers.length > 0 && visibleProviders.length === 0 && (
                    <div className="provider-search-empty">
                      <strong>没有匹配的供应商</strong>
                      <span>换一个名称、协议或 Endpoint 关键词。</span>
                    </div>
                  )}
                </section>
              </>
            )}

            {view === "routes" && (
              <>
                <SectionTitle
                  kicker="模型路由"
                  title="路由概览"
                  detail="查看各客户端当前选择的供应商和模型。"
                />
                <section className="route-status-grid">
                  <article>
                    <ClientLogo clientId="claude_code" name="Claude Code" />
                    <div>
                      <strong>Claude Code</strong>
                      <small>
                        {Object.keys(state.modelSlots).length} 个槽位 ·{" "}
                        {(state.clientExtensionSubagentIds.claude_code ?? []).length} 个扩展
                      </small>
                    </div>
                    <Badge tone={integrationTone}>
                      {takeoverLabel(integration.takeover)}
                    </Badge>
                  </article>
                  <article>
                    <ClientLogo clientId="claude_desktop" name="Claude Client" />
                    <div>
                      <strong>Claude Client 对话 / Cowork</strong>
                      <small>
                        {Object.keys(state.claudeDesktopModelSlots).length}{" "}
                        个对话角色模型
                      </small>
                    </div>
                    <Badge tone={desktopTone}>
                      {takeoverLabel(claudeDesktop.takeover)}
                    </Badge>
                  </article>
                  <article>
                    <ClientLogo clientId="pi" name="Pi" />
                    <div>
                      <strong>Pi</strong>
                      <small>
                        {state.piEnabledModelIds.length} 个可用模型 ·{" "}
                        {state.piMainModelId ? "已设默认" : "未设默认"}
                      </small>
                    </div>
                    <Badge tone={piTone}>
                      {takeoverLabel(piStatus.takeover)}
                    </Badge>
                  </article>
                </section>
                <section className="route-status-grid">
                  <article>
                    <ClientLogo clientId="codex" name="Codex" />
                    <div>
                      <strong>Codex</strong>
                      <small>
                        {state.codexMainModelId ? "已设默认模型" : "跟随原生"}
                      </small>
                    </div>
                    <Badge tone={codexTone}>
                      {takeoverLabel(codexStatus.takeover)}
                    </Badge>
                  </article>
                  {additionalClients.map((client) => {
                    const status = clientStatuses[client.id];
                    const config = state.clientConfigurations[client.id];
                    return (
                      <article key={client.id}>
                        <ClientLogo clientId={client.id} name={client.name} />
                        <div>
                          <strong>{client.name}</strong>
                          <small>
                            {config.mainModelId
                              ? `${config.enabledModelIds.length || 1} 个模型`
                              : "跟随原生"}
                          </small>
                        </div>
                        <Badge tone={takeoverTone(status.takeover)}>
                          {takeoverLabel(status.takeover)}
                        </Badge>
                      </article>
                    );
                  })}
                </section>
                <section className="relation-map">
                  <div className="relation-column">
                    <p>客户端</p>
                    <article className="relation-node relation-node--accent">
                      <ClientLogo clientId="claude_code" name="Claude Code" />
                      <div>
                        <strong>Claude Code CLI</strong>
                        <small>{takeoverLabel(integration.takeover)}</small>
                      </div>
                    </article>
                    <article className="relation-node relation-node--accent">
                      <ClientLogo clientId="claude_desktop" name="Claude Client" />
                      <div>
                        <strong>Claude Client</strong>
                        <small>{takeoverLabel(claudeDesktop.takeover)}</small>
                      </div>
                    </article>
                    <article className="relation-node relation-node--accent">
                      <ClientLogo clientId="pi" name="Pi" />
                      <div>
                        <strong>Pi</strong>
                        <small>{takeoverLabel(piStatus.takeover)}</small>
                      </div>
                    </article>
                  </div>
                  <div className="relation-arrow" aria-hidden="true">
                    →
                  </div>
                  <div className="relation-column">
                    <p>槽位 / SubAgent</p>
                    {integration.supportedModelSlots.map((slot) => (
                      <article className="relation-node" key={slot}>
                        <strong>{modelSlotLabels[slot] ?? slot}</strong>
                        <small>Claude Code</small>
                      </article>
                    ))}
                    {extensionSubagents.map((subagent) => (
                      <article className="relation-node" key={subagent.id}>
                        <strong>{subagent.name}</strong>
                        <small>
                          {subagent.capabilities.join(" · ") || "未标注能力"}
                        </small>
                      </article>
                    ))}
                    <article className="relation-node">
                      <strong>Client 对话角色</strong>
                      <small>Claude Client</small>
                    </article>
                    <article className="relation-node">
                      <strong>Pi 默认 / 模型池</strong>
                      <small>{state.piEnabledModelIds.length} 个模型</small>
                    </article>
                  </div>
                  <div className="relation-arrow" aria-hidden="true">
                    →
                  </div>
                  <div className="relation-column">
                    <p>模型 / 供应商</p>
                    {integration.supportedModelSlots.map((slot) => {
                      const model = models.find(
                        (item) => item.id === state.modelSlots[slot],
                      );
                      const provider = providers.find(
                        (item) => item.id === model?.providerId,
                      );
                      return (
                        <article className="relation-node" key={slot}>
                          <strong>{model?.name ?? "跟随原生"}</strong>
                          <small>
                            {provider
                              ? `${provider.name} · ${model?.upstreamId}`
                              : (modelSlotLabels[slot] ?? slot)}
                          </small>
                        </article>
                      );
                    })}
                    {extensionSubagents.map((subagent) => {
                      const model = models.find(
                        (item) => item.id === subagent.modelId,
                      );
                      const provider = providers.find(
                        (item) => item.id === model?.providerId,
                      );
                      return (
                        <article className="relation-node" key={subagent.id}>
                          <strong>{model?.name ?? "模型缺失"}</strong>
                          <small>
                            {provider?.name ?? "跟随来源原生"}
                          </small>
                        </article>
                      );
                    })}
                    <article className="relation-node">
                      <strong>
                        {Object.keys(state.claudeDesktopModelSlots).length} 个
                        Client 角色
                      </strong>
                      <small>
                        {Object.entries(state.claudeDesktopModelSlots)
                          .map(
                            ([slot, id]) =>
                              `${slot}: ${models.find((model) => model.id === id)?.name ?? "缺失"}`,
                          )
                          .join(" · ") || "全部跟随原生"}
                      </small>
                    </article>
                    <article className="relation-node">
                      <strong>
                        {models.find(
                          (model) => model.id === state.piMainModelId,
                        )?.name ?? "Pi 跟随原生"}
                      </strong>
                      <small>
                        {state.piEnabledModelIds
                          .map(
                            (id) =>
                              models.find((model) => model.id === id)?.name ??
                              "缺失",
                          )
                          .join(" · ") || "模型池为空"}
                      </small>
                    </article>
                  </div>
                </section>
              </>
            )}
          </>
        )}
      </main>
      {managingProvider && state && (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={closeProviderModels}
        >
          <section
            className="provider-model-manager"
            role="dialog"
            aria-modal="true"
            aria-labelledby="provider-model-manager-title"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header>
              <ProviderLogo provider={managingProvider} size="large" />
              <div>
                <p className="kicker">供应商模型</p>
                <h2 id="provider-model-manager-title">
                  {managingProvider.name}
                </h2>
                <p>
                  自动同步会为每个模型发送最小测试请求，并记录可直连与需桥接的协议。
                </p>
              </div>
              <button
                className="modal-close"
                aria-label="关闭供应商模型"
                onClick={closeProviderModels}
              >
                ×
              </button>
            </header>
            <div className="provider-surface-summary">
              <strong>供应商支持的调用方式</strong>
              <div className="provider-protocols">
                <ProviderProtocolFacts provider={managingProvider} />
              </div>
            </div>
            <div className="model-manager-toolbar">
              <label>
                <span>⌕</span>
                <input
                  value={modelSearch}
                  onChange={(event) => setModelSearch(event.target.value)}
                  placeholder="搜索已导入模型"
                />
              </label>
              <div>
                <button
                  className="button button--secondary"
                  disabled={Boolean(pending) || !managingProvider.enabled}
                  onClick={() => syncProviderModels(managingProvider)}
                >
                  {pending === "sync_provider_models"
                    ? "正在同步…"
                    : "↻ 自动同步"}
                </button>
                <button
                  className="button"
                  disabled={Boolean(pending)}
                  onClick={() =>
                    showModelForm
                      ? closeModelForm()
                      : openNewModel(managingProvider.id)
                  }
                >
                  {showModelForm ? "取消手动录入" : "+ 手动添加"}
                </button>
              </div>
            </div>
            {showModelForm && (
              <form
                className="inline-form inline-form--model model-manager-form"
                onSubmit={addModel}
              >
                <div className="form-heading">
                  <div>
                    <h2>{editingModelId ? "编辑模型" : "手动添加模型"}</h2>
                    <p>设置模型名称、任务能力和高级选项。</p>
                  </div>
                  <Badge>{managingProvider.name}</Badge>
                </div>
                <label>
                  模型标识
                  <input
                    readOnly
                    value={editingModelId ?? slug(modelDraft.name)}
                    placeholder="根据名称生成"
                  />
                </label>
                <label>
                  显示名称
                  <input
                    required
                    value={modelDraft.name}
                    onChange={(event) =>
                      setModelDraft((current) => ({
                        ...current,
                        name: event.target.value,
                      }))
                    }
                    placeholder="Coder Pro"
                    autoFocus
                  />
                </label>
                <label className="field-wide">
                  模型 ID
                  <input
                    required
                    value={modelDraft.upstreamId}
                    onChange={(event) =>
                      setModelDraft((current) => ({
                        ...current,
                        upstreamId: event.target.value,
                      }))
                    }
                    placeholder="vendor/model-id"
                  />
                </label>
                <label className="field-wide">
                  任务能力
                  <input
                    value={modelDraft.capabilities}
                    onChange={(event) =>
                      setModelDraft((current) => ({
                        ...current,
                        capabilities: event.target.value,
                      }))
                    }
                    placeholder="coding, review, refactor（逗号分隔）"
                  />
                </label>
                <label className="field-wide">
                  上下文长度
                  <input
                    inputMode="numeric"
                    value={modelDraft.contextWindow}
                    onChange={(event) =>
                      setModelDraft((current) => ({
                        ...current,
                        contextWindow: event.target.value,
                      }))
                    }
                    placeholder="例如 262144；留空表示未知，客户端沿用自身默认值"
                  />
                </label>
                <fieldset className="protocol-features field-wide">
                  <legend>模型原生 API 协议</legend>
                  <p>仅勾选该模型真实可直接调用的协议；GrillForge 会为客户端自动桥接其余协议。</p>
                  <div>
                    {(Object.entries(nativeProtocolLabels) as [NativeProtocol, string][]).map(
                      ([protocol, label]) => (
                        <label key={protocol}>
                          <input
                            type="checkbox"
                            checked={modelDraft.nativeProtocols.includes(protocol)}
                            onChange={() =>
                              setModelDraft((current) => ({
                                ...current,
                                nativeProtocols: current.nativeProtocols.includes(protocol)
                                  ? current.nativeProtocols.filter((value) => value !== protocol)
                                  : [...current.nativeProtocols, protocol],
                              }))
                            }
                          />
                          {label}
                        </label>
                      ),
                    )}
                  </div>
                </fieldset>
                <details className="protocol-features field-wide">
                  <summary>高级模型选项</summary>
                  <div>
                    {protocolFeatures.map((feature) => (
                      <label key={feature.id}>
                        <input
                          type="checkbox"
                          checked={modelDraft.protocolCapabilities.includes(
                            feature.id,
                          )}
                          onChange={() => toggleProtocolFeature(feature.id)}
                        />
                        {feature.label}
                      </label>
                    ))}
                  </div>
                </details>
                <input type="hidden" value={modelDraft.providerId} />
                <div className="form-actions">
                  <span>确认模型信息后保存。</span>
                  <button
                    className="button"
                    disabled={Boolean(pending)}
                    type="submit"
                  >
                    {editingModelId ? "保存修改" : "添加模型"}
                  </button>
                </div>
              </form>
            )}
            <section className="provider-model-list">
              <div className="registry-head">
                <div>
                  <p className="kicker">已导入</p>
                  <h2>{managingProviderModels.length} 个模型</h2>
                </div>
              </div>
              {managingProviderModels.length === 0 ? (
                <div className="subagent-empty">
                  <span>◉</span>
                  <strong>还没有模型</strong>
                  <p>可以自动同步，也可以手动录入。</p>
                </div>
              ) : (
                managingProviderModels
                  .filter((model) => {
                    const query = modelSearch.trim().toLowerCase();
                    return (
                      !query ||
                      `${model.name} ${model.upstreamId} ${model.id}`
                        .toLowerCase()
                        .includes(query)
                    );
                  })
                  .map((model) => {
                    const referenceCount = extensionSubagents.filter(
                      (subagent) => subagent.modelId === model.id,
                    ).length;
                    const deleting =
                      deleteTarget?.kind === "model" &&
                      deleteTarget.id === model.id;
                    return (
                      <article className="provider-model-row" key={model.id}>
                        <button
                          className="model-row-main"
                          onClick={() => setSelectedModelId(model.id)}
                        >
                          <ProviderLogo provider={managingProvider} />
                          <span>
                            <strong>{model.name}</strong>
                            <code>{model.upstreamId}</code>
                          </span>
                        </button>
                        <div className="tag-cloud">
                          {model.capabilities.slice(0, 3).map((capability) => (
                            <Badge key={capability}>{capability}</Badge>
                          ))}
                          <ProviderProtocolFacts
                            provider={managingProvider}
                            model={model}
                          />
                          {referenceCount > 0 && (
                            <Badge tone="good">
                              {referenceCount} 个 SubAgent
                            </Badge>
                          )}
                        </div>
                        <div className="row-actions">
                          <button
                            className="action-button action-button--test"
                            disabled={
                              Boolean(pending) || !managingProvider.enabled
                            }
                            onClick={() => testConnection(model)}
                          >
                            {pending === `test:${model.id}`
                              ? "测试中…"
                              : "测试"}
                          </button>
                          <button
                            className="action-button"
                            disabled={Boolean(pending)}
                            onClick={() => editModel(model)}
                          >
                            编辑
                          </button>
                          {deleting ? (
                            <>
                              <button
                                className="action-button"
                                onClick={() => setDeleteTarget(null)}
                              >
                                取消
                              </button>
                              <button
                                className="action-button action-button--danger"
                                disabled={Boolean(pending)}
                                onClick={() => confirmDelete(deleteTarget)}
                              >
                                确认删除
                              </button>
                            </>
                          ) : (
                            <button
                              className="action-button action-button--danger"
                              disabled={Boolean(pending)}
                              onClick={() =>
                                setDeleteTarget({ kind: "model", id: model.id })
                              }
                            >
                              删除
                            </button>
                          )}
                        </div>
                      </article>
                    );
                  })
              )}
            </section>
          </section>
        </div>
      )}
      {providerPickerOpen && (
        <div
          className="modal-backdrop provider-picker-backdrop"
          role="presentation"
          onMouseDown={() => setProviderPickerOpen(false)}
        >
          <section
            className="provider-picker"
            role="dialog"
            aria-modal="true"
            aria-labelledby="provider-picker-title"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header>
              <button
                className="picker-back"
                aria-label="关闭供应商选择"
                onClick={() => setProviderPickerOpen(false)}
              >
                ←
              </button>
              <div>
                <p className="kicker">供应商预设</p>
                <h2 id="provider-picker-title">添加新供应商</h2>
                <p>选择供应商预设，或从自定义配置开始。</p>
              </div>
            </header>
            <div className="picker-search">
              <span>⌕</span>
              <input
                autoFocus
                value={presetSearch}
                onChange={(event) => setPresetSearch(event.target.value)}
                placeholder="搜索供应商或模型"
              />
              <small>{visiblePresets.length} 个预设</small>
            </div>
            <div className="preset-gallery">
              <button
                className="preset-choice preset-choice--custom"
                onClick={chooseCustomProvider}
              >
                <span className="custom-preset-icon">＋</span>
                <strong>自定义配置</strong>
                <small>手动填写连接信息</small>
              </button>
              <button
                className="preset-choice"
                onClick={chooseAnthropicProvider}
              >
                <BrandLogo identity="anthropic" name="Anthropic API" />
                <span>
                  <strong>Anthropic API</strong>
                  <small>Anthropic Messages · 官方模型同步</small>
                </span>
                <i>›</i>
              </button>
              <button className="preset-choice" onClick={chooseOpenAiProvider}>
                <BrandLogo identity="openai" name="OpenAI API" />
                <span>
                  <strong>OpenAI API</strong>
                  <small>OpenAI Responses · 官方模型同步</small>
                </span>
                <i>›</i>
              </button>
              <button className="preset-choice" onClick={chooseGeminiProvider}>
                <BrandLogo identity="google gemini" name="Google Gemini API" />
                <span>
                  <strong>Google Gemini API</strong>
                  <small>Gemini Native · 官方模型同步</small>
                </span>
                <i>›</i>
              </button>
              {visiblePresets.map((preset) => (
                <button
                  className="preset-choice"
                  key={preset.id}
                  onClick={() => chooseProviderPreset(preset.id)}
                >
                  <BrandLogo
                    identity={`${preset.id} ${preset.name}`}
                    name={preset.name}
                  />
                  <span>
                    <strong>{preset.name}</strong>
                    <small>
                      {protocolLabels[providerProtocol(preset.protocol)]} ·{" "}
                      {preset.suggested_models.slice(0, 2).join(" · ") ||
                        "自定义模型"}
                    </small>
                  </span>
                  <i>›</i>
                </button>
              ))}
            </div>
            {visiblePresets.length === 0 && (
              <div className="provider-search-empty">
                <strong>没有匹配的预设</strong>
                <span>可以清空搜索或选择自定义配置。</span>
              </div>
            )}
            <footer>
              <span>选择后继续填写 API Key 等必要字段。</span>
              <button
                className="button button--secondary"
                onClick={() => setProviderPickerOpen(false)}
              >
                取消
              </button>
            </footer>
          </section>
        </div>
      )}
      {selectedModel && (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={() => setSelectedModelId(null)}
        >
          <section
            className="model-detail"
            role="dialog"
            aria-modal="true"
            aria-labelledby="model-detail-title"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <button
              className="modal-close"
              aria-label="关闭模型详情"
              onClick={() => setSelectedModelId(null)}
            >
              ×
            </button>
            <div className="model-detail-head">
              {selectedModelProvider ? (
                <ProviderLogo provider={selectedModelProvider} size="large" />
              ) : (
                <span className="model-avatar">
                  <AppLogo />
                </span>
              )}
              <div>
                <p className="kicker">模型详情</p>
                <h2 id="model-detail-title">{selectedModel.name}</h2>
                <div className="badge-line">
                  <Badge>{selectedModelProvider?.name ?? "供应商缺失"}</Badge>
                  {selectedModelProvider && (
                    <Badge>
                      {protocolLabels[selectedModelProvider.protocol]}
                    </Badge>
                  )}
                  {extensionSubagents.filter(
                    (subagent) => subagent.modelId === selectedModel.id,
                  ).length > 0 && (
                    <Badge tone="good">
                      {
                        extensionSubagents.filter(
                          (subagent) => subagent.modelId === selectedModel.id,
                        ).length
                      }{" "}
                      个 SubAgent
                    </Badge>
                  )}
                </div>
              </div>
            </div>
            <div className="detail-section">
              <strong>推荐用途</strong>
              <p>{recommendedUse(selectedModel)}</p>
            </div>
            <div className="detail-section">
              <strong>能力标签</strong>
              <div className="tag-cloud">
                {selectedModel.capabilities.length > 0 ? (
                  selectedModel.capabilities.map((capability) => (
                    <Badge key={capability}>{capability}</Badge>
                  ))
                ) : (
                  <span>暂无能力标签</span>
                )}
              </div>
            </div>
            <dl className="detail-grid">
              <div>
                <dt>模型 ID</dt>
                <dd>
                  <code>{selectedModel.upstreamId}</code>
                </dd>
              </div>
              <div>
                <dt>连接状态</dt>
                <dd>{connections[selectedModel.id] ? "已验证" : "尚未测试"}</dd>
              </div>
              <div>
                <dt>客户端路由</dt>
                <dd>
                  {selectedModelProvider
                    ? routeSupportLabel(selectedModelProvider.protocol)
                    : "供应商配置缺失"}
                </dd>
              </div>
            </dl>
          </section>
        </div>
      )}
      {restartPromptClient && (
        <div className="modal-backdrop" role="presentation">
          <section
            className="pi-mcp-install-confirm"
            role="dialog"
            aria-modal="true"
            aria-labelledby="claude-restart-title"
          >
            <p className="kicker">需要重启</p>
            <h2 id="claude-restart-title">
              重新打开 {restartPromptClient === "codex" ? "Codex" : "Claude Client"}
            </h2>
            <p>
              {restartPromptClient === "codex" ? "Codex" : "Claude Client"}
              需要重启一次，才能加载刚刚更新的配置。
            </p>
            {restartClientError && (
              <p className="pi-mcp-install-error" role="alert">
                {restartClientError}
              </p>
            )}
            <footer>
              <button
                className="button button--secondary"
                disabled={pending.startsWith("restart_")}
                onClick={() => setRestartPromptClient(null)}
              >
                暂不处理
              </button>
              <button
                className="button"
                disabled={pending.startsWith("restart_")}
                onClick={() => void restartDesktopClient()}
              >
                {pending.startsWith("restart_") ? "正在重启…" : "立即重启"}
              </button>
            </footer>
          </section>
        </div>
      )}
    </div>
  );
}

export default App;

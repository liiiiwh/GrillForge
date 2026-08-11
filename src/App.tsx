import { invoke } from "@tauri-apps/api/core";
import { FormEvent, ReactNode, useEffect, useMemo, useState } from "react";
import "./App.css";
import { DEFAULT_LOCALE, createTranslator } from "./i18n";
import anthropicIcon from "../upstream/cc-switch/src/icons/extracted/anthropic.svg";
import azureIcon from "../upstream/cc-switch/src/icons/extracted/azure.svg";
import deepseekIcon from "../upstream/cc-switch/src/icons/extracted/deepseek.svg";
import googleIcon from "../upstream/cc-switch/src/icons/extracted/google.svg";
import ollamaIcon from "../upstream/cc-switch/src/icons/extracted/ollama.svg";
import openaiIcon from "../upstream/cc-switch/src/icons/extracted/openai.svg";
import openrouterIcon from "../upstream/cc-switch/src/icons/extracted/openrouter.svg";
import qwenIcon from "../upstream/cc-switch/src/icons/extracted/qwen.svg";

const t = createTranslator(DEFAULT_LOCALE);

type View = "overview" | "clients" | "providers" | "routes";
type ClientTab = "slots" | "subagents";
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
};

type Model = {
  id: string;
  name: string;
  upstreamId: string;
  providerId: string;
  capabilities: string[];
  protocolCapabilities: ProtocolCapability[];
  workerEnabled: boolean;
  routeAlias: string;
};

type SubAgent = {
  id: string;
  name: string;
  modelId: string;
  capabilities: string[];
  enabled: boolean;
};

type ControlPlaneState = {
  providers: Provider[];
  models: Model[];
  agentEnabled: boolean;
  mainModelId: string | null;
  modelSlots: Record<string, string>;
  claudeDesktopModelSlots: Record<string, string>;
  piEnabled: boolean;
  piMainModelId: string | null;
  piEnabledModelIds: string[];
  codexMainModelId: string | null;
  codexNativeModelSlots: Record<string, string>;
  codexAgentModelIds: Record<string, string>;
  clientConfigurations: Record<string, ClientConfiguration>;
  workerMode: boolean;
  nativeSubagentEnabled: boolean;
  subagents: SubAgent[];
};

type ClientConfiguration = {
  mainModelId: string | null;
  secondaryModelId: string | null;
  enabledModelIds: string[];
};

type KimiCodeAgent = {
  name: string;
  description: string;
  modelPreference: "primary" | "secondary" | null;
  builtIn: boolean;
  source: string | null;
};

type ClientIntegrationStatus = {
  installed: boolean;
  executablePath: string | null;
  version: string | null;
  snapshotPresent: boolean;
  takeover: Takeover;
  configuredModelIds: string[];
  mainModelId: string | null;
  secondaryModelId?: string | null;
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
    id: "openclaw",
    name: "OpenClaw",
    mark: "CL",
    status: "openclaw_status",
    apply: "apply_openclaw",
    disable: "disable_openclaw",
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
  forcedWorkerAlias: string | null;
  generatedAgentNames: string[];
  selectorSkillInstalled: boolean;
  supportedModelSlots: string[];
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
  codeUsesClaudeCodeConfiguration: boolean;
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
};

type SubAgentDraft = {
  id: string;
  name: string;
  providerId: string;
  modelId: string;
  capabilities: string;
  enabled: boolean;
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

type DiscoveredModel = {
  id: string;
  ownedBy: string | null;
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
};

const EMPTY_SUBAGENT: SubAgentDraft = {
  id: "",
  name: "",
  providerId: "",
  modelId: "",
  capabilities: "",
  enabled: true,
};

const protocolFeatures: Array<{ id: ProtocolCapability; label: string }> = [
  { id: "reasoning_items", label: "推理条目" },
  { id: "reasoning_content", label: "推理内容" },
  { id: "reasoning_effort", label: "推理强度" },
];

const views: Array<{ id: View; label: string; icon: string }> = [
  { id: "overview", label: t("overview"), icon: "⌂" },
  { id: "clients", label: t("clients"), icon: "◇" },
  { id: "providers", label: t("providers"), icon: "◎" },
  { id: "routes", label: "路由策略", icon: "⌘" },
];

const clientTabs: Array<{ id: ClientTab; label: string; capability?: string }> =
  [
    { id: "slots", label: "Claude 模型槽位" },
    { id: "subagents", label: "SubAgent" },
  ];

const modelSlotLabels: Record<string, string> = {
  sonnet: t("sonnetSlot"),
  opus: t("opusSlot"),
  fable: t("fableSlot"),
  haiku: t("haikuSlot"),
};

const protocolLabels: Record<Protocol, string> = {
  anthropic_messages: "Anthropic Messages",
  open_ai_responses: "OpenAI Responses",
  open_ai_chat_completions: "OpenAI Chat",
  gemini_native: "Gemini Native",
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
  [["qwen", "dashscope", "alibaba", "aliyun"], qwenIcon],
  [["openrouter"], openrouterIcon],
  [["ollama"], ollamaIcon],
  [["google", "gemini"], googleIcon],
  [["azure"], azureIcon],
];

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
  const initials = name
    .split(/\s+/)
    .map((part) => part[0])
    .join("")
    .slice(0, 2)
    .toUpperCase();
  return (
    <span className={`provider-logo provider-logo--${size}`} title={name}>
      {icon ? <img src={icon} alt="" /> : initials || "API"}
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
  const [codexStatus, setCodexStatus] = useState<CodexStatus | null>(null);
  const [clientStatuses, setClientStatuses] = useState<Record<
    string,
    ClientIntegrationStatus
  > | null>(null);
  const [catalog, setCatalog] = useState<ProviderPresetCatalog | null>(null);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
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
  const [subAgentDraft, setSubAgentDraft] =
    useState<SubAgentDraft>(EMPTY_SUBAGENT);
  const [editingSubAgentId, setEditingSubAgentId] = useState<string | null>(
    null,
  );
  const [showSubAgentForm, setShowSubAgentForm] = useState(false);
  const [managingProviderId, setManagingProviderId] = useState<string | null>(
    null,
  );
  const [discoveredModels, setDiscoveredModels] = useState<DiscoveredModel[]>(
    [],
  );
  const [selectedDiscovered, setSelectedDiscovered] = useState<string[]>([]);
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
  const [
    clientSecondaryProviderSelections,
    setClientSecondaryProviderSelections,
  ] = useState<Record<string, string>>({});
  const [refreshingClients, setRefreshingClients] = useState(false);

  useEffect(() => {
    let active = true;
    Promise.all([
      invoke<ControlPlaneState>("load_state"),
      invoke<ProviderPresetCatalog>("provider_presets"),
      invoke<IntegrationStatus>("integration_status"),
      invoke<ClaudeCliStatus>("detect_claude_code"),
      invoke<ClaudeDesktopStatus>("claude_desktop_status"),
      invoke<PiStatus>("pi_status"),
      invoke<CodexStatus>("codex_status"),
      Promise.all(
        additionalClients.map(
          async (client) =>
            [
              client.id,
              await invoke<ClientIntegrationStatus>(client.status),
            ] as const,
        ),
      ),
    ])
      .then(
        ([
          loaded,
          presets,
          integrationStatus,
          cliStatus,
          desktopStatus,
          loadedPiStatus,
          loadedCodexStatus,
          loadedClientStatuses,
        ]) => {
          if (!active) return;
          setState(loaded);
          setCatalog(presets);
          setIntegration(integrationStatus);
          setClaudeCli(cliStatus);
          setClaudeDesktop(desktopStatus);
          setPiStatus(loadedPiStatus);
          setCodexStatus(loadedCodexStatus);
          setClientStatuses(Object.fromEntries(loadedClientStatuses));
        },
      )
      .catch((cause) => {
        if (active) setError(errorMessage(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
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
  const gatewayProviders = providers.filter(
    (provider) => provider.enabled && provider.protocol !== "gemini_native",
  );
  const gatewayProviderIds = new Set(
    gatewayProviders.map((provider) => provider.id),
  );
  const gatewayModels = models.filter((model) =>
    gatewayProviderIds.has(model.providerId),
  );
  const effectiveSubAgents = (state?.subagents ?? []).filter(
    (subagent) => subagent.enabled,
  );
  const effectiveSubAgentCount =
    effectiveSubAgents.length + (state?.nativeSubagentEnabled ? 1 : 0);
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
    command: "apply_claude_desktop" | "disable_claude_desktop",
    success: string,
  ) {
    if (!begin(command)) return false;
    try {
      const persisted = await invoke<ClaudeDesktopStatus>(command);
      setClaudeDesktop(persisted);
      setNotice(success);
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

  async function setKimiAgentPreference(
    name: string,
    preference: "primary" | "secondary",
  ) {
    if (!begin(`kimi-agent:${name}`)) return;
    try {
      const agents = await invoke<KimiCodeAgent[]>(
        "set_kimi_code_agent_model_preference_command",
        { name, preference },
      );
      setClientStatuses((current) => ({
        ...(current ?? {}),
        kimi_code: {
          ...(current?.kimi_code as ClientIntegrationStatus),
          agents,
        },
      }));
      setNotice(`${name} 已切换到 ${preference === "primary" ? "Primary" : "Secondary"} 模型。`);
    } catch (cause) {
      reportError(errorMessage(cause));
    } finally {
      setPending("");
    }
  }

  async function refreshClients() {
    if (refreshingClients) return;
    setRefreshingClients(true);
    try {
      const [
        cliStatus,
        desktopStatus,
        loadedPiStatus,
        loadedCodexStatus,
        loadedClientStatuses,
      ] = await Promise.all([
        invoke<ClaudeCliStatus>("detect_claude_code"),
        invoke<ClaudeDesktopStatus>("claude_desktop_status"),
        invoke<PiStatus>("pi_status"),
        invoke<CodexStatus>("codex_status"),
        Promise.all(
          additionalClients.map(
            async (client) =>
              [
                client.id,
                await invoke<ClientIntegrationStatus>(client.status),
              ] as const,
          ),
        ),
      ]);
      setClaudeCli(cliStatus);
      setClaudeDesktop(desktopStatus);
      setPiStatus(loadedPiStatus);
      setCodexStatus(loadedCodexStatus);
      setClientStatuses(Object.fromEntries(loadedClientStatuses));
    } catch (cause) {
      reportError(errorMessage(cause));
    } finally {
      setRefreshingClients(false);
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
    requestAnimationFrame(() => {
      document
        .querySelector<HTMLElement>(".workspace")
        ?.scrollTo({ top: 0, left: 0 });
    });
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
    });
    setShowModelForm(true);
  }

  function openProviderModels(provider: Provider) {
    closeProviderForm();
    closeModelForm();
    setManagingProviderId(provider.id);
    setDiscoveredModels([]);
    setSelectedDiscovered([]);
    setModelSearch("");
  }

  function closeProviderModels() {
    closeModelForm();
    setManagingProviderId(null);
    setDiscoveredModels([]);
    setSelectedDiscovered([]);
  }

  async function discoverModels(provider: Provider) {
    if (!begin(`discover:${provider.id}`)) return;
    try {
      const discovered = await invoke<DiscoveredModel[]>(
        "discover_provider_models",
        { providerId: provider.id },
      );
      setDiscoveredModels(discovered);
      const imported = new Set(
        models
          .filter((model) => model.providerId === provider.id)
          .map((model) => model.upstreamId),
      );
      setSelectedDiscovered(
        discovered
          .filter((model) => !imported.has(model.id))
          .map((model) => model.id),
      );
      setNotice(`已从 ${provider.name} 获取 ${discovered.length} 个模型。`);
    } catch (cause) {
      reportError(errorMessage(cause));
    } finally {
      setPending("");
    }
  }

  function toggleDiscovered(id: string) {
    setSelectedDiscovered((current) =>
      current.includes(id)
        ? current.filter((modelId) => modelId !== id)
        : [...current, id],
    );
  }

  async function importDiscovered(provider: Provider) {
    const selected = discoveredModels.filter((model) =>
      selectedDiscovered.includes(model.id),
    );
    if (selected.length === 0)
      return reportError("请至少选择一个尚未导入的模型。");
    if (
      await commit(
        "import_provider_models",
        { providerId: provider.id, models: selected },
        `已导入 ${selected.length} 个模型。`,
      )
    ) {
      setDiscoveredModels([]);
      setSelectedDiscovered([]);
    }
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
    const command = editingProviderId ? "update_provider" : "save_provider";
    if (
      await commit(
        command,
        { input },
        `${name} 已${editingProviderId ? "更新" : "保存"}。`,
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
    };
    const command = editingModelId ? "update_model" : "save_model";
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

  function openNewSubAgent() {
    setEditingSubAgentId(null);
    setSubAgentDraft(EMPTY_SUBAGENT);
    setShowSubAgentForm(true);
  }

  function editSubAgent(subagent: SubAgent) {
    const model = models.find((item) => item.id === subagent.modelId);
    setEditingSubAgentId(subagent.id);
    setSubAgentDraft({
      id: subagent.id,
      name: subagent.name,
      providerId: model?.providerId ?? "",
      modelId: subagent.modelId,
      capabilities: subagent.capabilities.join(", "),
      enabled: subagent.enabled,
    });
    setShowSubAgentForm(true);
  }

  function closeSubAgentForm() {
    setEditingSubAgentId(null);
    setSubAgentDraft(EMPTY_SUBAGENT);
    setShowSubAgentForm(false);
  }

  async function saveSubAgent(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const id = subAgentDraft.id.trim();
    const name = subAgentDraft.name.trim();
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(id)) {
      return reportError(
        "SubAgent ID 必须是小写字母、数字和单个连字符组成的稳定标识。",
      );
    }
    const input = {
      id,
      name,
      modelId: subAgentDraft.modelId,
      capabilities: subAgentDraft.capabilities
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
      enabled: subAgentDraft.enabled,
    };
    const command = editingSubAgentId ? "update_subagent" : "save_subagent";
    if (
      await commit(
        command,
        { input },
        `${name} SubAgent 已${editingSubAgentId ? "更新" : "添加"}。`,
      )
    ) {
      closeSubAgentForm();
    }
  }

  async function toggleSubAgent(subagent: SubAgent) {
    await commit(
      "update_subagent",
      { input: { ...subagent, enabled: !subagent.enabled } },
      `${subagent.name} 已${subagent.enabled ? "停用" : "启用"}。`,
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
        <span className="brand-mark">GF</span>
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
    ? providers.filter((provider) => {
        if (!provider.enabled) return false;
        if (selectedAdditionalClient.protocol === "gemini") {
          return (
            provider.protocol === "gemini_native" &&
            provider.endpointMode === "base_url" &&
            provider.apiKeyPlacement === "x_api_key"
          );
        }
        if (selectedAdditionalClient.protocol === "responses") {
          return (
            provider.protocol === "open_ai_responses" &&
            provider.endpointMode === "base_url" &&
            provider.apiKeyPlacement === "bearer"
          );
        }
        return provider.protocol !== "gemini_native";
      })
    : [];

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">GF</span>
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
        <div className="sidebar-foot">
          <span
            className={`status-dot ${ready ? "status-dot--good" : "status-dot--error"}`}
          />
          <div>
            <strong>{ready ? "GrillForge 已就绪" : "服务不可用"}</strong>
            <small>
              {ready
                ? `Claude Code ${takeoverLabel(integration.takeover)}`
                : "请重新启动应用"}
            </small>
          </div>
        </div>
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
                        setSelectedClient("claude_code");
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
                      <div className="dashboard-client-list">
                        <button
                          onClick={() => {
                            selectView("clients");
                            setSelectedClient("claude_code");
                          }}
                        >
                          <span className="client-mark">CC</span>
                          <div>
                            <strong>Claude Code</strong>
                            <small>
                              {Object.keys(state.modelSlots).length} 个槽位 ·{" "}
                              {effectiveSubAgents.length} 个 SubAgent
                            </small>
                          </div>
                          <Badge tone={integrationTone}>
                            {takeoverLabel(integration.takeover)}
                          </Badge>
                        </button>
                        <button
                          onClick={() => {
                            selectView("clients");
                            setSelectedClient("claude_desktop");
                          }}
                        >
                          <span className="client-mark">CD</span>
                          <div>
                            <strong>Claude Client</strong>
                            <small>
                              {
                                Object.keys(state.claudeDesktopModelSlots)
                                  .length
                              }{" "}
                              个对话角色
                            </small>
                          </div>
                          <Badge tone={desktopTone}>
                            {takeoverLabel(claudeDesktop.takeover)}
                          </Badge>
                        </button>
                        <button
                          onClick={() => {
                            selectView("clients");
                            setSelectedClient("pi");
                          }}
                        >
                          <span className="client-mark">PI</span>
                          <div>
                            <strong>Pi</strong>
                            <small>
                              {state.piEnabledModelIds.length} 个可用模型
                            </small>
                          </div>
                          <Badge tone={piTone}>
                            {takeoverLabel(piStatus.takeover)}
                          </Badge>
                        </button>
                      </div>
                    </div>
                    <div className="agent-orb">
                      <span>⌁</span>
                      <i>••</i>
                    </div>
                  </article>
                  <article className="quick-action-card">
                    <p className="kicker">快速操作</p>
                    <button onClick={() => selectView("clients")}>
                      <span>◇</span>
                      <div>
                        <strong>配置客户端</strong>
                        <small>选择模型和模型池</small>
                      </div>
                      <i>›</i>
                    </button>
                    <button onClick={openNewProvider}>
                      <span>◎</span>
                      <div>
                        <strong>添加供应商</strong>
                        <small>从供应商预设开始</small>
                      </div>
                      <i>›</i>
                    </button>
                    <button onClick={() => selectView("providers")}>
                      <span>◉</span>
                      <div>
                        <strong>同步供应商模型</strong>
                        <small>自动同步或手动添加</small>
                      </div>
                      <i>›</i>
                    </button>
                  </article>
                </section>
                <section className="metric-grid metric-grid--four">
                  <article className="metric-card">
                    <p>支持客户端</p>
                    <strong>{4 + additionalClients.length}</strong>
                    <span>已支持</span>
                  </article>
                  <article className="metric-card">
                    <p>有效 SubAgent</p>
                    <strong>{effectiveSubAgents.length}</strong>
                    <span>{state.subagents.length} 个已创建</span>
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
                      <span className="client-mark">CC</span>
                      <div>
                        <strong>Claude Code CLI</strong>
                        <small>
                          {Object.keys(state.modelSlots).length} 个槽位 ·{" "}
                          {effectiveSubAgents.length} 个 SubAgent
                        </small>
                      </div>
                      <Badge tone={integrationTone}>
                        {takeoverLabel(integration.takeover)}
                      </Badge>
                    </div>
                    <div className="recent-client">
                      <span className="client-mark">CD</span>
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
                    onClick={() => setSelectedClient("claude_code")}
                  >
                    <span className="client-mark">CC</span>
                    <div>
                      <strong>Claude Code</strong>
                      <small>
                        {claudeCli.installed
                          ? (claudeCli.version ?? "已安装")
                          : "未检测到 Claude CLI"}
                      </small>
                    </div>
                    <Badge tone={claudeCli.installed ? "good" : "warn"}>
                      {claudeCli.installed ? "可用" : "未安装"}
                    </Badge>
                  </button>
                  <button
                    className={`client-card client-card--available ${selectedClient === "claude_desktop" ? "client-card--selected" : ""}`}
                    type="button"
                    onClick={() => setSelectedClient("claude_desktop")}
                  >
                    <span className="client-mark">CD</span>
                    <div>
                      <strong>Claude Client</strong>
                      <small>
                        {claudeDesktop.installed ? "已检测到" : "未检测到安装"}
                      </small>
                    </div>
                    <Badge tone={claudeDesktop.installed ? "good" : "warn"}>
                      {claudeDesktop.installed ? "可用" : "未安装"}
                    </Badge>
                  </button>
                  <button
                    className={`client-card client-card--available ${selectedClient === "codex" ? "client-card--selected" : ""}`}
                    type="button"
                    onClick={() => setSelectedClient("codex")}
                  >
                    <span className="client-mark">CX</span>
                    <div>
                      <strong>Codex</strong>
                      <small>
                        {codexStatus.installed
                          ? `${codexStatus.executablePath?.includes("/ChatGPT.app/") ? "ChatGPT 内置" : "独立 CLI"} · ${codexStatus.version ?? "已安装"}`
                          : "未检测到 Codex CLI"}
                      </small>
                    </div>
                    <Badge tone={codexStatus.installed ? "good" : "warn"}>
                      {codexStatus.installed ? "可用" : "未安装"}
                    </Badge>
                  </button>
                  <button
                    className={`client-card client-card--available ${selectedClient === "pi" ? "client-card--selected" : ""}`}
                    type="button"
                    onClick={() => setSelectedClient("pi")}
                  >
                    <span className="client-mark">PI</span>
                    <div>
                      <strong>Pi</strong>
                      <small>
                        {piStatus.installed
                          ? `${piStatus.version ?? "已安装"}`
                          : "未检测到 Pi CLI"}
                      </small>
                    </div>
                    <Badge tone={piStatus.installed ? "good" : "warn"}>
                      {piStatus.installed ? "可用" : "未安装"}
                    </Badge>
                  </button>
                  {additionalClients.map((client) => {
                    const status = clientStatuses[client.id];
                    return (
                      <button
                        className={`client-card client-card--available ${selectedClient === client.id ? "client-card--selected" : ""}`}
                        type="button"
                        key={client.id}
                        onClick={() => setSelectedClient(client.id)}
                      >
                        <span className="client-mark">{client.mark}</span>
                        <div>
                          <strong>{client.name}</strong>
                          <small>
                            {status.installed
                              ? (status.version ?? "已安装")
                              : "未检测到客户端"}
                          </small>
                        </div>
                        <Badge tone={status.installed ? "good" : "warn"}>
                          {status.installed ? "可用" : "未安装"}
                        </Badge>
                      </button>
                    );
                  })}
                </section>
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
                        <small>SubAgent</small>
                        <strong>{effectiveSubAgentCount} 个可用</strong>
                        <span>{state.subagents.length} 个自定义</span>
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
                      <div className="agent-monogram">C</div>
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
                              "Claude Code 配置已应用。Claude Client 中已打开的 Code 会话需要重新启动后使用新路由。",
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
                              "Claude Code 已停用。",
                            )
                          }
                        >
                          停用
                        </button>
                      </div>
                    </section>
                    {clientTab === "subagents" && (
                      <section className="native-subagent-card">
                        <span className="subagent-icon">C</span>
                        <div className="subagent-main">
                          <div>
                            <h3>Claude 原生模型候选</h3>
                            <Badge tone="good">内置</Badge>
                          </div>
                          <p>
                            加入候选池时跟随 Claude 默认模型。关闭不会删除
                            Claude 内置 Agent；只启用一个外部 SubAgent
                            时，自动委派默认使用该外部模型。
                          </p>
                        </div>
                        <Toggle
                          checked={state.nativeSubagentEnabled}
                          disabled={Boolean(pending)}
                          label="切换 Claude 原生模型候选"
                          onChange={() =>
                            commit(
                              "set_native_subagent_enabled",
                              { enabled: !state.nativeSubagentEnabled },
                              `Claude 原生模型已${state.nativeSubagentEnabled ? "从候选池移除" : "加入候选池"}。`,
                            )
                          }
                        />
                      </section>
                    )}
                    {clientTab === "slots" && (
                      <div className="client-config-section">
                        <div className="config-heading">
                          <div>
                            <p className="kicker">模型槽位</p>
                            <h2>Claude Code 模型槽位</h2>
                            <p>先选择供应商，再选择该供应商下的模型。</p>
                          </div>
                        </div>
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
                                disabled={
                                  Boolean(pending) ||
                                  !(
                                    claudeMainProviderSelection ||
                                    modelProviderId(
                                      state.mainModelId ?? undefined,
                                    )
                                  )
                                }
                                value={
                                  modelProviderId(
                                    state.mainModelId ?? undefined,
                                  ) ===
                                  (claudeMainProviderSelection ||
                                    modelProviderId(
                                      state.mainModelId ?? undefined,
                                    ))
                                    ? (state.mainModelId ?? "")
                                    : ""
                                }
                                onChange={(event) =>
                                  commit(
                                    "set_main_model",
                                    { id: event.target.value || null },
                                    "Claude Code 默认模型已保存。",
                                  )
                                }
                              >
                                <option value="">选择模型</option>
                                {modelsForProvider(
                                  claudeMainProviderSelection ||
                                    modelProviderId(
                                      state.mainModelId ?? undefined,
                                    ),
                                ).map((model) => (
                                  <option key={model.id} value={model.id}>
                                    {model.name} · {model.upstreamId}
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
                                    disabled={
                                      Boolean(pending) || !selectedProviderId
                                    }
                                    value={
                                      modelProviderId(selectedModelId) ===
                                      selectedProviderId
                                        ? selectedModelId
                                        : ""
                                    }
                                    onChange={(event) =>
                                      commit(
                                        "set_model_slot",
                                        {
                                          slot,
                                          id: event.target.value || null,
                                        },
                                        `${modelSlotLabels[slot] ?? slot}已保存。`,
                                      )
                                    }
                                  >
                                    <option value="">选择模型</option>
                                    {modelsForProvider(selectedProviderId).map(
                                      (model) => (
                                        <option key={model.id} value={model.id}>
                                          {model.name} · {model.upstreamId}
                                        </option>
                                      ),
                                    )}
                                  </select>
                                </label>
                              </div>
                            );
                          })}
                        </section>
                      </div>
                    )}
                    {clientTab === "subagents" && (
                      <section className="subagent-section">
                        <div className="config-heading">
                          <div>
                            <p className="kicker">SubAgent</p>
                            <h2>独立智能体配置</h2>
                            <p>
                              每个 SubAgent
                              独立选择供应商、模型与能力标签，可以按需无限添加。
                            </p>
                          </div>
                          <button
                            className="button"
                            disabled={
                              Boolean(pending) || gatewayModels.length === 0
                            }
                            onClick={
                              showSubAgentForm
                                ? closeSubAgentForm
                                : openNewSubAgent
                            }
                          >
                            {showSubAgentForm ? "取消" : "+ 添加 SubAgent"}
                          </button>
                        </div>
                        {showSubAgentForm && (
                          <form
                            className="subagent-form"
                            onSubmit={saveSubAgent}
                          >
                            <label>
                              SubAgent 标识
                              <input
                                required
                                readOnly={Boolean(editingSubAgentId)}
                                value={subAgentDraft.id}
                                onChange={(event) =>
                                  setSubAgentDraft((current) => ({
                                    ...current,
                                    id: event.target.value,
                                  }))
                                }
                                placeholder="code-reviewer"
                              />
                            </label>
                            <label>
                              显示名称
                              <input
                                required
                                value={subAgentDraft.name}
                                onChange={(event) =>
                                  setSubAgentDraft((current) => ({
                                    ...current,
                                    name: event.target.value,
                                  }))
                                }
                                placeholder="代码审查"
                              />
                            </label>
                            <label>
                              供应商
                              <select
                                required
                                value={subAgentDraft.providerId}
                                onChange={(event) =>
                                  setSubAgentDraft((current) => ({
                                    ...current,
                                    providerId: event.target.value,
                                    modelId: "",
                                  }))
                                }
                              >
                                <option value="" disabled>
                                  选择供应商
                                </option>
                                {gatewayProviders.map((provider) => (
                                  <option value={provider.id} key={provider.id}>
                                    {provider.name}
                                  </option>
                                ))}
                              </select>
                            </label>
                            <label>
                              绑定模型
                              <select
                                required
                                disabled={!subAgentDraft.providerId}
                                value={subAgentDraft.modelId}
                                onChange={(event) =>
                                  setSubAgentDraft((current) => ({
                                    ...current,
                                    modelId: event.target.value,
                                  }))
                                }
                              >
                                <option value="" disabled>
                                  选择模型
                                </option>
                                {modelsForProvider(
                                  subAgentDraft.providerId,
                                ).map((model) => (
                                  <option value={model.id} key={model.id}>
                                    {model.name} · {model.upstreamId}
                                  </option>
                                ))}
                              </select>
                            </label>
                            <label>
                              能力标签
                              <input
                                required
                                value={subAgentDraft.capabilities}
                                onChange={(event) =>
                                  setSubAgentDraft((current) => ({
                                    ...current,
                                    capabilities: event.target.value,
                                  }))
                                }
                                placeholder="coding, review, testing"
                              />
                            </label>
                            <div className="subagent-form-actions">
                              <span>使用英文逗号分隔多个能力标签。</span>
                              <button
                                className="button"
                                type="submit"
                                disabled={Boolean(pending)}
                              >
                                {editingSubAgentId
                                  ? "保存修改"
                                  : "创建 SubAgent"}
                              </button>
                            </div>
                          </form>
                        )}
                        <div className="subagent-list">
                          {state.subagents.length === 0 ? (
                            <div className="subagent-empty">
                              <span>◇</span>
                              <strong>还没有 SubAgent</strong>
                              <p>添加后即可在 Claude Code 中使用。</p>
                              <button
                                className="text-link"
                                disabled={gatewayModels.length === 0}
                                onClick={openNewSubAgent}
                              >
                                添加第一个 SubAgent →
                              </button>
                            </div>
                          ) : (
                            state.subagents.map((subagent) => {
                              const model = models.find(
                                (item) => item.id === subagent.modelId,
                              );
                              const provider = providers.find(
                                (item) => item.id === model?.providerId,
                              );
                              return (
                                <article
                                  className={`subagent-card ${subagent.enabled ? "" : "subagent-card--disabled"}`}
                                  key={subagent.id}
                                >
                                  <span className="subagent-icon">
                                    {subagent.name.slice(0, 1)}
                                  </span>
                                  <div className="subagent-main">
                                    <div>
                                      <h3>{subagent.name}</h3>
                                      <code>{subagent.id}</code>
                                      {subagent.capabilities.map(
                                        (capability) => (
                                          <Badge key={capability}>
                                            {capability}
                                          </Badge>
                                        ),
                                      )}
                                    </div>
                                    <p>
                                      {model?.name ?? "模型缺失"} ·{" "}
                                      {provider?.name ?? "供应商缺失"}
                                    </p>
                                  </div>
                                  <Toggle
                                    checked={subagent.enabled}
                                    disabled={Boolean(pending)}
                                    label={`切换 ${subagent.name}`}
                                    onChange={() => toggleSubAgent(subagent)}
                                  />
                                  <div className="row-actions">
                                    <button
                                      className="action-button"
                                      onClick={() => editSubAgent(subagent)}
                                    >
                                      编辑
                                    </button>
                                    <button
                                      className="action-button action-button--danger"
                                      onClick={() =>
                                        commit(
                                          "delete_subagent",
                                          { id: subagent.id },
                                          `${subagent.name} 已删除。`,
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
                  <section className="client-detail">
                    <div className="client-detail-head">
                      <div>
                        <p className="kicker">Claude Client</p>
                        <h2>模型配置</h2>
                        <p>
                          配置对话与 Cowork 模型；Code 使用 Claude Code 的模型与
                          SubAgent 配置。
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
                        <small>Code / 后台任务</small>
                        <strong>{takeoverLabel(integration.takeover)}</strong>
                        <span>
                          可用 {effectiveSubAgentCount} 个 Claude Code
                          SubAgent
                        </span>
                      </article>
                    </section>
                    <section className="reuse-card">
                      <div>
                        <p className="kicker">Code SubAgent</p>
                        <h2>复用 Claude Code 配置</h2>
                        <p>
                          在 Claude Code 页面应用一次即可，无需为 Client Code
                          重复配置。
                        </p>
                      </div>
                      <button
                        className="button button--secondary"
                        onClick={() => setSelectedClient("claude_code")}
                      >
                        打开 Claude Code 配置
                      </button>
                    </section>
                    <section className="client-config-section desktop-role-section">
                      <div className="config-heading">
                        <div>
                          <p className="kicker">Client 对话 / Cowork</p>
                          <h2>对话角色模型</h2>
                          <p>先选择供应商，再选择该供应商下的模型。</p>
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
                                        `Claude Client ${modelSlotLabels[slot] ?? slot}已清除。`,
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
                                  disabled={
                                    Boolean(pending) || !selectedProviderId
                                  }
                                  value={
                                    modelProviderId(selectedModelId) ===
                                    selectedProviderId
                                      ? selectedModelId
                                      : ""
                                  }
                                  onChange={(event) =>
                                    commit(
                                      "set_claude_desktop_model_slot",
                                      { slot, id: event.target.value || null },
                                      `Claude Client ${modelSlotLabels[slot] ?? slot}已保存。`,
                                    )
                                  }
                                >
                                  <option value="">选择模型</option>
                                  {modelsForProvider(selectedProviderId).map(
                                    (model) => (
                                      <option key={model.id} value={model.id}>
                                        {model.name} · {model.upstreamId}
                                      </option>
                                    ),
                                  )}
                                </select>
                              </label>
                            </div>
                          );
                        })}
                      </section>
                    </section>
                    <section className="agent-card">
                      <div className="agent-monogram">D</div>
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
                            : "保存模型后应用配置，并重新启动 Claude Client。"}
                        </p>
                      </div>
                      <div className="agent-actions">
                        <button
                          className="button"
                          disabled={
                            Boolean(pending) || !claudeDesktop.installed
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
                              "Claude Client 已停用；重新启动 Client 后恢复官方模式。",
                            )
                          }
                        >
                          停用
                        </button>
                      </div>
                    </section>
                  </section>
                )}
                {selectedClient === "pi" && (
                  <section className="client-detail">
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
                    </section>
                    <section className="client-config-section">
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
                    <section className="agent-card">
                      <div className="agent-monogram">π</div>
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
                            runPiIntegration("disable_pi", "Pi 已停用。")
                          }
                        >
                          停用
                        </button>
                      </div>
                    </section>
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
                      <section className="client-detail">
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
                        </section>
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
                                    <span className="subagent-icon">
                                      {agent.name.slice(0, 1).toUpperCase()}
                                    </span>
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
                        <section className="agent-card">
                          <div className="agent-monogram">CX</div>
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
                                  "Codex 已停用。",
                                )
                              }
                            >
                              停用
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
                    const configuredSecondaryProviderId = modelProviderId(
                      selectedClientConfiguration.secondaryModelId ?? undefined,
                    );
                    const secondaryProviderId =
                      clientSecondaryProviderSelections[
                        selectedAdditionalClient.id
                      ] ?? configuredSecondaryProviderId;
                    return (
                      <section className="client-detail">
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
                        </section>
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
                              <span>主模型</span>
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
                            {selectedAdditionalClient.id === "kimi_code" && (
                              <div className="slot-card slot-card--cascade">
                                <span>Secondary 模型</span>
                                <label>
                                  <small>供应商</small>
                                  <select
                                    disabled={Boolean(pending)}
                                    value={secondaryProviderId}
                                    onChange={(event) => {
                                      const next = event.target.value;
                                      setClientSecondaryProviderSelections(
                                        (current) => ({
                                          ...current,
                                          [selectedAdditionalClient.id]: next,
                                        }),
                                      );
                                      if (!next)
                                        void commit(
                                          "set_client_secondary_model",
                                          {
                                            clientId:
                                              selectedAdditionalClient.id,
                                            id: null,
                                          },
                                          "Kimi Code Secondary 模型已清除。",
                                        );
                                    }}
                                  >
                                    <option value="">跟随 Primary</option>
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
                                    disabled={
                                      Boolean(pending) || !secondaryProviderId
                                    }
                                    value={
                                      modelProviderId(
                                        selectedClientConfiguration.secondaryModelId ??
                                          undefined,
                                      ) === secondaryProviderId
                                        ? (selectedClientConfiguration.secondaryModelId ??
                                          "")
                                        : ""
                                    }
                                    onChange={(event) =>
                                      commit(
                                        "set_client_secondary_model",
                                        {
                                          clientId: selectedAdditionalClient.id,
                                          id: event.target.value || null,
                                        },
                                        "Kimi Code Secondary 模型已保存。",
                                      )
                                    }
                                  >
                                    <option value="">选择模型</option>
                                    {modelsForProvider(secondaryProviderId).map(
                                      (model) => (
                                        <option key={model.id} value={model.id}>
                                          {model.name} · {model.upstreamId}
                                        </option>
                                      ),
                                    )}
                                  </select>
                                </label>
                              </div>
                            )}
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
                                          model.id ||
                                        selectedClientConfiguration.secondaryModelId ===
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
                                <p className="kicker">Agents</p>
                                <h2>Kimi Code Agents</h2>
                                <p>从 Kimi Code 的全局 Agent 目录同步。</p>
                              </div>
                            </div>
                            <div className="subagent-list">
                              {(selectedAdditionalStatus.agents ?? []).map(
                                (agent) => (
                                  <article
                                    className="subagent-card"
                                    key={`${agent.source ?? "built-in"}:${agent.name}`}
                                  >
                                    <span className="subagent-icon">
                                      {agent.name.slice(0, 1).toUpperCase()}
                                    </span>
                                    <div className="subagent-main">
                                      <div>
                                        <h3>{agent.name}</h3>
                                        <Badge
                                          tone={
                                            agent.builtIn ? "good" : "neutral"
                                          }
                                        >
                                          {agent.builtIn ? "内置" : "自定义"}
                                        </Badge>
                                        <Badge>
                                          {agent.modelPreference === "secondary"
                                            ? "Secondary"
                                            : "Primary"}
                                        </Badge>
                                      </div>
                                      <p>{agent.description || "未填写说明"}</p>
                                    </div>
                                    {!agent.builtIn && (
                                      <label className="agent-model-preference">
                                        <small>使用模型</small>
                                        <select
                                          value={
                                            agent.modelPreference ?? "primary"
                                          }
                                          disabled={Boolean(pending)}
                                          onChange={(event) =>
                                            void setKimiAgentPreference(
                                              agent.name,
                                              event.target.value as
                                                | "primary"
                                                | "secondary",
                                            )
                                          }
                                        >
                                          <option value="primary">
                                            Primary
                                          </option>
                                          <option value="secondary">
                                            Secondary
                                          </option>
                                        </select>
                                      </label>
                                    )}
                                  </article>
                                ),
                              )}
                            </div>
                          </section>
                        )}
                        <section className="agent-card">
                          <div className="agent-monogram">
                            {selectedAdditionalClient.mark}
                          </div>
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
                                  `${selectedAdditionalClient.name} 已停用。`,
                                )
                              }
                            >
                              停用
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
                        {editingProviderId ? "更新供应商" : "保存供应商"}
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
                              void discoverModels(provider);
                            }}
                          >
                            {pending === `discover:${provider.id}`
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
                    <span className="client-mark">CC</span>
                    <div>
                      <strong>Claude Code</strong>
                      <small>
                        {Object.keys(state.modelSlots).length} 个槽位 ·{" "}
                        {effectiveSubAgentCount} 个 SubAgent
                      </small>
                    </div>
                    <Badge tone={integrationTone}>
                      {takeoverLabel(integration.takeover)}
                    </Badge>
                  </article>
                  <article>
                    <span className="client-mark">CD</span>
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
                    <span className="client-mark">PI</span>
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
                    <span className="client-mark">CX</span>
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
                        <span className="client-mark">{client.mark}</span>
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
                      <span className="client-mark">CC</span>
                      <div>
                        <strong>Claude Code CLI</strong>
                        <small>{takeoverLabel(integration.takeover)}</small>
                      </div>
                    </article>
                    <article className="relation-node relation-node--accent">
                      <span className="client-mark">CD</span>
                      <div>
                        <strong>Claude Client</strong>
                        <small>{takeoverLabel(claudeDesktop.takeover)}</small>
                      </div>
                    </article>
                    <article className="relation-node relation-node--accent">
                      <span className="client-mark">PI</span>
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
                    {state.subagents.map((subagent) => (
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
                    {state.subagents.map((subagent) => {
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
                            {subagent.enabled ? "已启用" : "已停用"} ·{" "}
                            {provider?.name ?? "供应商缺失"}
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
                <p>自动同步模型列表，或手动添加模型。</p>
              </div>
              <button
                className="modal-close"
                aria-label="关闭供应商模型"
                onClick={closeProviderModels}
              >
                ×
              </button>
            </header>
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
                  onClick={() => discoverModels(managingProvider)}
                >
                  {pending === `discover:${managingProvider.id}`
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
            {discoveredModels.length > 0 && (
              <section className="discovery-panel">
                <div className="panel-head">
                  <div>
                    <strong>同步候选</strong>
                    <small>
                      已发现 {discoveredModels.length} 个，确认后即可导入。
                    </small>
                  </div>
                  <button
                    className="button"
                    disabled={
                      Boolean(pending) || selectedDiscovered.length === 0
                    }
                    onClick={() => importDiscovered(managingProvider)}
                  >
                    导入已选 {selectedDiscovered.length} 个
                  </button>
                </div>
                <div className="discovery-list">
                  {discoveredModels.map((model) => {
                    const imported = managingProviderModels.some(
                      (existing) => existing.upstreamId === model.id,
                    );
                    return (
                      <label
                        className={
                          imported
                            ? "discovery-item discovery-item--imported"
                            : "discovery-item"
                        }
                        key={model.id}
                      >
                        <input
                          type="checkbox"
                          disabled={imported}
                          checked={
                            imported || selectedDiscovered.includes(model.id)
                          }
                          onChange={() => toggleDiscovered(model.id)}
                        />
                        <span>
                          <strong>{model.id}</strong>
                          <small>
                            {imported
                              ? "已导入"
                              : (model.ownedBy ?? "未声明所有者")}
                          </small>
                        </span>
                      </label>
                    );
                  })}
                </div>
              </section>
            )}
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
                    const referenceCount = state.subagents.filter(
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
                          <span className="model-avatar">
                            {model.name.slice(0, 1)}
                          </span>
                          <span>
                            <strong>{model.name}</strong>
                            <code>{model.upstreamId}</code>
                          </span>
                        </button>
                        <div className="tag-cloud">
                          {model.capabilities.slice(0, 3).map((capability) => (
                            <Badge key={capability}>{capability}</Badge>
                          ))}
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
                  {selectedModel.name.slice(0, 1)}
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
                  {(state?.subagents ?? []).filter(
                    (subagent) => subagent.modelId === selectedModel.id,
                  ).length > 0 && (
                    <Badge tone="good">
                      {
                        (state?.subagents ?? []).filter(
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
    </div>
  );
}

export default App;

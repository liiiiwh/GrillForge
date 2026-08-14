import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import "./QuickPanel.css";
import grillforgeLogo from "./assets/grillforge-logo.png";

type Provider = { id: string; name: string; enabled: boolean };
type Model = { id: string; name: string; providerId: string };
type Extension = { id: string; name: string };

type State = {
  providers: Provider[];
  models: Model[];
  mainModelId: string | null;
  modelSlots: Record<string, string>;
  claudeNativeModelSlots: Record<string, string>;
  claudeDesktopModelSlots: Record<string, string>;
  piMainModelId: string | null;
  codexMainModelId: string | null;
  codexNativeModelSlots: Record<string, string>;
  codexAgentModelIds: Record<string, string>;
  clientConfigurations: Record<
    string,
    { mainModelId: string | null; enabledModelIds: string[] }
  >;
  extensionSubagents: Extension[];
  clientExtensionSubagentIds: Record<string, string[]>;
};

type NativeModel = { id: string; name: string };
type InstalledStatus = {
  installed: boolean;
  nativeModels?: NativeModel[];
  nativeCurrentModel?: string | null;
  nativeModelSlots?: Record<string, string>;
  customAgents?: Array<{ name: string; description: string }>;
};
type MountStatus = { clientId: string; mounted: boolean };

export type QuickProvider = {
  id: string;
  name: string;
  models: Array<{ id: string; name: string }>;
};
export type QuickSlot = {
  id: string;
  label: string;
  providerId: string;
  modelId: string;
  providers: QuickProvider[];
};
export type QuickClient = {
  id: string;
  name: string;
  slots: QuickSlot[];
  extensionMounted: boolean;
  extensions: Array<{ id: string; name: string; enabled: boolean }>;
};

export function QuickPanelContent({
  clients,
  busy,
  error,
  onOpenMain,
  onSelectModel,
  onApplyClient,
  onSetExtensionMounted,
  onSetExtensionEnabled,
}: {
  clients: QuickClient[];
  busy: string;
  error: string;
  onOpenMain: () => void;
  onSelectModel: (
    clientId: string,
    slotId: string,
    providerId: string,
    modelId: string,
  ) => void;
  onApplyClient?: (clientId: string) => void;
  onSetExtensionMounted: (clientId: string, mounted: boolean) => void;
  onSetExtensionEnabled: (
    clientId: string,
    extensionId: string,
    enabled: boolean,
  ) => void;
}) {
  return (
    <main className="quick-panel">
      <header>
        <div>
          <img src={grillforgeLogo} alt="" />
          <div>
            <strong>GrillForge</strong>
            <small>客户端快捷设置</small>
          </div>
        </div>
        <button onClick={onOpenMain}>打开主界面</button>
      </header>
      {error && <p className="quick-error" role="alert">{error}</p>}
      <section className="quick-clients">
        {clients.length === 0 && <p className="quick-empty">未检测到已安装客户端</p>}
        {clients.map((client) => (
          <details key={client.id} open>
            <summary>
              <strong>{client.name}</strong>
              {onApplyClient && client.slots.length > 0 && (
                <button
                  disabled={Boolean(busy)}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onApplyClient(client.id);
                  }}
                >
                  应用
                </button>
              )}
            </summary>
            <div className="quick-section">
              <h2>模型配置</h2>
              {client.slots.length === 0 && <p>没有可配置槽位</p>}
              {client.slots.map((slot) => {
                const provider =
                  slot.providers.find((item) => item.id === slot.providerId) ??
                  slot.providers[0];
                return (
                  <div className="quick-slot" key={slot.id}>
                    <span>{slot.label}</span>
                    <select
                      aria-label={`${slot.label}供应商`}
                      disabled={Boolean(busy)}
                      value={slot.providerId}
                      onChange={(event) => {
                        const nextProvider = slot.providers.find(
                          (item) => item.id === event.target.value,
                        );
                        onSelectModel(
                          client.id,
                          slot.id,
                          event.target.value,
                          nextProvider?.models[0]?.id ?? "",
                        );
                      }}
                    >
                      {slot.providers.map((item) => (
                        <option key={item.id} value={item.id}>{item.name}</option>
                      ))}
                    </select>
                    <select
                      aria-label={`${slot.label}模型`}
                      disabled={Boolean(busy) || !provider}
                      value={slot.modelId}
                      onChange={(event) =>
                        onSelectModel(
                          client.id,
                          slot.id,
                          slot.providerId,
                          event.target.value,
                        )
                      }
                    >
                      {(provider?.models ?? []).map((model) => (
                        <option key={model.id} value={model.id}>{model.name}</option>
                      ))}
                    </select>
                  </div>
                );
              })}
            </div>
            <div className="quick-section">
              <div className="quick-extension-head">
                <h2>扩展 Agent</h2>
                <label className="quick-switch">
                  <span>启用扩展</span>
                  <input
                    aria-label="启用扩展"
                    type="checkbox"
                    checked={client.extensionMounted}
                    disabled={Boolean(busy)}
                    onChange={(event) =>
                      onSetExtensionMounted(client.id, event.target.checked)
                    }
                  />
                </label>
              </div>
              {client.extensions.length === 0 && <p>没有扩展 Agent</p>}
              {client.extensions.map((extension) => (
                <label className="quick-extension" key={extension.id}>
                  <span>{extension.name}</span>
                  <input
                    aria-label={extension.name}
                    type="checkbox"
                    checked={extension.enabled}
                    disabled={Boolean(busy)}
                    onChange={(event) =>
                      onSetExtensionEnabled(
                        client.id,
                        extension.id,
                        event.target.checked,
                      )
                    }
                  />
                </label>
              ))}
            </div>
          </details>
        ))}
      </section>
    </main>
  );
}

const clientDefinitions = [
  ["claude_code", "Claude Code", "detect_claude_code"],
  ["claude_desktop", "Claude Client", "claude_desktop_status"],
  ["codex", "Codex", "codex_status"],
  ["pi", "Pi", "pi_status"],
  ["gemini", "Gemini CLI", "gemini_status"],
  ["grok_build", "Grok Build", "grok_build_status"],
  ["opencode", "OpenCode", "opencode_status"],
  ["hermes", "Hermes", "hermes_status"],
  ["kimi_code", "Kimi Code", "kimi_code_status"],
] as const;

function managedProviders(state: State): QuickProvider[] {
  return state.providers
    .filter((provider) => provider.enabled)
    .map((provider) => ({
      id: provider.id,
      name: provider.name,
      models: state.models
        .filter((model) => model.providerId === provider.id)
        .map((model) => ({ id: model.id, name: model.name })),
    }))
    .filter((provider) => provider.models.length > 0);
}

function slot(
  id: string,
  label: string,
  managedModelId: string | null | undefined,
  nativeModelId: string | null | undefined,
  nativeModels: NativeModel[],
  providers: QuickProvider[],
): QuickSlot {
  const providerId =
    providers.find((provider) =>
      provider.models.some((model) => model.id === managedModelId),
    )?.id ?? "native";
  const fallbackNative = nativeModelId || nativeModels[0]?.id || "default";
  const currentNative = nativeModels.find((model) => model.id === fallbackNative) ?? {
    id: fallbackNative,
    name: fallbackNative === "default" ? "客户端默认" : fallbackNative,
  };
  const nativeProvider = {
    id: "native",
    name: "跟随原生",
    models: [currentNative, ...nativeModels.filter((model) => model.id !== fallbackNative)],
  };
  return {
    id,
    label,
    providerId,
    modelId: providerId === "native" ? fallbackNative : managedModelId || "",
    providers: [nativeProvider, ...providers],
  };
}

function buildClients(
  state: State,
  statuses: Record<string, InstalledStatus>,
  mounts: MountStatus[],
): QuickClient[] {
  const providers = managedProviders(state);
  const mounted = new Set(mounts.filter((item) => item.mounted).map((item) => item.clientId));
  const extensionsFor = (clientId: string) => {
    const enabled = new Set(state.clientExtensionSubagentIds[clientId] ?? []);
    return state.extensionSubagents.map((extension) => ({
      id: extension.id,
      name: extension.name,
      enabled: enabled.has(extension.id),
    }));
  };
  const clients: QuickClient[] = [];
  for (const [id, name] of clientDefinitions) {
    const status = statuses[id];
    if (!status?.installed) continue;
    const nativeModels = status.nativeModels ?? [];
    const slots: QuickSlot[] = [];
    if (id === "claude_code") {
      slots.push(
        slot("main", "默认模型", state.mainModelId, state.claudeNativeModelSlots.main, nativeModels, providers),
      );
      for (const [slotId, label] of [
        ["sonnet", "Sonnet"], ["opus", "Opus"], ["fable", "Fable"],
        ["haiku", "Haiku"], ["subagent_default", "SubAgent 默认"],
      ] as const) {
        slots.push(slot(slotId, label, state.modelSlots[slotId], state.claudeNativeModelSlots[slotId], nativeModels, providers));
      }
    } else if (id === "claude_desktop") {
      for (const [slotId, label] of [
        ["sonnet", "Sonnet"], ["opus", "Opus"], ["fable", "Fable"], ["haiku", "Haiku"],
      ] as const) {
        slots.push(slot(slotId, label, state.claudeDesktopModelSlots[slotId], status.nativeCurrentModel, nativeModels, providers));
      }
      slots.push(slot("subagent_default", "SubAgent 默认", state.modelSlots.subagent_default, state.claudeNativeModelSlots.subagent_default, nativeModels, providers));
    } else if (id === "codex") {
      slots.push(slot("main", "主模型", state.codexMainModelId, state.codexNativeModelSlots.main, nativeModels, providers));
      slots.push(slot("default_subagent", "SubAgent 默认", state.codexAgentModelIds.default_subagent, state.codexNativeModelSlots.default_subagent, nativeModels, providers));
      for (const agent of status.customAgents ?? []) {
        slots.push(slot(`agent:${agent.name}`, agent.name, state.codexAgentModelIds[agent.name], state.codexNativeModelSlots[`agent_${agent.name}`], nativeModels, providers));
      }
    } else if (id === "pi") {
      slots.push(slot("main", "主模型", state.piMainModelId, null, [], providers));
    } else {
      slots.push(slot("main", id === "kimi_code" ? "默认模型" : "主模型", state.clientConfigurations[id]?.mainModelId, null, [], providers));
    }
    clients.push({ id, name, slots, extensionMounted: mounted.has(id), extensions: extensionsFor(id) });
  }
  return clients;
}

export default function QuickPanel() {
  const [state, setState] = useState<State | null>(null);
  const [statuses, setStatuses] = useState<Record<string, InstalledStatus>>({});
  const [mounts, setMounts] = useState<MountStatus[]>([]);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");

  async function refresh() {
    const [nextState, nextMounts, statusResults, claudeIntegration] = await Promise.all([
      invoke<State>("load_state"),
      invoke<MountStatus[]>("client_mcp_statuses"),
      Promise.allSettled(
        clientDefinitions.map(async ([id, , command]) => [id, await invoke<InstalledStatus>(command)] as const),
      ),
      Promise.allSettled([invoke<InstalledStatus>("integration_status")]).then(
        ([result]) => result,
      ),
    ]);
    setState(nextState);
    setMounts(nextMounts);
    const nextStatuses = Object.fromEntries(
      statusResults.flatMap((result) =>
        result.status === "fulfilled" ? [result.value] : [],
      ),
    );
    if (claudeIntegration.status === "fulfilled") {
      nextStatuses.claude_code = {
        ...claudeIntegration.value,
        ...nextStatuses.claude_code,
        nativeModels: claudeIntegration.value.nativeModels,
        nativeCurrentModel: claudeIntegration.value.nativeCurrentModel,
        nativeModelSlots: claudeIntegration.value.nativeModelSlots,
      };
    }
    setStatuses(nextStatuses);
    const failedClients: string[] = statusResults.flatMap((result, index) =>
      result.status === "rejected" ? [clientDefinitions[index][1]] : [],
    );
    if (claudeIntegration.status === "rejected") failedClients.push("Claude Code 模型目录");
    if (failedClients.length > 0) {
      setError(`以下客户端状态暂不可用：${failedClients.join("、")}`);
    }
  }

  useEffect(() => {
    const reload = () => void refresh().catch((cause) => setError(String(cause)));
    reload();
    window.addEventListener("focus", reload);
    return () => window.removeEventListener("focus", reload);
  }, []);

  const clients = useMemo(
    () => (state ? buildClients(state, statuses, mounts) : []),
    [state, statuses, mounts],
  );

  async function perform(name: string, operation: () => Promise<void>) {
    if (busy) return;
    setBusy(name);
    setError("");
    try {
      await operation();
      await refresh();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy("");
    }
  }

  async function selectModel(clientId: string, slotId: string, providerId: string, modelId: string) {
    await perform(`model:${clientId}:${slotId}`, async () => {
      const native = providerId === "native";
      if (clientId === "claude_code") {
        await invoke(slotId === "main" ? "set_main_model" : "set_model_slot", slotId === "main" ? { id: native ? null : modelId } : { slot: slotId, id: native ? null : modelId });
        if (native) await invoke("set_claude_native_model", { slot: slotId, model: modelId });
      } else if (clientId === "claude_desktop") {
        if (slotId === "subagent_default") {
          await invoke("set_model_slot", { slot: slotId, id: native ? null : modelId });
          if (native) await invoke("set_claude_native_model", { slot: slotId, model: modelId });
        } else {
          await invoke("set_claude_desktop_model_slot", { slot: slotId, id: native ? null : modelId });
        }
      } else if (clientId === "codex") {
        if (slotId === "main") {
          await invoke(native ? "set_codex_native_main_model" : "set_codex_main_model", native ? { model: modelId === "default" ? null : modelId } : { id: modelId });
        } else if (slotId === "default_subagent") {
          await invoke(native ? "set_codex_native_default_subagent_model" : "set_codex_default_subagent_model", native ? { model: modelId === "default" ? null : modelId } : { id: modelId });
        } else {
          const name = slotId.slice("agent:".length);
          await invoke(native ? "set_codex_native_custom_agent_model" : "set_codex_custom_agent_model", native ? { name, model: modelId === "default" ? null : modelId } : { name, id: modelId });
        }
      } else if (clientId === "pi") {
        await invoke("set_pi_main_model", { id: native ? null : modelId });
      } else {
        await invoke("set_client_main_model", { clientId, id: native ? null : modelId });
      }
    });
  }

  const applyCommands: Record<string, string> = {
    claude_code: "apply_claude_code", claude_desktop: "apply_claude_desktop",
    codex: "apply_codex", pi: "apply_pi", gemini: "apply_gemini",
    grok_build: "apply_grok_build", opencode: "apply_opencode",
    hermes: "apply_hermes", kimi_code: "apply_kimi_code",
  };

  return (
    <QuickPanelContent
      clients={clients}
      busy={busy}
      error={error}
      onOpenMain={() => void invoke("show_main_window")}
      onSelectModel={(...args) => void selectModel(...args)}
      onApplyClient={(clientId) => void perform(`apply:${clientId}`, async () => { await invoke(applyCommands[clientId]); })}
      onSetExtensionMounted={(clientId, mounted) => void perform(`mount:${clientId}`, async () => { await invoke(mounted ? "mount_client_mcp" : "unmount_client_mcp", { clientId }); })}
      onSetExtensionEnabled={(clientId, extensionSubagentId, enabled) => void perform(`extension:${clientId}:${extensionSubagentId}`, async () => { await invoke("set_client_extension_binding", { clientId, extensionSubagentId, enabled }); })}
    />
  );
}

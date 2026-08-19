// @vitest-environment jsdom

import { render, waitFor, fireEvent, within, cleanup } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import App, { additionalClients } from "./App";

const emptyState = {
  providers: [],
  models: [],
  agentEnabled: false,
  mainModelId: null,
  modelSlots: {},
  claudeNativeModelSlots: {},
  claudeDesktopModelSlots: {},
  piEnabled: false,
  piMainModelId: null,
  piEnabledModelIds: [],
  codexMainModelId: null,
  codexNativeModelSlots: {},
  codexAgentModelIds: {},
  clientConfigurations: Object.fromEntries(
    additionalClients.map((client) => [
      client.id,
      { mainModelId: null, enabledModelIds: [] },
    ]),
  ),
  extensionSubagents: [],
  clientExtensionSubagentIds: {},
  mcpMountedClientIds: [],
};

describe("every view mounts", () => {
  beforeEach(() => {
    // Without this a previous test's tree stays in the document and queries pick
    // its buttons instead of this test's.
    cleanup();
    vi.mocked(invoke).mockReset();
    // Status probes are allowed to fail; the app must still render.
    vi.mocked(invoke).mockImplementation(async (command: string) => {
      if (command === "load_state") return emptyState;
      if (command === "provider_presets")
        return { schema_version: 1, presets: [] };
      throw new Error("unavailable");
    });
  });

  it("renders the routing view without breaking the render", async () => {
    const { container } = render(<App />);
    await waitFor(() =>
      expect(within(container).getAllByText("控制中心").length).toBeGreaterThan(0),
    );

    // A hook placed after the loading early-return changes the hook count once
    // loading finishes, which blanks the window instead of rendering.
    const routesNav = within(container)
      .getAllByRole("button")
      .find((button) => button.textContent?.trim().endsWith("路由策略"));
    fireEvent.click(routesNav!);
    await waitFor(() => expect(within(container).getByText("路由概览")).toBeTruthy());
  });

  it("draws each client's routes as one branching node", async () => {
    const routedState = {
      ...emptyState,
      providers: [
        {
          id: "anthropic",
          name: "Anthropic",
          protocol: "anthropic_messages",
          endpoint: "https://api.anthropic.com",
          endpointMode: "base_url",
          apiKeyPlacement: "x_api_key",
          hasApiKey: true,
          enabled: true,
          modelsUrl: null,
          protocolEndpoints: [],
        },
      ],
      models: [
        {
          id: "opus",
          name: "Claude Opus 5",
          upstreamId: "claude-opus-5",
          providerId: "anthropic",
          capabilities: [],
          protocolCapabilities: [],
          nativeProtocols: ["anthropic_messages"],
          unsupportedNativeProtocols: [],
          routeAlias: "grillforge/opus",
        },
      ],
      // Two slots on one client must fork from a single client node.
      modelSlots: { main: "opus", sonnet: "opus" },
    };
    vi.mocked(invoke).mockImplementation(async (command: string) => {
      if (command === "load_state") return routedState;
      if (command === "provider_presets") return { schema_version: 1, presets: [] };
      throw new Error("unavailable");
    });

    const { container } = render(<App />);
    await waitFor(() =>
      expect(within(container).getAllByText("控制中心").length).toBeGreaterThan(0),
    );
    const routesNav = within(container)
      .getAllByRole("button")
      .find((button) => button.textContent?.trim().endsWith("路由策略"));
    fireEvent.click(routesNav!);
    await waitFor(() =>
      expect(container.querySelectorAll(".route-branch").length).toBeGreaterThan(0),
    );
    const branches = container.querySelectorAll(".route-branch");
    expect(branches).toHaveLength(1);
    expect(branches[0].querySelector(".route-branch-client")?.textContent).toContain(
      "Claude Code",
    );
    // Both slots hang off that one node rather than repeating the client per row.
    expect(branches[0].querySelectorAll(".route-forks > li")).toHaveLength(2);
    expect(branches[0].textContent).toContain("Claude Opus 5");
  });

  it("offers extension SubAgents to any client the backend can mount", async () => {
    // The backend reports a status per MCP-capable client; the page must follow
    // that rather than a hardcoded list that drifts when support is added.
    vi.mocked(invoke).mockImplementation(async (command: string) => {
      if (command === "load_state") return emptyState;
      if (command === "provider_presets") return { schema_version: 1, presets: [] };
      if (command === "client_mcp_statuses")
        return [
          {
            clientId: "dsh",
            desiredMounted: false,
            mounted: false,
            configurationChanged: false,
          },
        ];
      throw new Error("unavailable");
    });

    const { container } = render(<App />);
    await waitFor(() =>
      expect(within(container).getAllByText("控制中心").length).toBeGreaterThan(0),
    );
    const clientsNav = within(container)
      .getAllByRole("button")
      .find((button) => button.textContent?.trim().endsWith("客户端"));
    fireEvent.click(clientsNav!);
    await waitFor(() =>
      expect(within(container).getAllByText("DeepSeek Harness").length).toBeGreaterThan(0),
    );
    fireEvent.click(within(container).getAllByText("DeepSeek Harness")[0]);
    await waitFor(() =>
      expect(
        within(container).getByText("DeepSeek Harness 可用的扩展 SubAgent"),
      ).toBeTruthy(),
    );

    // The count tile sits with the status tiles at the top, so the feature is
    // visible without scrolling past the model pool at all.
    expect(within(container).getAllByText("扩展 SubAgent").length).toBeGreaterThan(1);

    // It must precede the model pool: a list of every model pushed it off-screen.
    const headings = within(container).getAllByRole("heading").map((node) => node.textContent ?? "");
    const bindings = headings.findIndex((text) => text.includes("可用的扩展 SubAgent"));
    const pool = headings.findIndex((text) => text.includes("可用模型"));
    expect(bindings).toBeGreaterThanOrEqual(0);
    expect(pool).toBeGreaterThanOrEqual(0);
    expect(bindings).toBeLessThan(pool);
  });

  it("renders the client view without breaking the render", async () => {
    const { container } = render(<App />);
    await waitFor(() =>
      expect(within(container).getAllByText("控制中心").length).toBeGreaterThan(0),
    );
    const nav = within(container)
      .getAllByRole("button")
      .find((button) => button.textContent?.trim().endsWith("客户端"));
    fireEvent.click(nav!);
    await waitFor(() =>
      expect(within(container).getAllByText("Claude Code").length).toBeGreaterThan(0),
    );
  });
});

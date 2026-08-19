// @vitest-environment jsdom

import { render, screen, waitFor, fireEvent } from "@testing-library/react";
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
    render(<App />);
    await waitFor(() =>
      expect(screen.getAllByText("控制中心").length).toBeGreaterThan(0),
    );

    // A hook placed after the loading early-return changes the hook count once
    // loading finishes, which blanks the window instead of rendering.
    const routesNav = screen
      .getAllByRole("button")
      .find((button) => button.textContent?.trim().endsWith("路由策略"));
    fireEvent.click(routesNav!);
    await waitFor(() => expect(screen.getByText("路由概览")).toBeTruthy());
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

    render(<App />);
    await waitFor(() =>
      expect(screen.getAllByText("控制中心").length).toBeGreaterThan(0),
    );
    const clientsNav = screen
      .getAllByRole("button")
      .find((button) => button.textContent?.trim().endsWith("客户端"));
    fireEvent.click(clientsNav!);
    await waitFor(() =>
      expect(screen.getAllByText("DeepSeek Harness").length).toBeGreaterThan(0),
    );
    fireEvent.click(screen.getAllByText("DeepSeek Harness")[0]);
    await waitFor(() =>
      expect(
        screen.getByText("DeepSeek Harness 可用的扩展 SubAgent"),
      ).toBeTruthy(),
    );

    // It must precede the model pool: a list of every model pushed it off-screen.
    const headings = screen.getAllByRole("heading").map((node) => node.textContent ?? "");
    const bindings = headings.findIndex((text) => text.includes("可用的扩展 SubAgent"));
    const pool = headings.findIndex((text) => text.includes("可用模型"));
    expect(bindings).toBeGreaterThanOrEqual(0);
    expect(pool).toBeGreaterThanOrEqual(0);
    expect(bindings).toBeLessThan(pool);
  });

  it("renders the client view without breaking the render", async () => {
    render(<App />);
    await waitFor(() =>
      expect(screen.getAllByText("控制中心").length).toBeGreaterThan(0),
    );
    const nav = screen
      .getAllByRole("button")
      .find((button) => button.textContent?.trim().endsWith("客户端"));
    fireEvent.click(nav!);
    await waitFor(() =>
      expect(screen.getAllByText("Claude Code").length).toBeGreaterThan(0),
    );
  });
});

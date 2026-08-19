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

// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { QuickPanelContent } from "./QuickPanel";

afterEach(cleanup);

describe("menu bar quick panel", () => {
  it("links provider and model selects for every client slot", () => {
    const onSelect = vi.fn();
    render(
      <QuickPanelContent
        clients={[
          {
            id: "claude_code",
            name: "Claude Code",
            slots: [
              {
                id: "main",
                label: "默认模型",
                providerId: "deepseek",
                modelId: "deepseek-v4-flash",
                providers: [
                  {
                    id: "native",
                    name: "跟随原生",
                    models: [{ id: "opus-5", name: "Opus 5" }],
                  },
                  {
                    id: "deepseek",
                    name: "DeepSeek",
                    models: [
                      { id: "deepseek-v4-flash", name: "V4 Flash" },
                      { id: "deepseek-v4-pro", name: "V4 Pro" },
                    ],
                  },
                ],
              },
            ],
            extensionMounted: true,
            extensions: [],
          },
        ]}
        busy=""
        error=""
        onOpenMain={() => {}}
        onSelectModel={onSelect}
        onSetExtensionMounted={() => {}}
        onSetExtensionEnabled={() => {}}
      />,
    );

    const provider = screen.getByLabelText("默认模型供应商");
    const model = screen.getByLabelText("默认模型模型");
    expect((provider as HTMLSelectElement).value).toBe("deepseek");
    expect((model as HTMLSelectElement).value).toBe("deepseek-v4-flash");

    fireEvent.change(provider, { target: { value: "native" } });
    expect(onSelect).toHaveBeenCalledWith(
      "claude_code",
      "main",
      "native",
      "opus-5",
    );
  });

  it("shows one extension master switch and one switch per bound Agent", () => {
    render(
      <QuickPanelContent
        clients={[
          {
            id: "codex",
            name: "Codex",
            slots: [],
            extensionMounted: false,
            extensions: [
              { id: "reviewer", name: "Reviewer", enabled: true },
              { id: "coder", name: "Coder", enabled: false },
            ],
          },
        ]}
        busy=""
        error=""
        onOpenMain={() => {}}
        onSelectModel={() => {}}
        onSetExtensionMounted={() => {}}
        onSetExtensionEnabled={() => {}}
      />,
    );

    expect(screen.getByRole("checkbox", { name: "启用扩展" })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: "Reviewer" })).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: "Coder" })).toBeTruthy();
  });
});

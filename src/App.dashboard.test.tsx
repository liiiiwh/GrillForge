// @vitest-environment jsdom

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { DashboardClientList, DashboardMascot } from "./App";

describe("control center clients", () => {
  it("renders every supported client in one scrollable list", () => {
    const clients = [
      "Claude Code",
      "Claude Client",
      "Codex",
      "Pi",
      "Gemini CLI",
      "Grok Build",
      "OpenCode",
      "Hermes",
      "Kimi Code",
    ].map((name, index) => ({
      id: `client-${index}`,
      name,
      detail: `${index} 个模型`,
      tone: "neutral" as const,
      status: "未应用",
    }));

    render(<DashboardClientList clients={clients} onSelect={() => {}} />);

    expect(screen.getAllByRole("button")).toHaveLength(9);
    expect(screen.getByText("Claude Code")).toBeTruthy();
    expect(screen.getByText("Kimi Code")).toBeTruthy();
    expect(
      screen.getByTestId("dashboard-client-list").classList.contains(
        "dashboard-client-list",
      ),
    ).toBe(true);
  });

  it("uses the original robot mascot instead of the app logo", () => {
    const { container } = render(<DashboardMascot />);

    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".agent-orb")).toBeTruthy();
  });
});

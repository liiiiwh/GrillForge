// @vitest-environment jsdom

import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { ClientDetectionStatus, loadOptionalClientSnapshot } from "./App";

describe("optional client status loading", () => {
  beforeEach(() => invoke.mockReset());

  it("preserves healthy clients and records failures beside only the broken clients", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "detect_claude_code")
        return Promise.reject(new Error("Claude CLI inspection failed"));
      if (command === "opencode_status")
        return Promise.reject(new Error("OpenCode CLI did not return a version"));
      if (command === "client_mcp_statuses") return Promise.resolve([]);
      if (command === "gemini_status")
        return Promise.resolve({ installed: true, version: "2.0.0" });
      return Promise.resolve({ installed: false });
    });

    const snapshot = await loadOptionalClientSnapshot();

    expect(snapshot.claudeCli.installed).toBe(false);
    expect(snapshot.clientStatuses.gemini.version).toBe("2.0.0");
    expect(snapshot.clientStatuses.opencode.installed).toBe(false);
    expect(snapshot.errors.claude_code).toBe("Claude CLI inspection failed");
    expect(snapshot.errors.opencode).toBe(
      "OpenCode CLI did not return a version",
    );
    expect(snapshot.errors.gemini).toBeUndefined();
  });

  it("renders a detection error inside the affected client card", () => {
    render(
      <ClientDetectionStatus
        name="OpenCode"
        installed={false}
        detail="未检测到客户端"
        error="CLI 版本检查失败"
      />,
    );

    expect(screen.getByRole("alert").textContent).toContain("CLI 版本检查失败");
    expect(screen.getByText("检测失败")).toBeTruthy();
  });
});

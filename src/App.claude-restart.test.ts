import { describe, expect, it } from "vitest";

import {
  claudeClientRestartRequired,
  desktopClientRestartAfterMcpChange,
} from "./App";

describe("Claude Client restart confirmation", () => {
  it("requires the same confirmation after switching to 3P or restoring 1P", () => {
    expect(claudeClientRestartRequired("apply_claude_desktop")).toBe(true);
    expect(claudeClientRestartRequired("disable_claude_desktop")).toBe(true);
  });

  it("asks desktop clients to restart after their MCP configuration changes", () => {
    expect(desktopClientRestartAfterMcpChange("claude_desktop")).toBe(true);
    expect(desktopClientRestartAfterMcpChange("codex")).toBe(true);
    expect(desktopClientRestartAfterMcpChange("pi")).toBe(false);
  });
});

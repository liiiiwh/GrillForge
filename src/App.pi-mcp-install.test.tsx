// @vitest-environment jsdom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PiMcpInstallControl } from "./App";

afterEach(cleanup);

describe("PiMcpInstallControl", () => {
  it("requires an in-app confirmation before installing", async () => {
    const user = userEvent.setup();
    const install = vi.fn().mockResolvedValue(undefined);

    render(
      <PiMcpInstallControl
        disabled={false}
        label="一键安装 pi-mcp-extension"
        onInstall={install}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "一键安装 pi-mcp-extension" }),
    );

    expect(install).not.toHaveBeenCalled();
    expect(
      screen.getByRole("dialog", { name: "安装 Pi MCP 扩展" }),
    ).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect(install).toHaveBeenCalledTimes(1);
  });

  it("shows pending state and prevents a duplicate install", async () => {
    const user = userEvent.setup();
    let finishInstall: (() => void) | undefined;
    const install = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finishInstall = resolve;
        }),
    );

    render(
      <PiMcpInstallControl
        disabled={false}
        label="一键安装"
        onInstall={install}
      />,
    );

    await user.click(screen.getByRole("button", { name: "一键安装" }));
    await user.click(screen.getByRole("button", { name: "确认安装" }));

    const pendingButton = within(screen.getByRole("dialog")).getByRole(
      "button",
      { name: "正在安装…" },
    );
    expect((pendingButton as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(pendingButton);
    expect(install).toHaveBeenCalledTimes(1);

    finishInstall?.();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("shows an install failure beside the action and allows retry", async () => {
    const user = userEvent.setup();
    const install = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error("扩展安装命令失败"))
      .mockResolvedValueOnce(undefined);

    render(
      <PiMcpInstallControl
        disabled={false}
        label="一键安装"
        onInstall={install}
      />,
    );

    await user.click(screen.getByRole("button", { name: "一键安装" }));
    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect((await screen.findByRole("alert")).textContent).toContain(
      "扩展安装命令失败",
    );

    await user.click(screen.getByRole("button", { name: "一键安装" }));
    await user.click(screen.getByRole("button", { name: "确认安装" }));

    expect(install).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

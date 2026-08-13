// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClaudeClientCodeSubagentSlot } from "./App";

afterEach(cleanup);

describe("ClaudeClientCodeSubagentSlot", () => {
  it("uses the same provider then model picker as every other client slot", async () => {
    const user = userEvent.setup();
    const changeModel = vi.fn().mockResolvedValue(undefined);
    const changeProvider = vi.fn();

    render(
      <ClaudeClientCodeSubagentSlot
        disabled={false}
        selectedProviderId=""
        managedModelId=""
        nativeModel="haiku"
        providers={[{ id: "deepseek", name: "DeepSeek" }]}
        models={[]}
        onProviderChange={changeProvider}
        onManagedModelChange={changeModel}
        onNativeModelChange={changeModel}
      />,
    );

    expect(screen.getByRole("option", { name: "Haiku" })).toBeTruthy();
    await user.selectOptions(screen.getByLabelText("SubAgent 默认模型"), "sonnet");
    expect(changeModel).toHaveBeenCalledWith("sonnet");
    await user.selectOptions(screen.getByLabelText("SubAgent 默认供应商"), "deepseek");
    expect(changeProvider).toHaveBeenCalledWith("deepseek");
  });
});

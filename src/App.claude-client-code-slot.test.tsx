// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClaudeClientCodeSubagentSlot } from "./App";

afterEach(cleanup);

describe("ClaudeClientCodeSubagentSlot", () => {
  it("edits the shared native SubAgent model and applies the Code setting", async () => {
    const user = userEvent.setup();
    const changeModel = vi.fn().mockResolvedValue(undefined);
    const apply = vi.fn().mockResolvedValue(undefined);

    render(
      <ClaudeClientCodeSubagentSlot
        disabled={false}
        value="haiku"
        onChange={changeModel}
        onApply={apply}
      />,
    );

    expect(screen.getByRole("option", { name: "Haiku" })).toBeTruthy();
    await user.selectOptions(screen.getByLabelText("原生 SubAgent 模型"), "sonnet");
    expect(changeModel).toHaveBeenCalledWith("sonnet");

    await user.click(screen.getByRole("button", { name: "应用 Code 设置" }));
    expect(apply).toHaveBeenCalledTimes(1);
  });
});

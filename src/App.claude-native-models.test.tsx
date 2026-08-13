import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ClaudeClientCodeSubagentSlot } from "./App";

describe("Claude native model catalog", () => {
  it("shows real versioned models and saves the exact full model id", () => {
    const onNativeModelChange = vi.fn();
    render(
      <ClaudeClientCodeSubagentSlot
        disabled={false}
        selectedProviderId=""
        managedModelId=""
        nativeModel="claude-opus-4-8[1m]"
        nativeModels={[
          { id: "claude-opus-5", name: "Opus 5" },
          { id: "claude-opus-4-8[1m]", name: "Opus 4.8 · 1M" },
          { id: "claude-sonnet-5", name: "Sonnet 5" },
        ]}
        providers={[]}
        models={[]}
        onProviderChange={vi.fn()}
        onManagedModelChange={vi.fn()}
        onNativeModelChange={onNativeModelChange}
      />,
    );

    expect(screen.getByRole("option", { name: /Opus 4\.8 · 1M/ })).toBeTruthy();
    fireEvent.change(screen.getByLabelText("SubAgent 默认模型"), {
      target: { value: "claude-sonnet-5" },
    });
    expect(onNativeModelChange).toHaveBeenCalledWith("claude-sonnet-5");
  });
});
// @vitest-environment jsdom

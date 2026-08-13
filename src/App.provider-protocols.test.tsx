// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { ProviderProtocolFacts } from "./App";

afterEach(cleanup);

describe("Provider protocol facts", () => {
  const provider = {
    protocolEndpoints: [
      {
        protocol: "anthropic_messages" as const,
        endpoint: "https://provider.example/anthropic",
        endpointMode: "base_url" as const,
        apiKeyPlacement: "bearer" as const,
      },
      {
        protocol: "openai_chat" as const,
        endpoint: "https://provider.example",
        endpointMode: "base_url" as const,
        apiKeyPlacement: "bearer" as const,
      },
    ],
  };

  it("shows only protocols actually supported by at least one Provider model", () => {
    render(<ProviderProtocolFacts provider={provider} />);

    expect(screen.getByText("Anthropic Messages")).toBeTruthy();
    expect(screen.getByText("OpenAI Chat")).toBeTruthy();
    expect(screen.queryByText("OpenAI Responses")).toBeNull();
  });

  it("shows model failures only when the Provider supports that protocol", () => {
    render(
      <ProviderProtocolFacts
        provider={provider}
        model={{
          nativeProtocols: ["anthropic_messages"],
          unsupportedNativeProtocols: ["openai_responses", "openai_chat"],
        }}
      />,
    );

    expect(screen.getByText("Anthropic Messages")).toBeTruthy();
    expect(screen.getByText("不支持 OpenAI Chat")).toBeTruthy();
    expect(screen.queryByText("不支持 OpenAI Responses")).toBeNull();
  });

  it("does not claim support before synchronization", () => {
    render(<ProviderProtocolFacts provider={{ protocolEndpoints: [] }} />);

    expect(screen.getByText("尚未探测调用方式")).toBeTruthy();
  });
});

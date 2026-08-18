import assert from "node:assert/strict";
import test from "node:test";

import { wrapPiMcpExtension } from "./progress-adapter.js";

function harness() {
  const requestOptions = [];

  class FakeClient {
    async request(request, _schema, options) {
      requestOptions.push(options);
      const tag = request.params.arguments.tag;
      for (let step = 1; step <= 3; step += 1) {
        await new Promise((resolve) => setTimeout(resolve, tag === "a" ? 2 : 1));
        options.onprogress({ progress: step, total: 3, message: `${tag}-${step}` });
      }
      return { content: [{ type: "text", text: `final-${tag}` }], details: { tag } };
    }
  }

  const tools = new Map();
  const pi = {
    registerTool(tool) {
      tools.set(tool.name, tool);
    },
  };
  const upstream = async (api) => {
    const client = new FakeClient();
    api.registerTool({
      name: "mcp_grillforge_pi_run_agent",
      label: "run_agent",
      description: "fake MCP tool",
      parameters: {},
      async execute(_toolCallId, params, signal) {
        return client.request(
          { method: "tools/call", params: { name: "run_agent", arguments: params } },
          {},
          { timeout: 10_800_000, ...(signal ? { signal } : {}) },
        );
      },
    });
  };

  return { FakeClient, pi, requestOptions, tools, upstream };
}

test("maps three MCP progress notifications to Pi updates and returns only the final result", async () => {
  const { FakeClient, pi, requestOptions, tools, upstream } = harness();
  await wrapPiMcpExtension(upstream, FakeClient)(pi);

  const updates = [];
  const result = await tools.get("mcp_grillforge_pi_run_agent").execute(
    "call-a",
    { tag: "a" },
    undefined,
    (update) => updates.push(update),
    {},
  );

  assert.deepEqual(
    updates.map((update) => update.content[0].text),
    ["a-1", "a-2", "a-3"],
  );
  assert.deepEqual(result, {
    content: [{ type: "text", text: "final-a" }],
    details: { tag: "a" },
  });
  assert.equal(JSON.stringify(result).includes("a-1"), false);
  assert.equal(requestOptions[0].resetTimeoutOnProgress, true);
});

test("keeps concurrent MCP progress updates on their originating Pi tool call", async () => {
  const { FakeClient, pi, tools, upstream } = harness();
  await wrapPiMcpExtension(upstream, FakeClient)(pi);
  const tool = tools.get("mcp_grillforge_pi_run_agent");
  const a = [];
  const b = [];

  const [resultA, resultB] = await Promise.all([
    tool.execute("call-a", { tag: "a" }, undefined, (update) => a.push(update), {}),
    tool.execute("call-b", { tag: "b" }, undefined, (update) => b.push(update), {}),
  ]);

  assert.deepEqual(a.map((update) => update.content[0].text), ["a-1", "a-2", "a-3"]);
  assert.deepEqual(b.map((update) => update.content[0].text), ["b-1", "b-2", "b-3"]);
  assert.equal(resultA.content[0].text, "final-a");
  assert.equal(resultB.content[0].text, "final-b");
});

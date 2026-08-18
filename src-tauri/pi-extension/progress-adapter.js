import { AsyncLocalStorage } from "node:async_hooks";

const executions = new AsyncLocalStorage();
const patchedClients = new WeakSet();

function progressResult(progress) {
  const message =
    typeof progress.message === "string" && progress.message.trim()
      ? progress.message.trim()
      : progress.total === undefined
        ? `Progress ${progress.progress}`
        : `Progress ${progress.progress}/${progress.total}`;
  return {
    content: [{ type: "text", text: message }],
    details: { progress: progress.progress, total: progress.total },
  };
}

function patchClient(Client) {
  if (patchedClients.has(Client)) return;
  const originalRequest = Client.prototype.request;
  Client.prototype.request = function (request, schema, options = {}) {
    const execution = executions.getStore();
    if (!execution || request?.method !== "tools/call") {
      return originalRequest.call(this, request, schema, options);
    }
    const previousProgress = options.onprogress;
    return originalRequest.call(this, request, schema, {
      ...options,
      resetTimeoutOnProgress: true,
      onprogress(progress) {
        previousProgress?.(progress);
        execution.onUpdate?.(progressResult(progress));
      },
    });
  };
  patchedClients.add(Client);
}

export function wrapPiMcpExtension(upstreamExtension, Client) {
  patchClient(Client);
  return async function grillforgePiMcpExtension(pi) {
    const api = new Proxy(pi, {
      get(target, property, receiver) {
        if (property === "registerTool") {
          return (tool) =>
            target.registerTool({
              ...tool,
              execute(...args) {
                return executions.run(
                  { onUpdate: args[3] },
                  () => tool.execute.apply(tool, args),
                );
              },
            });
        }
        const value = Reflect.get(target, property, receiver);
        return typeof value === "function" ? value.bind(target) : value;
      },
    });
    return upstreamExtension(api);
  };
}

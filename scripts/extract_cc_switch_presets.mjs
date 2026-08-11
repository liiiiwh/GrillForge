#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const PINNED_COMMIT = "413c09e0790c304506888ae24b9be72820aca126";
const scriptDir = dirname(fileURLToPath(import.meta.url));
const root = resolve(scriptDir, "..");
const upstreamRoot = resolve(root, "upstream/cc-switch");
const sources = {
  claude_code: {
    file: "src/config/claudeProviderPresets.ts",
    variable: "providerPresets",
  },
  codex: {
    file: "src/config/codexProviderPresets.ts",
    variable: "codexProviderPresets",
  },
  gemini: {
    file: "src/config/geminiProviderPresets.ts",
    variable: "geminiProviderPresets",
  },
};
const outputPath = resolve(root, "src-tauri/src/presets/catalog.json");

const commit = execFileSync("git", ["-C", upstreamRoot, "rev-parse", "HEAD"], {
  encoding: "utf8",
}).trim();
if (commit !== PINNED_COMMIT) {
  throw new Error(`cc-switch commit mismatch: expected ${PINNED_COMMIT}, got ${commit}`);
}

function propertyName(node, sourceFile) {
  if (ts.isIdentifier(node) || ts.isStringLiteral(node) || ts.isNumericLiteral(node)) {
    return node.text;
  }
  throw new Error(`unsupported property name at ${sourceFile.getLineAndCharacterOfPosition(node.pos).line + 1}`);
}

function staticValue(node, sourceFile) {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  if (ts.isNumericLiteral(node)) return Number(node.text);
  if (node.kind === ts.SyntaxKind.TrueKeyword) return true;
  if (node.kind === ts.SyntaxKind.FalseKeyword) return false;
  if (node.kind === ts.SyntaxKind.NullKeyword) return null;
  if (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isSatisfiesExpression(node)) {
    return staticValue(node.expression, sourceFile);
  }
  if (ts.isPrefixUnaryExpression(node) && ts.isNumericLiteral(node.operand)) {
    const value = Number(node.operand.text);
    return node.operator === ts.SyntaxKind.MinusToken ? -value : value;
  }
  if (ts.isArrayLiteralExpression(node)) {
    return node.elements.map((element) => staticValue(element, sourceFile));
  }
  if (ts.isObjectLiteralExpression(node)) {
    const result = {};
    for (const property of node.properties) {
      if (!ts.isPropertyAssignment(property)) {
        throw new Error(`non-static object member at ${sourceFile.getLineAndCharacterOfPosition(property.pos).line + 1}`);
      }
      result[propertyName(property.name, sourceFile)] = staticValue(property.initializer, sourceFile);
    }
    return result;
  }
  if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
    const name = node.expression.text;
    const args = node.arguments.map((argument) => staticValue(argument, sourceFile));
    if (name === "generateThirdPartyAuth") return { OPENAI_API_KEY: args[0] ?? "" };
    if (name === "modelCatalog") return args[0];
    if (name === "generateThirdPartyConfig") {
      const [providerName, baseUrl, modelName = "gpt-5.6-sol"] = args;
      return `model_provider = "custom"
model = ${JSON.stringify(modelName)}
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = ${JSON.stringify(providerName)}
base_url = ${JSON.stringify(baseUrl)}
wire_api = "responses"
requires_openai_auth = true`;
    }
  }
  throw new Error(
    `non-static expression '${node.getText(sourceFile).slice(0, 80)}' at line ${sourceFile.getLineAndCharacterOfPosition(node.pos).line + 1}`,
  );
}

function readStaticPresets(source) {
  const path = resolve(upstreamRoot, source.file);
  const text = readFileSync(path, "utf8");
  const sourceFile = ts.createSourceFile(
    path,
    text,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  let presets;
  for (const statement of sourceFile.statements) {
    if (!ts.isVariableStatement(statement)) continue;
    for (const declaration of statement.declarationList.declarations) {
      if (ts.isIdentifier(declaration.name) && declaration.name.text === source.variable) {
        presets = staticValue(declaration.initializer, sourceFile);
      }
    }
  }
  if (!Array.isArray(presets)) throw new Error(`${source.variable} array was not found in ${source.file}`);
  return { ...source, text, presets };
}

const sourceDocuments = Object.fromEntries(
  Object.entries(sources).map(([client, source]) => [client, readStaticPresets(source)]),
);

const EXCLUDED = {
  official: "native_default",
  cloud_provider: "bedrock_requires_agent_specific_auth",
  custom: "custom_template",
  github_copilot: "managed_oauth",
  codex_oauth: "managed_oauth",
  xai_oauth: "managed_oauth",
};

const ID_OVERRIDES = new Map([["火山Agentplan", "volcengine-agentplan"]]);
const EXPLICIT_MODEL_PROTOCOL_CAPABILITIES = new Map([
  ["DeepSeek|openai_responses", {
    "deepseek-v4-pro": ["reasoning_items"],
    "deepseek-v4-flash": ["reasoning_items"],
  }],
]);
const MODEL_FIELDS = [
  "ANTHROPIC_MODEL",
  "ANTHROPIC_DEFAULT_HAIKU_MODEL",
  "ANTHROPIC_DEFAULT_SONNET_MODEL",
  "ANTHROPIC_DEFAULT_OPUS_MODEL",
];

function slug(value) {
  const overridden = ID_OVERRIDES.get(value);
  if (overridden) return overridden;
  const normalized = value
    .normalize("NFKD")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (!normalized) throw new Error(`preset needs an explicit stable id: ${value}`);
  return normalized;
}

function snake(value) {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[^A-Za-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .toLowerCase();
}

function exclusionReason(preset) {
  if (preset.category === "official") return EXCLUDED.official;
  if (preset.category === "cloud_provider") return EXCLUDED.cloud_provider;
  if (preset.category === "custom") return EXCLUDED.custom;
  if (preset.requiresOAuth === true || preset.providerType) {
    const reason = EXCLUDED[preset.providerType];
    if (!reason) throw new Error(`unknown managed provider type: ${preset.providerType}`);
    return reason;
  }
  return null;
}

function endpointFor(preset, baseUrl) {
  const matches = [...baseUrl.matchAll(/\$\{([A-Z][A-Z0-9_]*)\}/g)];
  if (matches.length === 0) return { kind: "literal", url: baseUrl };

  const definitions = preset.templateValues ?? {};
  const keys = [...new Set(matches.map((match) => match[1]))];
  const parameters = keys.map((key) => {
    const definition = definitions[key];
    if (!definition) throw new Error(`${preset.name}: endpoint placeholder ${key} is not declared`);
    return {
      id: snake(key),
      label: definition.label,
      placeholder: definition.placeholder,
      required: true,
      ...(definition.defaultValue ? { default_value: definition.defaultValue } : {}),
    };
  });

  let template = baseUrl;
  for (const key of keys) template = template.replaceAll(`\${${key}}`, `{${snake(key)}}`);
  return { kind: "parameterized", template, parameters };
}

function protocolFor(preset) {
  switch (preset.apiFormat ?? "anthropic") {
    case "anthropic": return "anthropic_messages";
    case "openai_chat": return "openai_chat_completions";
    case "openai_responses": return "openai_responses";
    case "gemini_native": return "gemini_native";
    default: throw new Error(`${preset.name}: unsupported protocol ${preset.apiFormat}`);
  }
}

function tomlString(config, key) {
  if (typeof config !== "string") return null;
  const match = config.match(new RegExp(`^${key}\\s*=\\s*("(?:[^"\\\\]|\\\\.)*")`, "m"));
  return match ? JSON.parse(match[1]) : null;
}

function uniqueStrings(values) {
  return [...new Set(values.filter((value) => typeof value === "string" && value.length > 0))];
}

function unsupportedCompatibility() {
  return {
    mode: "unsupported",
    protocol: null,
    auth: null,
    endpoint: null,
    suggested_models: [],
  };
}

function claudeCompatibility(preset) {
  const env = preset.settingsConfig?.env;
  const baseUrl = env?.ANTHROPIC_BASE_URL;
  if (typeof baseUrl !== "string" || baseUrl.length === 0) return null;
  const protocol = protocolFor(preset);
  // cc-switch proxy/providers/claude.rs: Anthropic is passthrough; Chat,
  // Responses and Gemini Native require a local transform.
  return {
    mode: protocol === "anthropic_messages" ? "direct" : "local_route",
    protocol,
    auth: protocol === "anthropic_messages" && preset.apiKeyField === "ANTHROPIC_API_KEY"
      ? "x_api_key"
      : "bearer",
    endpoint: endpointFor(preset, baseUrl),
    suggested_models: uniqueStrings(MODEL_FIELDS.map((field) => env[field])),
  };
}

function codexCompatibility(preset) {
  const baseUrl = tomlString(preset.config, "base_url");
  if (!baseUrl) return null;
  const wireApi = preset.apiFormat ?? tomlString(preset.config, "wire_api") ?? "openai_responses";
  const protocol = wireApi === "openai_chat" || wireApi === "chat"
    ? "openai_chat_completions"
    : wireApi === "anthropic"
      ? "anthropic_messages"
      : "openai_responses";
  const catalogModels = Array.isArray(preset.modelCatalog)
    ? preset.modelCatalog.map((model) => typeof model === "string" ? model : model.model)
    : [];
  // Codex always speaks Responses. cc-switch passes Responses upstream and
  // locally converts explicit Chat/Anthropic providers.
  return {
    mode: protocol === "openai_responses" ? "direct" : "local_route",
    protocol,
    auth: "bearer",
    endpoint: { kind: "literal", url: baseUrl },
    suggested_models: uniqueStrings([
      tomlString(preset.config, "model"),
      tomlString(preset.config, "review_model"),
      ...catalogModels,
    ]),
  };
}

function codexModelProtocolCapabilities(preset, compatibility) {
  if (!compatibility || compatibility.protocol !== "openai_chat_completions") return {};
  const reasoning = preset.codexChatReasoning;
  if (!reasoning) return {};

  const capabilities = [];
  if (reasoning.outputFormat === "reasoning_content") capabilities.push("reasoning_content");
  if (reasoning.supportsEffort === true && reasoning.effortParam !== "none") {
    capabilities.push("reasoning_effort");
  }
  if (capabilities.length === 0) return {};
  return Object.fromEntries(
    compatibility.suggested_models.map((model) => [model, capabilities]),
  );
}

function mergeCapabilityMaps(...maps) {
  const merged = {};
  for (const map of maps) {
    for (const [model, capabilities] of Object.entries(map ?? {})) {
      merged[model] = uniqueStrings([...(merged[model] ?? []), ...capabilities]);
    }
  }
  return merged;
}

function geminiCompatibility(preset) {
  const env = preset.settingsConfig?.env;
  const baseUrl = preset.baseURL ?? env?.GOOGLE_GEMINI_BASE_URL;
  if (typeof baseUrl !== "string" || baseUrl.length === 0) return null;
  // Gemini CLI and these presets both speak native generateContent.
  return {
    mode: "direct",
    protocol: "gemini_native",
    auth: "x_api_key",
    endpoint: { kind: "literal", url: baseUrl },
    suggested_models: uniqueStrings([preset.model, env?.GEMINI_MODEL]),
  };
}

function routesByName(presets, route) {
  const routes = new Map();
  for (const preset of presets) {
    if (exclusionReason(preset)) continue;
    const compatibility = route(preset);
    if (!compatibility) continue;
    if (routes.has(preset.name)) throw new Error(`duplicate client preset name: ${preset.name}`);
    routes.set(preset.name, compatibility);
  }
  return routes;
}

const claudeRoutes = routesByName(sourceDocuments.claude_code.presets, claudeCompatibility);
const codexRoutes = routesByName(sourceDocuments.codex.presets, codexCompatibility);
const geminiRoutes = routesByName(sourceDocuments.gemini.presets, geminiCompatibility);
const codexCapabilities = new Map();
for (const preset of sourceDocuments.codex.presets) {
  const compatibility = codexRoutes.get(preset.name);
  const capabilities = codexModelProtocolCapabilities(preset, compatibility);
  if (Object.keys(capabilities).length > 0) codexCapabilities.set(preset.name, capabilities);
}

const exclusionsByKey = new Map();
for (const [client, source] of Object.entries(sourceDocuments)) {
  for (const preset of source.presets) {
    const reason = exclusionReason(preset);
    if (reason) {
      exclusionsByKey.set(`${client}|${preset.name}|${reason}`, {
        client,
        name: preset.name,
        reason,
      });
    }
  }
}
const exclusions = [...exclusionsByKey.values()].sort((left, right) =>
  left.client.localeCompare(right.client) || left.name.localeCompare(right.name));

const modelsUrls = new Map();
for (const preset of sourceDocuments.claude_code.presets) {
  if (typeof preset.modelsUrl === "string" && preset.modelsUrl.length > 0) {
    modelsUrls.set(preset.name, preset.modelsUrl);
  }
}

function compatibilityForUpstream(route, client) {
  const supported = (() => {
    if (client === "claude_code") {
      return route.protocol === "anthropic_messages" ? "direct" : "local_route";
    }
    if (client === "codex") {
      if (route.protocol === "openai_responses") return "direct";
      if (route.protocol === "anthropic_messages" || route.protocol === "openai_chat_completions") {
        return "local_route";
      }
      return null;
    }
    return route.protocol === "gemini_native" ? "direct" : null;
  })();
  if (!supported) return unsupportedCompatibility();
  return {
    mode: supported,
    protocol: route.protocol,
    auth: route.auth,
    endpoint: route.endpoint,
    suggested_models: route.suggested_models,
  };
}

const protocolPriority = new Map([
  ["openai_responses", 0],
  ["anthropic_messages", 1],
  ["openai_chat_completions", 2],
  ["gemini_native", 3],
]);
const protocolSlug = {
  openai_responses: "responses",
  anthropic_messages: "anthropic",
  openai_chat_completions: "chat",
  gemini_native: "gemini",
};
const protocolName = {
  openai_responses: "OpenAI Responses",
  anthropic_messages: "Anthropic Messages",
  openai_chat_completions: "OpenAI Chat Completions",
  gemini_native: "Gemini Native",
};
const sourceClientName = {
  claude_code: "Claude Code",
  codex: "Codex",
  gemini: "Gemini",
};

function routeKey(route) {
  return `${route.protocol}|${route.auth}|${JSON.stringify(route.endpoint)}`;
}

function mergeRoute(group, route, sourceClient, capabilities) {
  group.sources.add(sourceClient);
  group.suggested_models = uniqueStrings([...group.suggested_models, ...route.suggested_models]);
  group.model_protocol_capabilities = mergeCapabilityMaps(
    group.model_protocol_capabilities,
    capabilities,
  );
}

const names = new Set([...claudeRoutes.keys(), ...codexRoutes.keys(), ...geminiRoutes.keys()]);
const presets = [];
const ids = new Set();
for (const name of [...names].sort((left, right) => left.localeCompare(right))) {
  const groups = new Map();
  for (const [sourceClient, route, capabilities] of [
    ["claude_code", claudeRoutes.get(name), {}],
    ["codex", codexRoutes.get(name), codexCapabilities.get(name) ?? {}],
    ["gemini", geminiRoutes.get(name), {}],
  ]) {
    if (!route) continue;
    const key = routeKey(route);
    const group = groups.get(key) ?? {
      protocol: route.protocol,
      auth: route.auth,
      endpoint: route.endpoint,
      suggested_models: [],
      model_protocol_capabilities: {},
      sources: new Set(),
    };
    mergeRoute(group, route, sourceClient, capabilities);
    groups.set(key, group);
  }

  const routes = [...groups.values()].sort((left, right) => {
    const priority = protocolPriority.get(left.protocol) - protocolPriority.get(right.protocol);
    return priority || routeKey(left).localeCompare(routeKey(right));
  });
  const routeProtocolCounts = routes.reduce((counts, route) => {
    counts.set(route.protocol, (counts.get(route.protocol) ?? 0) + 1);
    return counts;
  }, new Map());
  const baseId = slug(name);
  const protocolVariants = new Map();
  for (const [index, route] of routes.entries()) {
    route.model_protocol_capabilities = mergeCapabilityMaps(
      route.model_protocol_capabilities,
      EXPLICIT_MODEL_PROTOCOL_CAPABILITIES.get(`${name}|${route.protocol}`),
    );
    const variantNumber = (protocolVariants.get(route.protocol) ?? 0) + 1;
    protocolVariants.set(route.protocol, variantNumber);
    let id = index === 0 ? baseId : `${baseId}-${protocolSlug[route.protocol]}`;
    if (ids.has(id)) id = `${id}-${variantNumber}`;
    if (ids.has(id)) throw new Error(`duplicate stable preset id: ${id}`);
    ids.add(id);
    const sourceSuffix = [...route.sources]
      .sort()
      .map((source) => sourceClientName[source])
      .join(" / ");
    const displaySuffix = routeProtocolCounts.get(route.protocol) === 1
      ? protocolName[route.protocol]
      : `${protocolName[route.protocol]} · ${sourceSuffix}`;
    presets.push({
      id,
      name: index === 0 ? name : `${name} · ${displaySuffix}`,
      protocol: route.protocol,
      auth: route.auth,
      endpoint: route.endpoint,
      suggested_models: route.suggested_models,
      model_protocol_capabilities: route.model_protocol_capabilities,
      models_url: modelsUrls.get(name) ?? null,
      client_compatibility: {
        claude_code: compatibilityForUpstream(route, "claude_code"),
        codex: compatibilityForUpstream(route, "codex"),
        gemini: compatibilityForUpstream(route, "gemini"),
      },
    });
  }
}

function fnv1a64(text) {
  let hash = 0xcbf29ce484222325n;
  for (const byte of Buffer.from(text, "utf8")) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }
  return hash.toString(16).padStart(16, "0");
}

const catalog = {
  schema_version: 2,
  source: {
    repository: "https://github.com/farion1231/cc-switch",
    commit: PINNED_COMMIT,
    provider_count: Object.values(sourceDocuments)
      .reduce((count, source) => count + source.presets.length, 0),
    files: Object.values(sourceDocuments).map((source) => ({
      file: source.file,
      fnv1a64: fnv1a64(source.text),
      provider_count: source.presets.length,
    })),
  },
  exclusions,
  presets,
};

if (catalog.source.provider_count !== 161 || catalog.presets.length !== 151 || catalog.exclusions.length !== 10) {
  throw new Error(
    `unexpected extraction counts: source=${catalog.source.provider_count}, included=${catalog.presets.length}, excluded=${catalog.exclusions.length}`,
  );
}

const rendered = `${JSON.stringify(catalog, null, 2)}\n`;
if (process.argv.includes("--check")) {
  const checkedIn = readFileSync(outputPath, "utf8");
  if (checkedIn !== rendered) throw new Error("checked-in preset catalog is stale; run this script without --check");
  console.log(`verified ${catalog.presets.length} checked-in presets`);
} else {
  writeFileSync(outputPath, rendered, "utf8");
  console.log(`wrote ${catalog.presets.length} presets to ${outputPath}`);
}

import { Buffer } from "node:buffer";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

function isObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function isNonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function clampInt(value, fallback, min, max) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    return fallback;
  }
  const rounded = Math.floor(parsed);
  return Math.max(min, Math.min(max, rounded));
}

function estimateTokens(text) {
  if (typeof text !== "string" || text.length === 0) {
    return 0;
  }

  const lengthBased = Math.ceil(text.length / 4);
  const words = text.trim() ? text.trim().split(/\s+/).length : 0;
  const wordBased = Math.ceil(words * 1.33);
  const hasCjk = /[\u3040-\u30ff\u3400-\u9fff\uf900-\ufaff]/.test(text);
  if (hasCjk) {
    return Math.max(lengthBased, Math.ceil(text.length * 0.85));
  }
  return Math.max(lengthBased, wordBased);
}

function estimateBytes(text) {
  if (typeof text !== "string") {
    return 0;
  }
  return Buffer.byteLength(text, "utf8");
}

function compactByBudget(text, limits) {
  if (typeof text !== "string") {
    return {
      text,
      truncated: false,
      estimatedTokensBefore: 0,
      estimatedTokensAfter: 0,
    };
  }

  const estimatedTokensBefore = estimateTokens(text);
  const withinChar = text.length <= limits.maxChars;
  const withinToken = estimatedTokensBefore <= limits.maxTokens;
  if (withinChar && withinToken) {
    return {
      text,
      truncated: false,
      estimatedTokensBefore,
      estimatedTokensAfter: estimatedTokensBefore,
    };
  }

  const charBudgetFromTokens = Math.max(800, limits.maxTokens * 4);
  const effectiveCharBudget = Math.max(
    800,
    Math.min(limits.maxChars, charBudgetFromTokens),
  );

  const omittedTokens = Math.max(0, estimatedTokensBefore - limits.maxTokens);
  const marker = `\n\n[moon truncated ~${omittedTokens} tokens; ` +
    `full payload may be available in details]\n\n`;

  const sliceBudget = Math.max(220, effectiveCharBudget - marker.length);
  let head = Math.max(120, Math.floor(sliceBudget * 0.62));
  let tail = Math.max(80, sliceBudget - head);

  if (head + tail >= text.length) {
    head = Math.max(40, Math.floor(text.length * 0.6));
    tail = Math.max(20, Math.floor(text.length * 0.2));
  }

  const start = text.slice(0, head);
  const end = text.slice(-tail);
  const omittedChars = Math.max(0, text.length - head - tail);
  const compacted = `${start}${marker}[omitted ${omittedChars} chars]\n${end}`;

  return {
    text: compacted,
    truncated: true,
    estimatedTokensBefore,
    estimatedTokensAfter: estimateTokens(compacted),
  };
}

function projectJsonSummary(text) {
  try {
    const parsed = JSON.parse(text);
    if (Array.isArray(parsed)) {
      return JSON.stringify(
        {
          kind: "array",
          length: parsed.length,
          sample: parsed.slice(0, 20),
        },
        null,
        2,
      );
    }
    if (isObject(parsed)) {
      const keys = Object.keys(parsed);
      const sample = {};
      for (const key of keys.slice(0, 20)) {
        const value = parsed[key];
        if (Array.isArray(value)) {
          sample[key] = `[array:${value.length}]`;
        } else if (isObject(value)) {
          sample[key] = `[object:${Object.keys(value).length} keys]`;
        } else {
          sample[key] = value;
        }
      }
      return JSON.stringify(
        {
          kind: "object",
          keyCount: keys.length,
          keys: keys.slice(0, 60),
          sample,
        },
        null,
        2,
      );
    }
  } catch {
    return null;
  }
  return null;
}

function resolvePathSetting(api, value) {
  if (!isNonEmptyString(value)) {
    return null;
  }

  const trimmed = value.trim();
  const normalized = trimmed === "~"
    ? os.homedir()
    : trimmed.startsWith("~/")
    ? path.join(os.homedir(), trimmed.slice(2))
    : trimmed;
  if (path.isAbsolute(normalized)) {
    return normalized;
  }

  if (typeof api?.resolvePath === "function") {
    try {
      const resolved = api.resolvePath(normalized);
      if (isNonEmptyString(resolved)) {
        return resolved.trim();
      }
    } catch {
      return normalized;
    }
  }

  return normalized;
}

function resolveProcessCwd() {
  try {
    return process.cwd();
  } catch {
    return null;
  }
}

const DEFAULT_TOOL_PROFILES = {
  read: { maxTokens: 6000, maxChars: 32000 },
  "message/readMessages": { maxTokens: 5000, maxChars: 28000 },
  "message/searchMessages": { maxTokens: 5000, maxChars: 28000 },
  web_fetch: { maxTokens: 7000, maxChars: 35000 },
  "web.fetch": { maxTokens: 7000, maxChars: 35000 },
};

const DEFAULT_CONTEXT_ENGINE_TIMEOUT_MS = 120_000;
const DEFAULT_FALLBACK_MODE = "disabled";
const DEFAULT_CONTEXT_PACKET_MAX_TOKENS = 1_400;
const DEFAULT_CONTEXT_PACKET_CANDIDATE_THRESHOLD = 10;
const DEFAULT_ASSEMBLY_SUBAGENT_MODE = "disabled";
const DEFAULT_ASSEMBLY_SUBAGENT_TIMEOUT_MS = 15_000;
const DEFAULT_ASSEMBLY_SUBAGENT_CACHE_TTL_MS = 300_000;
const DEFAULT_ASSEMBLY_SUBAGENT_MODELS = {
  openai: "gpt-5.4-mini",
  "openai-codex": "gpt-5.4-mini",
  anthropic: "claude-3-5-haiku-latest",
  google: "gemini-3.1-flash-lite-preview",
  "openai-compatible": "deepseek-chat",
};
const MOON_CLEANSE_TARGET_TOKENS = 40_000;
const MIN_COMPACTION_TARGET_TOKENS = 2_000;
const COMPACTION_TARGET_BUDGET_RATIO = 0.45;
const COMPACTION_SUMMARY_HEADROOM_TOKENS = 200;

function normalizeFallbackMode(raw) {
  if (!isNonEmptyString(raw)) {
    return DEFAULT_FALLBACK_MODE;
  }
  const normalized = raw.trim().toLowerCase();
  if (
    normalized === "disabled" || normalized === "off" || normalized === "none"
  ) {
    return "disabled";
  }
  return "openclaw";
}

function normalizeAssemblySubagentMode(raw) {
  if (!isNonEmptyString(raw)) {
    return DEFAULT_ASSEMBLY_SUBAGENT_MODE;
  }
  const normalized = raw.trim().toLowerCase();
  if (
    normalized === "off" || normalized === "none" || normalized === "disabled"
  ) {
    return "disabled";
  }
  return "gated";
}

function normalizeAssemblySubagentProviderKey(raw) {
  if (!isNonEmptyString(raw)) {
    return null;
  }
  const normalized = raw.trim().toLowerCase();
  switch (normalized) {
    case "openai":
      return "openai";
    case "openai-codex":
    case "codex":
      return "openai-codex";
    case "anthropic":
    case "claude":
      return "anthropic";
    case "google":
    case "gemini":
      return "google";
    case "openai-compatible":
    case "compatible":
    case "deepseek":
      return "openai-compatible";
    default:
      return normalized;
  }
}

function inferAssemblySubagentProvider(model) {
  if (!isNonEmptyString(model)) {
    return null;
  }
  const normalized = model.trim().toLowerCase();
  if (
    normalized.startsWith("gpt-") || normalized.startsWith("o1") ||
    normalized.startsWith("o3") || normalized.startsWith("o4")
  ) {
    return "openai";
  }
  if (normalized.startsWith("claude-")) {
    return "anthropic";
  }
  if (normalized.startsWith("gemini-")) {
    return "google";
  }
  if (normalized.startsWith("deepseek-")) {
    return "openai-compatible";
  }
  return null;
}

function defaultAssemblySubagentModel(provider) {
  const normalized = normalizeAssemblySubagentProviderKey(provider);
  if (!normalized) {
    return null;
  }
  return DEFAULT_ASSEMBLY_SUBAGENT_MODELS[normalized] ?? null;
}

function resolveLimits(pluginConfig, toolName) {
  const globalMaxTokens = clampInt(pluginConfig.maxTokens, 12000, 500, 500000);
  const globalMaxChars = clampInt(pluginConfig.maxChars, 60000, 1000, 200000);
  const maxRetainedBytes = clampInt(
    pluginConfig.maxRetainedBytes,
    250000,
    0,
    5000000,
  );

  const profileDefault = isObject(DEFAULT_TOOL_PROFILES[toolName])
    ? DEFAULT_TOOL_PROFILES[toolName]
    : {};
  const toolCfg =
    isObject(pluginConfig.tools) && isObject(pluginConfig.tools[toolName])
      ? pluginConfig.tools[toolName]
      : {};

  const maxTokens = clampInt(
    toolCfg.maxTokens,
    clampInt(profileDefault.maxTokens, globalMaxTokens, 100, 500000),
    100,
    500000,
  );
  const maxChars = clampInt(
    toolCfg.maxChars,
    clampInt(profileDefault.maxChars, globalMaxChars, 200, 200000),
    200,
    200000,
  );

  return { maxTokens, maxChars, maxRetainedBytes };
}

function resolveContextEngineSettings(api) {
  const pluginConfig = isObject(api?.pluginConfig) ? api.pluginConfig : {};
  const explicitAssemblySubagentProvider = isNonEmptyString(
      pluginConfig.assemblySubagentProvider,
    )
    ? pluginConfig.assemblySubagentProvider.trim()
    : null;
  const explicitAssemblySubagentModel = isNonEmptyString(
      pluginConfig.assemblySubagentModel,
    )
    ? pluginConfig.assemblySubagentModel.trim()
    : null;
  const inferredAssemblySubagentProvider = explicitAssemblySubagentProvider ||
    inferAssemblySubagentProvider(explicitAssemblySubagentModel);

  return {
    moonPath: resolvePathSetting(api, pluginConfig.moonPath) ||
      (isNonEmptyString(process.env.MOON_BIN)
        ? process.env.MOON_BIN.trim()
        : "moon"),
    moonHome: resolvePathSetting(api, pluginConfig.moonHome) ||
      (isNonEmptyString(process.env.MOON_HOME)
        ? process.env.MOON_HOME.trim()
        : null),
    memoryDir: resolvePathSetting(api, pluginConfig.memoryDir),
    memoryFile: resolvePathSetting(api, pluginConfig.memoryFile),
    contextEngineTimeoutMs: clampInt(
      pluginConfig.contextEngineTimeoutMs,
      DEFAULT_CONTEXT_ENGINE_TIMEOUT_MS,
      1000,
      300000,
    ),
    syncAfterTurn: pluginConfig.syncAfterTurn !== false,
    fallbackMode: normalizeFallbackMode(
      pluginConfig.fallbackMode ||
        (isNonEmptyString(process.env.MOON_CONTEXT_ENGINE_FALLBACK_MODE)
          ? process.env.MOON_CONTEXT_ENGINE_FALLBACK_MODE
          : DEFAULT_FALLBACK_MODE),
    ),
    compactFallbackOnSkip: pluginConfig.compactFallbackOnSkip === true,
    contextPacketMaxTokens: clampInt(
      pluginConfig.contextPacketMaxTokens,
      DEFAULT_CONTEXT_PACKET_MAX_TOKENS,
      200,
      20_000,
    ),
    contextPacketCandidateThreshold: clampInt(
      pluginConfig.contextPacketCandidateThreshold,
      DEFAULT_CONTEXT_PACKET_CANDIDATE_THRESHOLD,
      1,
      100,
    ),
    assemblySubagentMode: normalizeAssemblySubagentMode(
      pluginConfig.assemblySubagentMode,
    ),
    assemblySubagentProvider: inferredAssemblySubagentProvider,
    assemblySubagentModel: explicitAssemblySubagentModel ||
      defaultAssemblySubagentModel(inferredAssemblySubagentProvider),
    assemblySubagentTimeoutMs: clampInt(
      pluginConfig.assemblySubagentTimeoutMs,
      DEFAULT_ASSEMBLY_SUBAGENT_TIMEOUT_MS,
      1000,
      120000,
    ),
    assemblySubagentCacheTtlMs: clampInt(
      pluginConfig.assemblySubagentCacheTtlMs,
      DEFAULT_ASSEMBLY_SUBAGENT_CACHE_TTL_MS,
      1000,
      86_400_000,
    ),
  };
}

function openclawFallbackEnabled(settings) {
  return settings?.fallbackMode === "openclaw";
}

function fallbackReason(trigger, reason) {
  return `moon->openclaw fallback trigger=${trigger} reason=${reason}`;
}

function messagesContainCompactionSummary(messages) {
  return Array.isArray(messages) &&
    messages.some((message) => message?.role === "compactionSummary");
}

function extractPromptText(messages, explicitPrompt) {
  if (isNonEmptyString(explicitPrompt)) {
    return explicitPrompt.trim();
  }
  if (!Array.isArray(messages)) {
    return "";
  }
  for (let idx = messages.length - 1; idx >= 0; idx -= 1) {
    const message = messages[idx];
    if (message?.role !== "user") {
      continue;
    }
    const text = extractMessageText(message);
    if (isNonEmptyString(text)) {
      return text.trim();
    }
  }
  return "";
}

function extractMessageText(message) {
  if (!message) {
    return "";
  }
  if (typeof message.content === "string") {
    return message.content;
  }
  if (!Array.isArray(message.content)) {
    return "";
  }
  return message.content
    .map((item) => {
      if (typeof item === "string") {
        return item;
      }
      if (isObject(item) && typeof item.text === "string") {
        return item.text;
      }
      return "";
    })
    .filter(Boolean)
    .join("\n");
}

function buildMoonPacketMessage(packetText) {
  return {
    role: "assistant",
    timestamp: Date.now(),
    content: [{ type: "text", text: packetText }],
  };
}

function injectMoonPacketMessage(messages, packetText) {
  const normalized = cleanMoonPacketText(packetText);
  if (!isNonEmptyString(normalized)) {
    return Array.isArray(messages) ? messages : [];
  }
  const packetMessage = buildMoonPacketMessage(normalized);
  const next = Array.isArray(messages) ? [...messages] : [];
  if (next.length === 0) {
    return [packetMessage];
  }
  const lastMessage = next[next.length - 1];
  if (lastMessage?.role === "user") {
    next.splice(next.length - 1, 0, packetMessage);
    return next;
  }
  return [packetMessage, ...next];
}

function cleanMoonPacketText(text) {
  if (!isNonEmptyString(text)) {
    return "";
  }
  const trimmed = text.trim();
  if (!trimmed) {
    return "";
  }
  return trimmed.startsWith("# Moon Active Context")
    ? trimmed
    : `# Moon Active Context\n\n${trimmed}`;
}

function isRecallHeavyPrompt(prompt) {
  if (!isNonEmptyString(prompt)) {
    return false;
  }
  const lower = prompt.toLowerCase();
  return [
    "remember",
    "recall",
    "previous",
    "earlier",
    "history",
    "decision",
    "why did",
    "status",
    "context",
  ].some((term) => lower.includes(term));
}

function shouldUseAssemblySubagent(
  settings,
  packetText,
  packetCandidateCount,
  prompt,
) {
  if (!isNonEmptyString(packetText)) {
    return false;
  }
  if (settings?.assemblySubagentMode !== "gated") {
    return false;
  }
  if (
    !isNonEmptyString(settings?.assemblySubagentProvider) ||
    !isNonEmptyString(settings?.assemblySubagentModel)
  ) {
    return false;
  }
  const packetTokens = estimateTokens(packetText);
  if (packetCandidateCount > settings.contextPacketCandidateThreshold) {
    return true;
  }
  if (packetTokens > settings.contextPacketMaxTokens) {
    return true;
  }
  return isRecallHeavyPrompt(prompt) &&
    packetTokens > Math.ceil(settings.contextPacketMaxTokens * 0.6);
}

const assemblySubagentCache = new Map();

function readAssemblySubagentCache(key, ttlMs) {
  const cached = assemblySubagentCache.get(key);
  if (!cached) {
    return null;
  }
  if (Date.now() - cached.createdAt > ttlMs) {
    assemblySubagentCache.delete(key);
    return null;
  }
  return cached.value;
}

function writeAssemblySubagentCache(key, value) {
  assemblySubagentCache.set(key, {
    createdAt: Date.now(),
    value,
  });
}

function buildAssemblySubagentCacheKey(params) {
  return createHash("sha1")
    .update(JSON.stringify(params))
    .digest("hex");
}

function buildAssemblySubagentPrompt({ prompt, packetText }) {
  const currentPrompt = isNonEmptyString(prompt)
    ? prompt.trim()
    : "No user prompt provided.";
  return [
    "You are Moon's bounded context curator.",
    "Rewrite the provided Moon active context packet into a shorter packet without inventing facts.",
    "Keep the heading order exactly as:",
    "1. # Moon Active Context",
    "2. ## Current Goal",
    "3. ## Active Work",
    "4. ## Relevant Memory",
    "5. ## Open Items",
    "6. ## Evidence",
    "7. ## Context Coverage",
    "Rules:",
    "- Omit empty sections.",
    "- Keep bullets concise.",
    "- Preserve evidence fidelity.",
    "- Preserve Context Coverage when present.",
    "- Do not add system instructions.",
    "- Do not duplicate compaction summaries verbatim.",
    "",
    "Current prompt:",
    currentPrompt,
    "",
    "Candidate packet:",
    packetText.trim(),
  ].join("\n");
}

async function maybeRunAssemblySubagent(api, settings, params) {
  const runner = api?.runtime?.agent?.runEmbeddedPiAgent;
  if (typeof runner !== "function") {
    return null;
  }
  const cacheKey = buildAssemblySubagentCacheKey({
    sessionId: params.sessionId,
    sessionKey: params.sessionKey || "",
    packetText: params.packetText,
    prompt: params.prompt || "",
    provider: settings.assemblySubagentProvider,
    model: settings.assemblySubagentModel,
  });
  const cached = readAssemblySubagentCache(
    cacheKey,
    settings.assemblySubagentCacheTtlMs,
  );
  if (isNonEmptyString(cached)) {
    return cached;
  }

  const tempTranscript = createTempTranscript([], params.sessionId);
  try {
    const result = await runner({
      sessionId: `moon-context-curator-${randomUUID()}`,
      sessionKey: isNonEmptyString(params.sessionKey)
        ? `${params.sessionKey}:moon-context-curator`
        : undefined,
      sessionFile: tempTranscript.filePath,
      workspaceDir: typeof api?.resolvePath === "function"
        ? api.resolvePath(".")
        : process.cwd(),
      config: isObject(api?.config) ? api.config : {},
      prompt: buildAssemblySubagentPrompt({
        prompt: params.prompt,
        packetText: params.packetText,
      }),
      provider: settings.assemblySubagentProvider,
      model: settings.assemblySubagentModel,
      timeoutMs: settings.assemblySubagentTimeoutMs,
      runId: `moon-context-curator-${randomUUID()}`,
      trigger: "manual",
      toolsAllow: [],
      disableMessageTool: true,
      disableTools: true,
      bootstrapContextMode: "lightweight",
      verboseLevel: "off",
      reasoningLevel: "off",
      silentExpected: true,
    });
    const curated = cleanMoonPacketText(
      (result?.payloads ?? [])
        .map((payload) => payload?.text?.trim() ?? "")
        .filter(Boolean)
        .join("\n")
        .trim(),
    );
    if (!isNonEmptyString(curated)) {
      return null;
    }
    writeAssemblySubagentCache(cacheKey, curated);
    return curated;
  } finally {
    tempTranscript.cleanup();
  }
}

function compactToolResultMessage(message, toolName, pluginConfig) {
  if (!isObject(message)) {
    return message;
  }
  if (message.role !== "toolResult" || !Array.isArray(message.content)) {
    return message;
  }

  const limits = resolveLimits(pluginConfig, String(toolName || ""));
  const namesForJsonProjection = new Set([
    "read",
    "message/readMessages",
    "message/searchMessages",
    "web_fetch",
    "web.fetch",
  ]);

  const nextContent = [];
  const fullTextParts = [];
  let fullTextBytes = 0;
  let mutated = false;
  const strategies = new Set();
  let textBlockCount = 0;
  let compactedBlockCount = 0;
  let totalCharsBefore = 0;
  let totalCharsAfter = 0;
  let totalTokensBefore = 0;
  let totalTokensAfter = 0;

  for (const block of message.content) {
    if (
      !isObject(block) || block.type !== "text" ||
      typeof block.text !== "string"
    ) {
      nextContent.push(block);
      continue;
    }

    textBlockCount += 1;
    const originalText = block.text;
    let workingText = originalText;

    const blockChars = originalText.length;
    const blockTokens = estimateTokens(originalText);
    totalCharsBefore += blockChars;
    totalTokensBefore += blockTokens;

    fullTextParts.push(originalText);
    fullTextBytes += estimateBytes(originalText);

    if (
      namesForJsonProjection.has(String(toolName || "")) &&
      (blockChars > limits.maxChars || blockTokens > limits.maxTokens)
    ) {
      const projected = projectJsonSummary(originalText);
      if (projected) {
        workingText = `[moon projected JSON summary]\n${projected}`;
        strategies.add("json_projection");
      }
    }

    const compacted = compactByBudget(workingText, limits);
    totalCharsAfter += compacted.text.length;
    totalTokensAfter += compacted.estimatedTokensAfter;

    if (compacted.truncated || workingText !== originalText) {
      mutated = true;
      compactedBlockCount += 1;
      if (compacted.truncated) {
        strategies.add("head_tail_trim");
      }
      nextContent.push({ ...block, text: compacted.text });
    } else {
      nextContent.push(block);
    }
  }

  if (!mutated) {
    return message;
  }

  const details = isObject(message.details) ? { ...message.details } : {};
  const metadata = {
    compactedAt: new Date().toISOString(),
    toolName: toolName || null,
    strategy: strategies.size > 0
      ? Array.from(strategies).sort().join("+")
      : "head_tail_trim",
    textBlockCount,
    compactedBlockCount,
    originalTextChars: totalCharsBefore,
    finalTextChars: totalCharsAfter,
    estimatedTokensBefore: totalTokensBefore,
    estimatedTokensAfter: totalTokensAfter,
    maxTokens: limits.maxTokens,
    maxChars: limits.maxChars,
    maxRetainedBytes: limits.maxRetainedBytes,
    retrievalHint:
      "Use the original source/tool call id to refetch full payload if omitted from persisted text.",
  };

  if (limits.maxRetainedBytes > 0 && fullTextBytes <= limits.maxRetainedBytes) {
    metadata.fullText = fullTextParts.join("\n");
    metadata.fullTextRetained = true;
  } else {
    metadata.fullTextRetained = false;
  }

  details.moon = metadata;

  return { ...message, content: nextContent, details };
}

function extractReportDetail(report, prefix) {
  if (!isObject(report) || !Array.isArray(report.details)) {
    return null;
  }

  for (const detail of report.details) {
    if (typeof detail === "string" && detail.startsWith(prefix)) {
      return detail.slice(prefix.length);
    }
  }

  return null;
}

function parseCommandReport(raw) {
  if (!isNonEmptyString(raw)) {
    return null;
  }

  try {
    const parsed = JSON.parse(raw);
    if (
      isObject(parsed) && Array.isArray(parsed.details) &&
      Array.isArray(parsed.issues)
    ) {
      return parsed;
    }
  } catch {
    return null;
  }

  return null;
}

function serializeMessagesAsJsonl(messages) {
  if (!Array.isArray(messages) || messages.length === 0) {
    return "";
  }

  return `${
    messages.map((message) => JSON.stringify({ message })).join("\n")
  }\n`;
}

function sanitizeSessionId(sessionId) {
  if (!isNonEmptyString(sessionId)) {
    return "session";
  }
  return sessionId.trim().replace(/[^A-Za-z0-9._-]+/g, "-");
}

function createTempTranscript(messages, sessionId) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "moon-context-engine-"));
  const filePath = path.join(dir, `${sanitizeSessionId(sessionId)}.jsonl`);
  fs.writeFileSync(filePath, serializeMessagesAsJsonl(messages), "utf8");

  return {
    filePath,
    cleanup() {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        // Best-effort cleanup only.
      }
    },
  };
}

function estimateMessageTokens(messages) {
  if (!Array.isArray(messages) || messages.length === 0) {
    return 0;
  }

  try {
    return estimateTokens(JSON.stringify(messages));
  } catch {
    return 0;
  }
}

function logMoonPluginError(api, message) {
  const logger = api?.logger ?? api?.runtime?.logger ?? api?.log;
  if (typeof logger?.error === "function") {
    try {
      logger.error(message);
      return;
    } catch {
      // Fall back to stderr if the host logger rejects the payload.
    }
  }

  console.error(`[moon plugin] ${message}`);
}

function stripFrontMatter(text) {
  if (!isNonEmptyString(text)) {
    return "";
  }

  const trimmedStart = text.trimStart();
  if (!trimmedStart.startsWith("---")) {
    return text.trim();
  }

  const match = trimmedStart.match(/^---\r?\n[\s\S]*?\r?\n---\r?\n*/);
  if (!match) {
    return text.trim();
  }

  return trimmedStart.slice(match[0].length).trim();
}

function readFileIfExists(filePath) {
  if (!isNonEmptyString(filePath)) {
    return null;
  }

  const resolved = filePath.trim();
  if (!fs.existsSync(resolved)) {
    return null;
  }

  return fs.readFileSync(resolved, "utf8");
}

function parseJsonlEntries(raw) {
  if (!isNonEmptyString(raw)) {
    return [];
  }

  const entries = [];
  for (const line of raw.split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    try {
      entries.push(JSON.parse(line));
    } catch {
      // Ignore malformed lines; OpenClaw repair handles the authoritative path.
    }
  }
  return entries;
}

function resolveSessionBranch(entries) {
  const branchableEntries = entries.filter(
    (entry) =>
      isObject(entry) && entry.type !== "session" && isNonEmptyString(entry.id),
  );
  if (branchableEntries.length === 0) {
    return [];
  }

  const byId = new Map(branchableEntries.map((entry) => [entry.id, entry]));
  const seen = new Set();
  const pathEntries = [];
  let current = branchableEntries[branchableEntries.length - 1];

  while (
    isObject(current) && isNonEmptyString(current.id) && !seen.has(current.id)
  ) {
    pathEntries.unshift(current);
    seen.add(current.id);
    current = isNonEmptyString(current.parentId)
      ? byId.get(current.parentId)
      : null;
  }

  return pathEntries;
}

function isContextBearingEntry(entry) {
  return (
    isObject(entry) &&
    (entry.type === "message" || entry.type === "custom_message" ||
      entry.type === "branch_summary")
  );
}

function estimateEntryTokens(entry) {
  if (!isObject(entry)) {
    return 0;
  }

  if (entry.type === "message") {
    return estimateTokens(JSON.stringify(entry.message ?? {}));
  }
  if (entry.type === "custom_message") {
    return estimateTokens(
      JSON.stringify({
        customType: entry.customType,
        content: entry.content,
        details: entry.details,
      }),
    );
  }
  if (entry.type === "branch_summary") {
    return estimateTokens(entry.summary ?? "");
  }

  return 0;
}

function resolveCompactionTargetTokens(tokenBudget) {
  if (Number.isFinite(tokenBudget) && tokenBudget > 0) {
    const budgetTarget = Math.floor(
      tokenBudget * COMPACTION_TARGET_BUDGET_RATIO,
    );
    return Math.max(
      MIN_COMPACTION_TARGET_TOKENS,
      Math.min(MOON_CLEANSE_TARGET_TOKENS, budgetTarget),
    );
  }

  return MOON_CLEANSE_TARGET_TOKENS;
}

function normalizeCompactionSummary(text, targetTokens) {
  const summary = stripFrontMatter(text);
  if (!isNonEmptyString(summary)) {
    return null;
  }

  const summaryTokenBudget = Math.min(
    3000,
    Math.max(500, Math.floor(targetTokens * 0.35)),
  );
  const summaryCharBudget = Math.min(
    24_000,
    Math.max(2_000, summaryTokenBudget * 6),
  );
  return compactByBudget(summary, {
    maxTokens: summaryTokenBudget,
    maxChars: summaryCharBudget,
  }).text;
}

function generateSessionEntryId(existingIds) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const id = randomUUID().slice(0, 8);
    if (!existingIds.has(id)) {
      return id;
    }
  }
  return randomUUID();
}

function appendMoonCompactionEntry(sessionFile, params) {
  if (!isNonEmptyString(sessionFile)) {
    return {
      ok: false,
      compacted: false,
      reason: "session file missing",
    };
  }

  const resolvedSessionFile = sessionFile.trim();
  if (!fs.existsSync(resolvedSessionFile)) {
    return {
      ok: false,
      compacted: false,
      reason: `session file not found: ${resolvedSessionFile}`,
    };
  }

  const raw = fs.readFileSync(resolvedSessionFile, "utf8");
  const entries = parseJsonlEntries(raw);
  const header = entries[0];
  if (
    !isObject(header) || header.type !== "session" ||
    !isNonEmptyString(header.id)
  ) {
    return {
      ok: false,
      compacted: false,
      reason: "invalid OpenClaw session transcript header",
    };
  }

  const branch = resolveSessionBranch(entries);
  if (branch.length === 0) {
    return {
      ok: true,
      compacted: false,
      reason: "empty session",
    };
  }

  const targetTokens = resolveCompactionTargetTokens(params.tokenBudget);
  const summary = normalizeCompactionSummary(
    params.cleanseSummaryText,
    targetTokens,
  );
  if (!isNonEmptyString(summary)) {
    const reason = params.cleanseReason === "no-pressure-snapshot"
      ? "moon cleanse did not trigger"
      : "moon context-engine did not emit a readable cleanse summary";
    params.logError?.(
      `missing cleanse summary during compaction session_id=${
        params.sessionId ?? "unknown"
      } reason=${reason} cleanse_summary_path=${
        params.cleanseSummaryPath ?? "none"
      } assembly_path=${params.assemblyPath ?? "none"}`,
    );
    return {
      ok: true,
      compacted: false,
      reason,
    };
  }

  const contextEntries = branch.filter(isContextBearingEntry);
  const summaryTokens = estimateTokens(summary);
  const tailBudget = Math.max(
    0,
    targetTokens - summaryTokens - COMPACTION_SUMMARY_HEADROOM_TOKENS,
  );

  let keptTokens = 0;
  let keptEntryCount = 0;
  let firstKeptEntryId = branch[branch.length - 1]?.id ?? null;

  for (let index = contextEntries.length - 1; index >= 0; index -= 1) {
    const entry = contextEntries[index];
    const entryTokens = estimateEntryTokens(entry);
    if (keptEntryCount > 0 && keptTokens + entryTokens > tailBudget) {
      break;
    }

    keptTokens += entryTokens;
    keptEntryCount += 1;
    firstKeptEntryId = entry.id;
  }

  if (!isNonEmptyString(firstKeptEntryId)) {
    firstKeptEntryId = branch[branch.length - 1].id;
  }

  const existingIds = new Set(
    entries
      .filter((entry) => isObject(entry) && isNonEmptyString(entry.id))
      .map((entry) => entry.id),
  );
  const compactionEntry = {
    type: "compaction",
    id: generateSessionEntryId(existingIds),
    parentId: branch[branch.length - 1].id,
    timestamp: new Date().toISOString(),
    summary,
    firstKeptEntryId,
    tokensBefore: Number.isFinite(params.tokensBefore)
      ? Math.max(0, Math.floor(params.tokensBefore))
      : contextEntries.reduce(
        (total, entry) => total + estimateEntryTokens(entry),
        0,
      ),
    details: {
      moon: {
        source: "moon-context-engine",
        cleanseSummaryPath: params.cleanseSummaryPath ?? null,
        assemblyPath: params.assemblyPath ?? null,
        cleanseReason: params.cleanseReason ?? null,
        targetTokens,
        keptEntryCount,
        summaryTokens,
        keptTokens,
      },
    },
  };

  const leadingNewline = raw.length > 0 && !raw.endsWith("\n") ? "\n" : "";
  fs.appendFileSync(
    resolvedSessionFile,
    `${leadingNewline}${JSON.stringify(compactionEntry)}\n`,
    "utf8",
  );

  return {
    ok: true,
    compacted: true,
    result: {
      summary,
      firstKeptEntryId,
      tokensBefore: compactionEntry.tokensBefore,
      tokensAfter: summaryTokens + keptTokens,
      details: compactionEntry.details,
    },
  };
}

async function runMoonContextEngine(api, settings, params) {
  const sessionId = sanitizeSessionId(params.sessionId);
  const sourcePath = isNonEmptyString(params.sourcePath) &&
      fs.existsSync(params.sourcePath.trim())
    ? params.sourcePath.trim()
    : null;
  const usedTokens = Number.isFinite(params.usedTokens)
    ? Math.max(0, Math.floor(params.usedTokens))
    : null;
  const maxTokens = Number.isFinite(params.maxTokens)
    ? Math.max(1, Math.floor(params.maxTokens))
    : null;

  let tempTranscript = null;
  const effectiveSourcePath = sourcePath ||
    (() => {
      tempTranscript = createTempTranscript(params.messages, sessionId);
      return tempTranscript.filePath;
    })();

  const argv = [
    settings.moonPath,
    "--json",
    "--allow-out-of-bounds",
    "context-engine",
    "--source",
    effectiveSourcePath,
    "--session-id",
    sessionId,
  ];
  if (usedTokens !== null && maxTokens !== null) {
    argv.push(
      "--used-tokens",
      String(usedTokens),
      "--max-tokens",
      String(maxTokens),
    );
  }
  if (params.forceCleanse === true) {
    argv.push("--force-cleanse");
  }
  if (params.replayHasCompactionSummary === true) {
    argv.push("--replay-has-compaction-summary");
  }

  const env = { ...process.env };
  if (isNonEmptyString(settings.moonHome)) {
    env.MOON_HOME = settings.moonHome;
  }

  try {
    let result;
    try {
      result = await api.runtime.system.runCommandWithTimeout(argv, {
        timeoutMs: settings.contextEngineTimeoutMs,
        env,
      });
    } catch (err) {
      const processCwd = resolveProcessCwd();
      throw new Error(
        `moon context-engine launch failed executable=${settings.moonPath} ` +
          `process_cwd=${processCwd ?? "unknown"} cause=${String(err)}`,
      );
    }

    if (result.code !== 0) {
      throw new Error(
        result.stderr.trim() || result.stdout.trim() ||
          `moon context-engine exited with ${String(result.code)}`,
      );
    }

    const report = parseCommandReport(result.stdout);
    if (!report) {
      throw new Error("moon context-engine returned non-JSON output");
    }
    if (report.ok !== true) {
      throw new Error(
        Array.isArray(report.issues) && report.issues.length > 0
          ? report.issues.join("; ")
          : "moon context-engine reported failure",
      );
    }

    const assemblyPath = extractReportDetail(
      report,
      "context_engine.assembly_path=",
    );
    if (!isNonEmptyString(assemblyPath) || !fs.existsSync(assemblyPath)) {
      throw new Error(
        "moon context-engine did not emit a readable assembly artifact",
      );
    }
    const cleanseSummaryPath = extractReportDetail(
      report,
      "context_engine.cleanse_summary_path=",
    );
    const effectiveCleanseSummaryPath =
      isNonEmptyString(cleanseSummaryPath) && cleanseSummaryPath !== "none"
        ? cleanseSummaryPath
        : null;
    const packetPath = extractReportDetail(
      report,
      "context_engine.packet_path=",
    );
    const effectivePacketPath =
      isNonEmptyString(packetPath) && packetPath !== "none" &&
        fs.existsSync(packetPath)
        ? packetPath
        : null;

    return {
      report,
      assemblyPath,
      cleanseSummaryPath: effectiveCleanseSummaryPath,
      cleanseSummaryText: readFileIfExists(effectiveCleanseSummaryPath),
      cleanseReason: extractReportDetail(
        report,
        "context_engine.cleanse_reason=",
      ),
      packetPath: effectivePacketPath,
      packetText: readFileIfExists(effectivePacketPath),
      packetCandidateCount: clampInt(
        extractReportDetail(report, "context_engine.packet_candidate_count="),
        0,
        0,
        100000,
      ),
      packetCacheHit:
        extractReportDetail(report, "context_engine.packet_cache_hit=") ===
          "true",
      packetQuery: extractReportDetail(report, "context_engine.packet_query="),
    };
  } finally {
    tempTranscript?.cleanup();
  }
}

async function runMoonContextSync(api, settings, params) {
  const sessionId = sanitizeSessionId(params.sessionId);
  const sourcePath = isNonEmptyString(params.sourcePath) &&
      fs.existsSync(params.sourcePath.trim())
    ? params.sourcePath.trim()
    : null;
  const usedTokens = Number.isFinite(params.usedTokens)
    ? Math.max(0, Math.floor(params.usedTokens))
    : null;
  const maxTokens = Number.isFinite(params.maxTokens)
    ? Math.max(1, Math.floor(params.maxTokens))
    : null;

  let tempTranscript = null;
  const effectiveSourcePath = sourcePath ||
    (() => {
      tempTranscript = createTempTranscript(params.messages, sessionId);
      return tempTranscript.filePath;
    })();

  const argv = [
    settings.moonPath,
    "--json",
    "--allow-out-of-bounds",
    "context-engine",
    "--source",
    effectiveSourcePath,
    "--session-id",
    sessionId,
    "--sync-only",
  ];
  if (usedTokens !== null && maxTokens !== null) {
    argv.push(
      "--used-tokens",
      String(usedTokens),
      "--max-tokens",
      String(maxTokens),
    );
  }
  if (params.replayHasCompactionSummary === true) {
    argv.push("--replay-has-compaction-summary");
  }

  const env = { ...process.env };
  if (isNonEmptyString(settings.moonHome)) {
    env.MOON_HOME = settings.moonHome;
  }

  try {
    const result = await api.runtime.system.runCommandWithTimeout(argv, {
      timeoutMs: settings.contextEngineTimeoutMs,
      env,
    });

    if (result.code !== 0) {
      throw new Error(
        result.stderr.trim() || result.stdout.trim() ||
          `moon context-engine sync exited with ${String(result.code)}`,
      );
    }

    const report = parseCommandReport(result.stdout);
    if (!report) {
      throw new Error("moon context-engine sync returned non-JSON output");
    }
    if (report.ok !== true) {
      throw new Error(
        Array.isArray(report.issues) && report.issues.length > 0
          ? report.issues.join("; ")
          : "moon context-engine sync reported failure",
      );
    }

    return {
      report,
      syncReason: extractReportDetail(report, "context_engine.sync_reason="),
    };
  } finally {
    tempTranscript?.cleanup();
  }
}

function createMoonContextEngine(api) {
  const sessionFiles = new Map();

  function cacheSessionFile(sessionId, sessionFile) {
    if (isNonEmptyString(sessionId) && isNonEmptyString(sessionFile)) {
      sessionFiles.set(sessionId.trim(), sessionFile.trim());
    }
  }

  function knownSessionFile(sessionId) {
    if (!isNonEmptyString(sessionId)) {
      return null;
    }
    const cached = sessionFiles.get(sessionId.trim());
    if (isNonEmptyString(cached) && fs.existsSync(cached)) {
      return cached;
    }
    return null;
  }

  return {
    info: {
      id: "moon",
      name: "Moon Context Engine",
      version: "1.2.1",
      ownsCompaction: true,
    },
    bootstrap(params) {
      cacheSessionFile(params.sessionId, params.sessionFile);
      return {
        bootstrapped: false,
        reason:
          "moon bootstrap caches the OpenClaw transcript path for later checkpoints",
      };
    },
    ingest() {
      return { ingested: false };
    },
    async assemble(params) {
      const settings = resolveContextEngineSettings(api);
      const usedTokens = estimateMessageTokens(params.messages);
      try {
        const output = await runMoonContextEngine(api, settings, {
          sessionId: params.sessionId,
          sessionKey: params.sessionKey,
          sourcePath: knownSessionFile(params.sessionId),
          messages: params.messages,
          usedTokens,
          maxTokens: params.tokenBudget,
          replayHasCompactionSummary: messagesContainCompactionSummary(
            params.messages,
          ),
        });
        let packetText = cleanMoonPacketText(output.packetText);
        if (
          shouldUseAssemblySubagent(
            settings,
            packetText,
            output.packetCandidateCount,
            extractPromptText(params.messages, params.prompt),
          )
        ) {
          const curated = await maybeRunAssemblySubagent(api, settings, {
            sessionId: params.sessionId,
            sessionKey: params.sessionKey,
            packetText,
            prompt: extractPromptText(params.messages, params.prompt),
          }).catch((err) => {
            logMoonPluginError(
              api,
              `moon assembly curator degraded reason=${String(err)}`,
            );
            return null;
          });
          if (isNonEmptyString(curated)) {
            packetText = curated;
          }
        }
        const messages = injectMoonPacketMessage(params.messages, packetText);
        return {
          messages,
          estimatedTokens: estimateMessageTokens(messages),
        };
      } catch (err) {
        if (!openclawFallbackEnabled(settings)) {
          throw err;
        }
        logMoonPluginError(
          api,
          fallbackReason("assemble-error", String(err)),
        );
        return {
          messages: Array.isArray(params.messages) ? params.messages : [],
          estimatedTokens: usedTokens,
        };
      }
    },
    async afterTurn(params) {
      cacheSessionFile(params.sessionId, params.sessionFile);

      const settings = resolveContextEngineSettings(api);
      if (!settings.syncAfterTurn) {
        return;
      }

      try {
        await runMoonContextSync(api, settings, {
          sessionId: params.sessionId,
          sourcePath: params.sessionFile,
          messages: params.messages,
          usedTokens: estimateMessageTokens(params.messages),
          maxTokens: params.tokenBudget,
          replayHasCompactionSummary: messagesContainCompactionSummary(
            params.messages,
          ),
        });
      } catch (err) {
        if (!openclawFallbackEnabled(settings)) {
          throw err;
        }
        logMoonPluginError(
          api,
          fallbackReason("after-turn-error", String(err)),
        );
      }
    },
    async compact(params) {
      cacheSessionFile(params.sessionId, params.sessionFile);
      const settings = resolveContextEngineSettings(api);

      try {
        const output = await runMoonContextEngine(api, settings, {
          sessionId: params.sessionId,
          sourcePath: params.sessionFile || knownSessionFile(params.sessionId),
          messages: [],
          usedTokens: Number.isFinite(params.currentTokenCount)
            ? params.currentTokenCount
            : null,
          maxTokens: params.tokenBudget,
          forceCleanse: params.force === true,
          replayHasCompactionSummary: true,
        });

        const compacted = appendMoonCompactionEntry(params.sessionFile, {
          tokenBudget: params.tokenBudget,
          tokensBefore: params.currentTokenCount,
          sessionId: params.sessionId,
          cleanseSummaryPath: output.cleanseSummaryPath,
          cleanseSummaryText: output.cleanseSummaryText,
          cleanseReason: output.cleanseReason,
          assemblyPath: output.assemblyPath,
          logError: (message) => logMoonPluginError(api, message),
        });
        if (
          !openclawFallbackEnabled(settings) || compacted.compacted === true
        ) {
          return compacted;
        }
        if (compacted.ok === false) {
          const reason = fallbackReason(
            "compact-error",
            compacted.reason || "moon compact failed",
          );
          logMoonPluginError(api, reason);
          return {
            ok: false,
            compacted: false,
            reason,
          };
        }
        if (
          compacted.ok === true &&
          compacted.compacted === false &&
          settings.compactFallbackOnSkip
        ) {
          const reason = fallbackReason(
            "compact-skip",
            compacted.reason || "moon compact skipped",
          );
          logMoonPluginError(api, reason);
          return {
            ok: false,
            compacted: false,
            reason,
          };
        }
        return compacted;
      } catch (err) {
        if (openclawFallbackEnabled(settings)) {
          const reason = fallbackReason("compact-error", String(err));
          logMoonPluginError(api, reason);
          return {
            ok: false,
            compacted: false,
            reason,
          };
        }
        return {
          ok: false,
          compacted: false,
          reason: String(err),
        };
      }
    },
    dispose() {
      sessionFiles.clear();
    },
  };
}

export default {
  id: "moon",
  register(api) {
    api.registerContextEngine("moon", () => createMoonContextEngine(api));

    api.on("tool_result_persist", (event, ctx) => {
      const pluginCfg = isObject(api && api.pluginConfig)
        ? api.pluginConfig
        : {};
      const toolName = event.toolName || ctx.toolName || "";
      const next = compactToolResultMessage(event.message, toolName, pluginCfg);
      return { message: next };
    });
  },
};

export const __moonTest = {
  appendMoonCompactionEntry,
  buildMoonPacketMessage,
  createMoonContextEngine,
  extractMessageText,
  extractReportDetail,
  injectMoonPacketMessage,
  isRecallHeavyPrompt,
  maybeRunAssemblySubagent,
  messagesContainCompactionSummary,
  parseJsonlEntries,
  parseCommandReport,
  resolveContextEngineSettings,
  serializeMessagesAsJsonl,
  stripFrontMatter,
  logMoonPluginError,
};

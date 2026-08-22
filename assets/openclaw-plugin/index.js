import { spawn } from "node:child_process";

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

const OPENCLAW_CORE_SPECIFIER = "openclaw/plugin-sdk/core";
const REASONING_LEVELS = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "adaptive",
  "max",
  "ultra",
];

function clampInteger(value, fallback, minimum, maximum) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return fallback;
  }
  return Math.min(maximum, Math.max(minimum, Math.floor(number)));
}

function clampNumber(value, fallback, minimum, maximum) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return fallback;
  }
  return Math.min(maximum, Math.max(minimum, number));
}

function visibleText(value, depth = 0) {
  if (depth > 5 || value === null || value === undefined) {
    return "";
  }
  if (typeof value === "string") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => visibleText(item, depth + 1)).filter(Boolean)
      .join("\n");
  }
  if (!isObject(value)) {
    return "";
  }
  return ["text", "input_text", "summary", "content"]
    .map((key) => visibleText(value[key], depth + 1))
    .filter(Boolean)
    .join("\n");
}

function queryFromParams(params) {
  if (nonEmptyString(params?.prompt)) {
    return params.prompt.trim().slice(0, 1_000);
  }
  const messages = Array.isArray(params?.messages) ? params.messages : [];
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message?.role !== "user") {
      continue;
    }
    const text = visibleText(message.content).trim();
    if (text) {
      return text.slice(0, 1_000);
    }
  }
  return "";
}

function resolvePath(api, value) {
  if (!nonEmptyString(value)) {
    return null;
  }
  if (typeof api?.resolvePath === "function") {
    const resolved = api.resolvePath(value.trim());
    if (nonEmptyString(resolved)) {
      return resolved.trim();
    }
  }
  return value.trim();
}

function resolveSettings(api) {
  const config = isObject(api?.pluginConfig) ? api.pluginConfig : {};
  const openClawModel = api?.config?.agents?.defaults?.model;
  const openClawPrimary = nonEmptyString(openClawModel)
    ? openClawModel.trim()
    : nonEmptyString(openClawModel?.primary)
    ? openClawModel.primary.trim()
    : null;
  const openClawFallback = Array.isArray(openClawModel?.fallbacks)
    ? openClawModel.fallbacks.find(nonEmptyString)?.trim() ?? null
    : null;
  const mode = ["lexical", "semantic", "hybrid"].includes(config.mode)
    ? config.mode
    : "lexical";
  const primaryModel = nonEmptyString(config.primaryModel)
    ? config.primaryModel.trim()
    : openClawPrimary;
  const fallbackModel = nonEmptyString(config.fallbackModel)
    ? config.fallbackModel.trim()
    : openClawFallback;
  return {
    moonPath: resolvePath(api, config.moonPath) || "moon",
    moonHome: resolvePath(api, config.moonHome),
    mode,
    primaryModel,
    fallbackModel: fallbackModel === primaryModel ? null : fallbackModel,
    primaryReasoning: REASONING_LEVELS.includes(config.primaryReasoning)
      ? config.primaryReasoning
      : "off",
    fallbackReasoning: REASONING_LEVELS.includes(config.fallbackReasoning)
      ? config.fallbackReasoning
      : "off",
    modelTimeoutMs: clampInteger(
      config.modelTimeoutMs,
      120_000,
      1_000,
      300_000,
    ),
    dimensions: clampInteger(config.dimensions, 384, 1, 4096),
    scope: nonEmptyString(config.scope) ? config.scope.trim() : null,
    limit: clampInteger(config.limit, 8, 1, 32),
    maxChars: clampInteger(config.maxChars, 3_500, 512, 32_000),
    evidencePerMemory: clampInteger(config.evidencePerMemory, 2, 0, 8),
    timeoutMs: clampInteger(config.timeoutMs, 10_000, 1_000, 300_000),
    failOpen: config.failOpen !== false,
    learningEnabled: config.learningEnabled !== false,
    learningTimeoutMs: clampInteger(
      config.learningTimeoutMs,
      120_000,
      1_000,
      300_000,
    ),
    learningScope: nonEmptyString(config.learningScope)
      ? config.learningScope.trim()
      : (nonEmptyString(config.scope) ? config.scope.trim() : "global"),
    learningMaxMemories: clampInteger(
      config.learningMaxMemories,
      3,
      1,
      8,
    ),
    learningMinConfidence: clampNumber(
      config.learningMinConfidence,
      0.78,
      0,
      1,
    ),
    learningMinImportance: clampNumber(
      config.learningMinImportance,
      0.55,
      0,
      1,
    ),
    embeddingEnabled: config.embeddingEnabled !== false,
    embeddingBatchSize: clampInteger(
      config.embeddingBatchSize,
      64,
      1,
      1_000,
    ),
    embeddingTimeoutMs: clampInteger(
      config.embeddingTimeoutMs,
      120_000,
      5_000,
      300_000,
    ),
  };
}

function contextArguments(settings, query) {
  const argv = [settings.moonPath];
  if (settings.moonHome) {
    argv.push("--home", settings.moonHome);
  }
  argv.push("--dimensions", String(settings.dimensions));
  argv.push(
    "context",
    "--query",
    query,
    "--mode",
    settings.mode,
    "--limit",
    String(settings.limit),
    "--max-chars",
    String(settings.maxChars),
    "--evidence-per-memory",
    String(settings.evidencePerMemory),
  );
  if (settings.scope) {
    argv.push("--scope", settings.scope);
  }
  if (settings.mode !== "lexical") {
    argv.push("--provider", "local");
  }
  argv.push("--adapter", "--json");
  return argv;
}

function metricInjectionArguments(settings, requestId, injected) {
  const argv = baseMoonArguments(settings, true);
  argv.push("metrics", "mark-injection", "--request", requestId);
  if (injected) {
    argv.push("--injected");
  }
  return argv;
}

function runtimeMetricArguments(settings, metric) {
  const argv = baseMoonArguments(settings, true);
  argv.push(
    "metrics",
    "record-runtime",
    "--kind",
    metric.event_kind,
    "--status",
    metric.status,
    "--duration-us",
    String(metric.duration_us),
  );
  for (
    const [field, flag] of [
      ["evidence_changed", "--evidence-changed"],
      ["learning_eligible", "--learning-eligible"],
      ["compacted", "--compacted"],
    ]
  ) {
    if (metric[field] === true) {
      argv.push(flag);
    }
  }
  for (
    const [field, flag] of [
      ["proposed_memories", "--proposed-memories"],
      ["accepted_memories", "--accepted-memories"],
      ["tokens_before", "--tokens-before"],
      ["tokens_after", "--tokens-after"],
    ]
  ) {
    if (Number.isSafeInteger(metric[field]) && metric[field] >= 0) {
      argv.push(flag, String(metric[field]));
    }
  }
  return argv;
}

function baseMoonArguments(settings, json = false) {
  const argv = [settings.moonPath];
  if (settings.moonHome) {
    argv.push("--home", settings.moonHome);
  }
  argv.push("--dimensions", String(settings.dimensions));
  if (json) {
    argv.push("--json");
  }
  return argv;
}

function recordArguments(settings, turn) {
  return [
    ...baseMoonArguments(settings, true),
    "record",
    "--session-id",
    turn.evidenceSessionId,
    "--scope",
    settings.learningScope,
    "--completed-at-ms",
    String(turn.completedAtMs),
    "--metadata-json",
    JSON.stringify(turn.metadata),
  ];
}

function distillBatchArguments(settings, evidenceSessionId) {
  return [
    ...baseMoonArguments(settings, true),
    "distill-batch",
    "--session-id",
    evidenceSessionId,
    "--scope",
    settings.learningScope,
  ];
}

function structuredContextArguments(settings, query) {
  const argv = baseMoonArguments(settings, true);
  argv.push(
    "context",
    "--query",
    query,
    "--mode",
    settings.mode,
    "--limit",
    String(Math.min(settings.limit, 4)),
    "--max-chars",
    String(Math.min(settings.maxChars, 3_500)),
    "--evidence-per-memory",
    "1",
  );
  if (settings.scope) {
    argv.push("--scope", settings.scope);
  }
  if (settings.mode !== "lexical") {
    argv.push("--provider", "local");
  }
  return argv;
}

function stdioWorkerArguments(settings) {
  const argv = baseMoonArguments(settings, false);
  argv.push("serve", "--provider", "local");
  return argv;
}

function parseModelReference(reference) {
  if (!nonEmptyString(reference)) {
    throw new Error("Moon requires an OpenClaw primary model");
  }
  const separator = reference.indexOf("/");
  if (separator <= 0 || separator === reference.length - 1) {
    throw new Error("Moon model references must use provider/model format");
  }
  return {
    provider: reference.slice(0, separator),
    model: reference.slice(separator + 1),
  };
}

async function runOpenClawModel(api, settings, prompt, params = {}) {
  const modelRef = params.modelRef;
  const reasoning = params.reasoning;
  const route = params.route;
  const selected = parseModelReference(modelRef);
  const runner = api?.runtime?.agent?.runEmbeddedPiAgent;
  if (typeof runner !== "function") {
    throw new Error("OpenClaw model runtime unavailable");
  }
  if (!nonEmptyString(params.sessionFile)) {
    throw new Error("OpenClaw model request requires an isolated session file");
  }
  const id = `moon-model-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  const timeoutMs = params.timeoutMs ?? settings.modelTimeoutMs;
  const result = await runner({
    sessionId: id,
    sessionKey: nonEmptyString(params.sessionKey)
      ? `${params.sessionKey}:moon-model`
      : undefined,
    sessionFile: params.sessionFile,
    workspaceDir: nonEmptyString(params.workspaceDir)
      ? params.workspaceDir
      : resolvePath(api, "."),
    config: isObject(api?.config) ? api.config : {},
    prompt,
    provider: selected.provider,
    model: selected.model,
    timeoutMs,
    runId: id,
    trigger: "manual",
    toolsAllow: [],
    disableMessageTool: true,
    disableTools: true,
    bootstrapContextMode: "lightweight",
    verboseLevel: "off",
    reasoningLevel: reasoning,
    silentExpected: true,
  });
  const output = (result?.payloads ?? [])
    .map((payload) => payload?.text?.trim() ?? "")
    .filter(Boolean)
    .join("\n")
    .trim();
  if (!nonEmptyString(output)) {
    throw new Error("OpenClaw model returned an empty response");
  }
  return {
    modelRoute: route,
    model: modelRef,
    reasoning,
    output,
    validatedOutput: null,
  };
}

async function runModelWithFallback(api, settings, prompt, params = {}) {
  const routes = [{
    route: "primary",
    modelRef: settings.primaryModel,
    reasoning: settings.primaryReasoning,
  }];
  if (settings.fallbackModel) {
    routes.push({
      route: "fallback",
      modelRef: settings.fallbackModel,
      reasoning: settings.fallbackReasoning,
    });
  }
  for (const route of routes) {
    try {
      const outcome = await runOpenClawModel(api, settings, prompt, {
        ...params,
        ...route,
      });
      if (typeof params.validateOutput === "function") {
        outcome.validatedOutput = params.validateOutput(outcome.output);
      }
      return outcome;
    } catch {
      // Provider diagnostics can contain credentials or remote response bodies.
    }
  }
  throw new Error(
    settings.fallbackModel
      ? "OpenClaw primary and fallback model requests failed"
      : "OpenClaw primary model request failed",
  );
}

function packetMessage(packet) {
  return {
    role: "assistant",
    timestamp: Date.now(),
    content: [{ type: "text", text: packet }],
  };
}

function injectPacket(messages, packet) {
  const next = Array.isArray(messages) ? [...messages] : [];
  const memoryMessage = packetMessage(packet);
  if (next.length > 0 && next[next.length - 1]?.role === "user") {
    next.splice(next.length - 1, 0, memoryMessage);
    return next;
  }
  return [memoryMessage, ...next];
}

function estimateTokens(messages) {
  return Math.ceil(visibleText(messages).length / 4);
}

function isTrivialQuery(query) {
  const normalized = String(query ?? "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s]/gu, " ")
    .replace(/\s+/g, " ")
    .trim();
  return /^(hi|hello|hey|thanks|thank you|ok|okay|good morning|good evening|hi lilac|hello lilac)$/
    .test(
      normalized,
    );
}

function stableHash(value) {
  let left = 0x811c9dc5;
  let right = 0x9e3779b9;
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    left = Math.imul(left ^ code, 0x01000193);
    right = Math.imul(right ^ (code + 0x9e37), 0x85ebca6b);
  }
  return `${(left >>> 0).toString(16).padStart(8, "0")}${
    (right >>> 0).toString(16).padStart(8, "0")
  }`;
}

function boundedText(value, maximum) {
  const text = String(value ?? "").trim();
  return text.length <= maximum ? text : `${text.slice(0, maximum - 1)}…`;
}

function unicodeLength(value) {
  return Array.from(String(value ?? "")).length;
}

function completedTurnFromParams(params) {
  if (params?.isHeartbeat === true) {
    return null;
  }
  const messages = Array.isArray(params?.messages) ? params.messages : [];
  const start = clampInteger(
    params?.prePromptMessageCount,
    0,
    0,
    messages.length,
  );
  const turnMessages = messages.slice(start);
  const user = turnMessages.find((message) => message?.role === "user");
  const assistants = turnMessages.filter((message) =>
    message?.role === "assistant"
  );
  const userText = boundedText(visibleText(user?.content), 64_000);
  const assistantText = boundedText(
    visibleText(assistants.at(-1)?.content),
    128_000,
  );
  if (!nonEmptyString(userText) || !nonEmptyString(assistantText)) {
    return null;
  }
  const transcript = `User:\n${userText}\n\nAssistant:\n${assistantText}`;
  const sessionId = nonEmptyString(params?.sessionId)
    ? params.sessionId.trim()
    : "unknown";
  const fingerprint = stableHash(`${sessionId}\n${transcript}`);
  const safeSessionId = sessionId
    .replace(/[^A-Za-z0-9._:/@-]/g, "-")
    .slice(0, 210);
  const completedAtMs = assistants.at(-1)?.timestamp ??
    user?.timestamp ??
    Date.now();
  return {
    userText,
    assistantText,
    transcript,
    title: boundedText(userText.replace(/\s+/g, " "), 120),
    evidenceSessionId: `openclaw:${safeSessionId}:turn:${fingerprint}`,
    completedAtMs: Number.isFinite(Number(completedAtMs))
      ? Math.floor(Number(completedAtMs))
      : Date.now(),
    metadata: {
      source: "openclaw",
      parent_session_id: sessionId,
      session_key: nonEmptyString(params?.sessionKey)
        ? params.sessionKey.trim()
        : null,
      turn_fingerprint: fingerprint,
      message_count: turnMessages.length,
    },
  };
}

function isLearningCandidate(turn) {
  if (!turn || isTrivialQuery(turn.userText)) {
    return false;
  }
  const durableCue =
    /\b(remember|prefer|preference|decided|decision|always|never|correct(?:ion)?|actually|instead|no longer|changed|update|workflow|my name|project|architecture)\b/i;
  return durableCue.test(turn.userText) ||
    turn.userText.length >= 60 ||
    turn.assistantText.length >= 180;
}

function correctionRequested(userText) {
  return /\b(correct(?:ion)?|actually|instead|no longer|changed|update|wrong|replace|supersede)\b/i
    .test(userText);
}

function parseJsonObject(value) {
  const text = String(value ?? "").trim();
  const first = text.indexOf("{");
  const last = text.lastIndexOf("}");
  if (first < 0 || last <= first) {
    throw new Error("learning model returned no JSON object");
  }
  return JSON.parse(text.slice(first, last + 1));
}

function evidenceSupportsContent(content, evidenceQuote) {
  const normalizedQuote = evidenceQuote.toLowerCase();
  const numbers = [...content.matchAll(/\d+(?:[.:]\d+)?/g)]
    .map((match) => match[0]);
  if (numbers.some((number) => !normalizedQuote.includes(number))) {
    return false;
  }
  const stopWords = new Set([
    "about",
    "after",
    "also",
    "and",
    "are",
    "for",
    "from",
    "has",
    "have",
    "into",
    "that",
    "the",
    "their",
    "this",
    "user",
    "using",
    "with",
  ]);
  const terms = content
    .toLowerCase()
    .split(/[^\p{L}\p{N}._+-]+/u)
    .filter((term) => term.length >= 4 && !stopWords.has(term));
  const uniqueTerms = [...new Set(terms)];
  if (uniqueTerms.length === 0) {
    return true;
  }
  const matched =
    uniqueTerms.filter((term) => normalizedQuote.includes(term)).length;
  return matched / uniqueTerms.length >= 0.5;
}

function normalizeProposal(
  raw,
  turn,
  settings,
  activeMemoryIds,
  activeMemoryKeys = new Set(),
) {
  if (!isObject(raw)) {
    return null;
  }
  const canonicalKey = nonEmptyString(raw.canonical_key)
    ? raw.canonical_key.trim()
    : "";
  const content = nonEmptyString(raw.content) ? raw.content.trim() : "";
  const evidenceQuote = nonEmptyString(raw.evidence_quote)
    ? raw.evidence_quote.trim()
    : "";
  const title = nonEmptyString(raw.title)
    ? boundedText(raw.title.trim(), 160)
    : "Learned memory";
  const kind = nonEmptyString(raw.kind)
    ? raw.kind.trim().toLowerCase()
    : "fact";
  const importance = clampNumber(raw.importance, 0, 0, 1);
  const confidence = clampNumber(raw.confidence, 0, 0, 1);
  if (
    !/^[A-Za-z0-9._:/-]{2,256}$/.test(canonicalKey) ||
    !["fact", "preference", "decision", "workflow", "relationship", "summary"]
      .includes(kind) ||
    !content ||
    content.length > 2_000 ||
    !evidenceQuote ||
    evidenceQuote.length > 8_192 ||
    !turn.transcript.includes(evidenceQuote) ||
    !evidenceSupportsContent(content, evidenceQuote) ||
    importance < settings.learningMinImportance ||
    confidence < settings.learningMinConfidence
  ) {
    return null;
  }
  if (
    activeMemoryKeys.has(canonicalKey) &&
    !turn.userText.includes(evidenceQuote)
  ) {
    return null;
  }
  const requestedSupersedes = Number(raw.supersedes_document_id);
  const supersedesDocumentId = Number.isSafeInteger(requestedSupersedes) &&
      activeMemoryIds.has(requestedSupersedes) &&
      correctionRequested(turn.userText)
    ? requestedSupersedes
    : null;
  return {
    canonicalKey,
    content,
    evidenceQuote,
    title,
    kind,
    importance,
    confidence,
    supersedesDocumentId,
  };
}

function learningPrompt(turn, activeMemories, settings) {
  const current = activeMemories.map((memory) => ({
    document_id: memory.document_id,
    canonical_key: memory.canonical_key,
    kind: memory.memory_kind,
    content: boundedText(memory.content, 1_200),
  }));
  return [
    "You are Moon's conservative memory curator.",
    "Return exactly one JSON object and no markdown.",
    `Extract at most ${settings.learningMaxMemories} durable memories from the completed turn.`,
    "Keep only stable user preferences, confirmed facts, decisions, corrections, relationships, or reusable successful workflows.",
    "A concrete, source-attributed or tool-verified result in the final answer may be retained when it directly answers the request, such as an exact calculation or the successful method used to produce it.",
    "Named-entity reference data is durable: when the user asks about a named person, project, or object and the final answer establishes exact reusable facts, retain a concise entity memory; do not dismiss it as merely task-specific.",
    "Prioritize, in order: explicit user preferences or decisions, exact named-entity reference data, then reusable workflows.",
    "Do not retain greetings, temporary task logistics, guesses, secrets, credentials, private keys, tokens, raw tool chatter, unsupported interpretations, or ordinary assistant prose.",
    "Each memory must be self-contained and useful in a future conversation.",
    "evidence_quote must be one exact contiguous substring from the completed turn and must support every factual detail in the memory.",
    "Every number, date, time, coordinate, name, and calculated value in memory content must appear in evidence_quote. Use a longer contiguous quote or make the memory narrower.",
    "Use a stable lowercase canonical_key with namespaces, for example user:preference:response-style.",
    "Set confidence below 0.78 when uncertain; those proposals will be discarded.",
    "Set importance below 0.55 for minor details; those proposals will be discarded.",
    "Only set supersedes_document_id when the user explicitly corrects or changes one of the supplied active memories.",
    "Do not re-extract an active memory merely because the assistant recalled or restated it. Confirm an active memory only when the user explicitly reconfirms it, and use the user's words as evidence_quote.",
    'Schema: {"eligible":boolean,"memories":[{"canonical_key":string,"kind":"fact|preference|decision|workflow|relationship|summary","title":string,"content":string,"evidence_quote":string,"importance":number,"confidence":number,"supersedes_document_id":number|null}]}',
    `Active relevant memories: ${JSON.stringify(current)}`,
    `Completed turn:\n${turn.transcript}`,
  ].join("\n\n");
}

function logError(api, message) {
  const logger = api?.logger ?? api?.runtime?.logger ?? api?.log;
  if (typeof logger?.error === "function") {
    logger.error(message);
    return;
  }
  console.error(`[moon plugin] ${message}`);
}

function logInfo(api, message) {
  const logger = api?.logger ?? api?.runtime?.logger ?? api?.log;
  if (typeof logger?.info === "function") {
    logger.info(message);
  }
}

async function runMoonCommand(api, argv, timeoutMs, input) {
  const result = await api.runtime.system.runCommandWithTimeout(argv, {
    timeoutMs,
    ...(input === undefined ? {} : { input }),
  });
  if (result.code !== 0) {
    throw new Error(
      result.stderr?.trim() || `moon exited with ${result.code}`,
    );
  }
  return result.stdout?.trim() ?? "";
}

class MoonStdioClient {
  constructor(settings, spawnProcess = spawn) {
    this.settings = settings;
    this.spawnProcess = spawnProcess;
    this.child = null;
    this.buffer = "";
    this.nextId = 1;
    this.pending = new Map();
  }

  start() {
    if (this.child) {
      return;
    }
    const argv = stdioWorkerArguments(this.settings);
    const child = this.spawnProcess(argv[0], argv.slice(1), {
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child = child;
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => this.onData(chunk));
    child.on("error", (error) => this.onExit(error));
    child.on("exit", (code, signal) => {
      this.onExit(
        new Error(
          `moon worker exited code=${String(code)} signal=${String(signal)}`,
        ),
      );
    });
  }

  onData(chunk) {
    this.buffer += chunk;
    while (true) {
      const newline = this.buffer.indexOf("\n");
      if (newline < 0) {
        return;
      }
      const line = this.buffer.slice(0, newline);
      this.buffer = this.buffer.slice(newline + 1);
      let response;
      try {
        response = JSON.parse(line);
      } catch {
        this.onExit(new Error("moon worker returned invalid JSON"));
        return;
      }
      const pending = this.pending.get(response?.id);
      if (!pending) {
        continue;
      }
      this.pending.delete(response.id);
      clearTimeout(pending.timer);
      if (response.ok === true) {
        pending.resolve(response.result);
      } else {
        pending.reject(
          new Error(
            nonEmptyString(response.error)
              ? response.error
              : "moon worker request failed",
          ),
        );
      }
    }
  }

  onExit(error) {
    const child = this.child;
    this.child = null;
    this.buffer = "";
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
    if (child && !child.killed) {
      child.kill();
    }
  }

  request(operation, timeoutMs) {
    this.start();
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          new Error(`moon worker request timed out after ${timeoutMs}ms`),
        );
        this.dispose();
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(`${JSON.stringify({ id, ...operation })}\n`);
    });
  }

  dispose() {
    const child = this.child;
    if (!child) {
      return Promise.resolve();
    }
    const closed = new Promise((resolve) => child.once("close", resolve));
    this.onExit(new Error("moon worker disposed"));
    return closed;
  }
}

async function delegateCompaction(
  params,
  loadCore = () => import(OPENCLAW_CORE_SPECIFIER),
) {
  const harnessId = nonEmptyString(params?.runtimeContext?.agentHarnessId)
    ? params.runtimeContext.agentHarnessId.trim().toLowerCase()
    : null;
  if (harnessId !== "openclaw") {
    return {
      ok: true,
      compacted: false,
      reason: harnessId
        ? `${harnessId} owns native automatic compaction; unsafe generic fallback disabled`
        : "runtime compaction owner is unknown; unsafe generic fallback disabled",
    };
  }
  const core = await loadCore();
  if (typeof core?.delegateCompactionToRuntime !== "function") {
    throw new Error("OpenClaw compaction delegate is unavailable");
  }
  return await core.delegateCompactionToRuntime(params);
}

async function retrieveStructuredContext(api, settings, query, worker) {
  const packet = settings.mode === "lexical"
    ? JSON.parse(
      await runMoonCommand(
        api,
        structuredContextArguments(settings, query),
        settings.timeoutMs,
      ),
    )
    : await worker.request(
      contextWorkerRequest(settings, query, true),
      settings.embeddingTimeoutMs,
    );
  if (!Array.isArray(packet?.memories) || !Array.isArray(packet?.references)) {
    throw new Error("moon structured context returned invalid JSON");
  }
  return packet;
}

async function recordCompletedTurn(api, settings, turn) {
  const output = await runMoonCommand(
    api,
    recordArguments(settings, turn),
    settings.timeoutMs,
    turn.transcript,
  );
  const outcome = JSON.parse(output);
  if (
    outcome?.session_id !== turn.evidenceSessionId ||
    typeof outcome?.changed !== "boolean"
  ) {
    throw new Error("moon record returned an invalid outcome");
  }
  return outcome;
}

async function distillCompletedTurn(api, settings, params, turn, worker) {
  const packet = await retrieveStructuredContext(
    api,
    settings,
    turn.userText,
    worker,
  );
  const activeMemories = packet.memories.slice(0, 4);
  const activeMemoryIds = new Set(
    activeMemories
      .map((memory) => Number(memory?.document_id))
      .filter(Number.isSafeInteger),
  );
  const activeMemoryKeys = new Set(
    activeMemories
      .map((memory) => memory?.canonical_key)
      .filter(nonEmptyString),
  );
  const model = await runModelWithFallback(
    api,
    settings,
    learningPrompt(turn, activeMemories, settings),
    {
      sessionFile: params.sessionFile,
      sessionKey: params.sessionKey,
      workspaceDir: params.runtimeSettings?.executionHost?.workspaceDir,
      timeoutMs: settings.learningTimeoutMs,
      validateOutput: parseJsonObject,
    },
  );
  const result = model.validatedOutput;
  if (result?.eligible !== true || !Array.isArray(result?.memories)) {
    return { proposed: 0, distilled: 0, modelRoute: model.modelRoute };
  }
  const proposals = result.memories
    .slice(0, settings.learningMaxMemories)
    .map((proposal) =>
      normalizeProposal(
        proposal,
        turn,
        settings,
        activeMemoryIds,
        activeMemoryKeys,
      )
    )
    .filter(Boolean);
  if (proposals.length > 0) {
    const payload = JSON.stringify(proposals.map((proposal) => ({
      canonical_key: proposal.canonicalKey,
      kind: proposal.kind,
      title: proposal.title,
      content: proposal.content,
      evidence_quote: proposal.evidenceQuote,
      importance: proposal.importance,
      confidence: proposal.confidence,
      pinned: false,
      supersedes_document_id: proposal.supersedesDocumentId,
    })));
    const output = await runMoonCommand(
      api,
      distillBatchArguments(settings, turn.evidenceSessionId),
      settings.timeoutMs,
      payload,
    );
    const outcome = JSON.parse(output);
    if (outcome?.distilled !== proposals.length) {
      throw new Error("moon distill-batch returned an invalid outcome");
    }
  }
  return {
    proposed: proposals.length,
    distilled: proposals.length,
    modelRoute: model.modelRoute,
  };
}

function contextWorkerRequest(settings, query, structured, observe = false) {
  return {
    op: "context",
    query,
    mode: settings.mode,
    limit: structured ? Math.min(settings.limit, 4) : settings.limit,
    scope: settings.scope,
    max_chars: structured
      ? Math.min(settings.maxChars, 3_500)
      : settings.maxChars,
    evidence_per_memory: structured ? 1 : settings.evidencePerMemory,
    structured,
    observe,
  };
}

async function retrievePacket(api, settings, query, worker) {
  let observation;
  if (settings.mode !== "lexical") {
    observation = await worker.request(
      contextWorkerRequest(settings, query, false, true),
      settings.embeddingTimeoutMs,
    );
  } else {
    const result = await api.runtime.system.runCommandWithTimeout(
      contextArguments(settings, query),
      {
        timeoutMs: settings.timeoutMs,
      },
    );
    if (result.code !== 0) {
      throw new Error(
        result.stderr?.trim() || `moon context exited with ${result.code}`,
      );
    }
    try {
      observation = JSON.parse(result.stdout ?? "");
    } catch {
      throw new Error("moon context returned an invalid metrics envelope");
    }
  }
  if (!isObject(observation)) {
    throw new Error("moon context returned an invalid metrics envelope");
  }
  const requestId = observation.request_id;
  if (
    requestId !== null &&
    !(typeof requestId === "string" && /^[0-9a-f]{32}$/.test(requestId))
  ) {
    throw new Error("moon context returned an invalid metric request id");
  }
  const packet = observation.packet;
  if (packet !== null && !nonEmptyString(packet)) {
    throw new Error("moon context returned an invalid context packet");
  }
  if (packet !== null && !packet.startsWith("# Moon Context")) {
    throw new Error("moon context returned an invalid packet");
  }
  if (packet !== null && unicodeLength(packet) > settings.maxChars) {
    throw new Error(
      "moon context exceeded the configured character limit",
    );
  }
  for (const field of ["memory_count", "reference_count", "packet_chars"]) {
    if (!Number.isSafeInteger(observation[field]) || observation[field] < 0) {
      throw new Error("moon context returned invalid metric counts");
    }
  }
  if (typeof observation.truncated !== "boolean") {
    throw new Error("moon context returned an invalid truncation metric");
  }
  return {
    requestId,
    packet,
    memoryCount: observation.memory_count,
    referenceCount: observation.reference_count,
    packetChars: observation.packet_chars,
    truncated: observation.truncated,
  };
}

async function markContextInjection(
  api,
  settings,
  worker,
  requestId,
  injected,
) {
  if (!requestId) {
    logError(api, "context metrics degraded: request was not recorded");
    return;
  }
  try {
    if (settings.mode === "lexical") {
      await runMoonCommand(
        api,
        metricInjectionArguments(settings, requestId, injected),
        settings.timeoutMs,
      );
    } else {
      const result = await worker.request(
        { op: "context_injection", request_id: requestId, injected },
        settings.embeddingTimeoutMs,
      );
      if (result?.updated !== true) {
        throw new Error("moon worker returned an invalid metrics update");
      }
    }
  } catch (error) {
    logError(api, `context metrics degraded: ${String(error)}`);
  }
}

async function recordRuntimeMetric(api, settings, worker, metric) {
  try {
    let result;
    if (worker) {
      result = await worker.request(
        { op: "runtime_metric", ...metric },
        settings.embeddingTimeoutMs,
      );
    } else {
      result = JSON.parse(
        await runMoonCommand(
          api,
          runtimeMetricArguments(settings, metric),
          settings.timeoutMs,
        ),
      );
    }
    const eventId = result?.event_id;
    if (!(typeof eventId === "string" && /^[0-9a-f]{32}$/.test(eventId))) {
      throw new Error("moon returned an invalid runtime metric event id");
    }
  } catch (error) {
    logError(api, `runtime metrics degraded: ${String(error)}`);
  }
}

function elapsedMicroseconds(started) {
  return Math.max(0, Math.round((performance.now() - started) * 1_000));
}

async function observeCompaction(
  api,
  settings,
  params,
  worker,
  compact = delegateCompaction,
) {
  const started = performance.now();
  try {
    const outcome = await compact(params);
    const tokensBefore = Number(outcome?.result?.tokensBefore);
    const tokensAfter = Number(outcome?.result?.tokensAfter);
    await recordRuntimeMetric(api, settings, worker, {
      event_kind: "compaction",
      status: outcome?.compacted === true ? "ok" : "skipped",
      duration_us: elapsedMicroseconds(started),
      compacted: outcome?.compacted === true,
      tokens_before: Number.isSafeInteger(tokensBefore) && tokensBefore >= 0
        ? tokensBefore
        : null,
      tokens_after: Number.isSafeInteger(tokensAfter) && tokensAfter >= 0
        ? tokensAfter
        : null,
    });
    return outcome;
  } catch (error) {
    await recordRuntimeMetric(api, settings, worker, {
      event_kind: "compaction",
      status: "error",
      duration_us: elapsedMicroseconds(started),
      compacted: false,
    });
    throw error;
  }
}

async function drainEmbeddingQueue(api, settings, worker) {
  if (!settings.embeddingEnabled) {
    return;
  }
  try {
    const report = await worker.request(
      { op: "embed", limit: settings.embeddingBatchSize },
      settings.embeddingTimeoutMs,
    );
    if (
      !isObject(report) ||
      !Number.isSafeInteger(report.embedded) ||
      !Number.isSafeInteger(report.remaining)
    ) {
      throw new Error("moon worker returned an invalid embedding report");
    }
    logInfo(
      api,
      `moon embeddings embedded=${report.embedded} remaining=${report.remaining}`,
    );
  } catch (error) {
    logError(api, `embedding degraded: ${String(error)}`);
  }
}

function createMoonContextEngine(api, sharedWorkerState = null) {
  let stdioClient = null;
  function workerFor(settings) {
    if (sharedWorkerState) {
      if (!sharedWorkerState.client) {
        sharedWorkerState.client = new MoonStdioClient(settings);
      }
      return sharedWorkerState.client;
    }
    if (!stdioClient) {
      stdioClient = new MoonStdioClient(settings);
    }
    return stdioClient;
  }
  return {
    info: {
      id: "moon",
      name: "Moon SQLite Context Engine",
      version: "2.4.1",
      ownsCompaction: false,
    },
    bootstrap() {
      return {
        bootstrapped: false,
        reason: "Moon retrieves SQLite context; OpenClaw owns transcripts",
      };
    },
    ingest() {
      return { ingested: false };
    },
    async assemble(params) {
      const messages = Array.isArray(params?.messages) ? params.messages : [];
      const query = queryFromParams(params);
      if (!query || isTrivialQuery(query)) {
        return { messages, estimatedTokens: estimateTokens(messages) };
      }
      const settings = resolveSettings(api);
      try {
        const worker = settings.mode === "lexical" ? null : workerFor(settings);
        const observation = await retrievePacket(
          api,
          settings,
          query,
          worker,
        );
        if (!observation.packet) {
          await markContextInjection(
            api,
            settings,
            worker,
            observation.requestId,
            false,
          );
          logInfo(
            api,
            `moon context request=${
              observation.requestId ?? "unrecorded"
            } injected=false memories=0 references=0 chars=${observation.packetChars} truncated=${observation.truncated}`,
          );
          return { messages, estimatedTokens: estimateTokens(messages) };
        }
        const injected = injectPacket(messages, observation.packet);
        await markContextInjection(
          api,
          settings,
          worker,
          observation.requestId,
          true,
        );
        logInfo(
          api,
          `moon context request=${
            observation.requestId ?? "unrecorded"
          } injected=true memories=${observation.memoryCount} references=${observation.referenceCount} chars=${observation.packetChars} truncated=${observation.truncated}`,
        );
        return {
          messages: injected,
          estimatedTokens: estimateTokens(injected),
        };
      } catch (error) {
        logError(api, `context retrieval degraded: ${String(error)}`);
        if (!settings.failOpen) {
          throw error;
        }
        return { messages, estimatedTokens: estimateTokens(messages) };
      }
    },
    async afterTurn(params) {
      const settings = resolveSettings(api);
      const turn = completedTurnFromParams(params);
      if (!turn) {
        return;
      }
      const learningStarted = performance.now();
      const learningMetric = {
        event_kind: "learning",
        status: settings.learningEnabled ? "ok" : "skipped",
        duration_us: 0,
        evidence_changed: false,
        learning_eligible: isLearningCandidate(turn),
        proposed_memories: 0,
        accepted_memories: 0,
      };
      if (settings.learningEnabled) {
        try {
          const recorded = await recordCompletedTurn(api, settings, turn);
          learningMetric.evidence_changed = recorded.changed;
          if (!recorded.changed || !isLearningCandidate(turn)) {
            logInfo(
              api,
              `moon learning evidence=${
                recorded.changed ? "recorded" : "duplicate"
              } distilled=0`,
            );
          } else {
            const outcome = await distillCompletedTurn(
              api,
              settings,
              params,
              turn,
              settings.mode === "lexical" ? null : workerFor(settings),
            );
            learningMetric.proposed_memories = outcome.proposed;
            learningMetric.accepted_memories = outcome.distilled;
            logInfo(
              api,
              `moon learning evidence=recorded proposed=${outcome.proposed} distilled=${outcome.distilled} model_route=${outcome.modelRoute}`,
            );
          }
        } catch (error) {
          learningMetric.status = "error";
          logError(api, `learning degraded: ${String(error)}`);
          if (!settings.failOpen) {
            learningMetric.duration_us = elapsedMicroseconds(learningStarted);
            await recordRuntimeMetric(
              api,
              settings,
              settings.mode === "lexical" ? null : workerFor(settings),
              learningMetric,
            );
            throw error;
          }
        }
      }
      learningMetric.duration_us = elapsedMicroseconds(learningStarted);
      await recordRuntimeMetric(
        api,
        settings,
        settings.mode === "lexical" ? null : workerFor(settings),
        learningMetric,
      );
      await drainEmbeddingQueue(api, settings, workerFor(settings));
    },
    async compact(params) {
      const settings = resolveSettings(api);
      return await observeCompaction(
        api,
        settings,
        params,
        settings.mode === "lexical" ? null : workerFor(settings),
      );
    },
    dispose() {
      if (sharedWorkerState) {
        return;
      }
      const disposed = stdioClient?.dispose();
      stdioClient = null;
      return disposed;
    },
  };
}

export default {
  id: "moon",
  register(api) {
    const sharedWorkerState = { client: null };
    api.registerService({
      id: "moon-local-embedding-worker",
      start() {},
      async stop() {
        const disposed = sharedWorkerState.client?.dispose();
        sharedWorkerState.client = null;
        await disposed;
      },
    });
    api.registerContextEngine(
      "moon",
      () => createMoonContextEngine(api, sharedWorkerState),
    );
  },
};

export const __moonTest = {
  contextArguments,
  contextWorkerRequest,
  completedTurnFromParams,
  createMoonContextEngine,
  delegateCompaction,
  distillBatchArguments,
  evidenceSupportsContent,
  injectPacket,
  isLearningCandidate,
  isTrivialQuery,
  metricInjectionArguments,
  normalizeProposal,
  observeCompaction,
  queryFromParams,
  recordArguments,
  resolveSettings,
  parseModelReference,
  runModelWithFallback,
  runOpenClawModel,
  runtimeMetricArguments,
  stdioWorkerArguments,
  unicodeLength,
  visibleText,
};

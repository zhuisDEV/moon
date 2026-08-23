import moonPlugin, { __moonTest } from "./index.js";

const METRIC_REQUEST_ID = "0123456789abcdef0123456789abcdef";

function metricsEnvelope(packet: string | null) {
  return JSON.stringify({
    request_id: METRIC_REQUEST_ID,
    packet,
    memory_count: packet ? 1 : 0,
    reference_count: 0,
    packet_chars: packet?.length ?? 0,
    truncated: false,
  });
}

function assert(
  condition: unknown,
  message = "assertion failed",
): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function assertEquals(actual: unknown, expected: unknown) {
  const left = JSON.stringify(actual);
  const right = JSON.stringify(expected);
  if (left !== right) {
    throw new Error(`expected ${right}, got ${left}`);
  }
}

function createApi(
  result: { code: number; stdout: string; stderr: string },
  calls: Array<{ argv: string[]; timeoutMs: number; input?: string }>,
  overrides: Record<string, unknown> = {},
  embeddedRunner?: (params: Record<string, unknown>) => unknown,
) {
  return {
    config: {
      agents: {
        defaults: {
          model: {
            primary: "vllm/local-primary",
            fallbacks: ["openai/remote-fallback"],
          },
        },
      },
    },
    pluginConfig: {
      moonPath: "/tmp/bin/moon",
      moonHome: "/tmp/moon-home",
      mode: "lexical",
      embeddingEnabled: false,
      ...overrides,
    },
    resolvePath(value: string) {
      return value;
    },
    runtime: {
      system: {
        runCommandWithTimeout(
          argv: string[],
          options: { timeoutMs: number; input?: string },
        ) {
          calls.push({
            argv,
            timeoutMs: options.timeoutMs,
            input: options.input,
          });
          return result;
        },
      },
      agent: {
        runEmbeddedPiAgent: embeddedRunner,
      },
    },
    logger: {
      error() {},
    },
  };
}

Deno.test("adapter retrieves and injects context before the latest user message", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const packet = "# Moon Context\n\n## Retrieved references\n\nUseful context";
  const api = createApi(
    { code: 0, stdout: metricsEnvelope(packet), stderr: "" },
    calls,
  );
  const engine = __moonTest.createMoonContextEngine(api);
  const messages = [
    { role: "assistant", content: [{ type: "text", text: "Earlier answer" }] },
    { role: "user", content: [{ type: "text", text: "Recall SQLite plan" }] },
  ];

  const result = await engine.assemble({ messages });
  assertEquals(calls.length, 2);
  assertEquals(
    calls[0].argv.slice(0, 7),
    [
      "/tmp/bin/moon",
      "--home",
      "/tmp/moon-home",
      "--dimensions",
      "384",
      "context",
      "--query",
    ],
  );
  assert(calls[0].argv.includes("Recall SQLite plan"));
  assert(calls[0].argv.includes("--adapter"));
  assert(calls[1].argv.includes("mark-injection"));
  assert(calls[1].argv.includes("--injected"));
  assert(calls[1].argv.includes(METRIC_REQUEST_ID));
  assertEquals(result.messages.length, 3);
  assertEquals(result.messages[1].role, "assistant");
  assertEquals(result.messages[1].content[0].text, packet);
  assertEquals(result.messages[2].role, "user");
  assert(engine.info.ownsCompaction === false);
});

Deno.test("adapter fails open without changing messages", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "database unavailable" },
    calls,
  );
  const engine = __moonTest.createMoonContextEngine(api);
  const messages = [
    { role: "user", content: [{ type: "text", text: "Recall history" }] },
  ];
  const result = await engine.assemble({ messages });
  assertEquals(result.messages, messages);
  assertEquals(calls.length, 1);
});

Deno.test("adapter skips retrieval for greetings and empty packets", async () => {
  const greetingCalls: Array<
    { argv: string[]; timeoutMs: number; input?: string }
  > = [];
  const greetingApi = createApi(
    { code: 0, stdout: "unused", stderr: "" },
    greetingCalls,
  );
  const greetingEngine = __moonTest.createMoonContextEngine(
    greetingApi,
  );
  const greetingMessages = [
    { role: "user", content: [{ type: "text", text: "Hi lilac" }] },
  ];
  const greeting = await greetingEngine.assemble({
    messages: greetingMessages,
  });
  assertEquals(greeting.messages, greetingMessages);
  assertEquals(greetingCalls.length, 0);

  const emptyCalls: Array<
    { argv: string[]; timeoutMs: number; input?: string }
  > = [];
  const emptyApi = createApi(
    { code: 0, stdout: metricsEnvelope(null), stderr: "" },
    emptyCalls,
  );
  const emptyEngine = __moonTest.createMoonContextEngine(emptyApi);
  const emptyMessages = [
    {
      role: "user",
      content: [{ type: "text", text: "Unrelated obscure subject" }],
    },
  ];
  const empty = await emptyEngine.assemble({ messages: emptyMessages });
  assertEquals(empty.messages, emptyMessages);
  assertEquals(emptyCalls.length, 2);
  assert(emptyCalls[1].argv.includes("mark-injection"));
  assert(!emptyCalls[1].argv.includes("--injected"));
});

Deno.test("adapter delegates compaction to OpenClaw", async () => {
  const params = {
    sessionId: "session-compact",
    sessionFile: "/tmp/session-compact.jsonl",
    force: true,
    runtimeContext: { agentHarnessId: "openclaw" },
  };
  let delegated: unknown = null;
  const result = await __moonTest.delegateCompaction(
    params,
    () =>
      Promise.resolve({
        delegateCompactionToRuntime(received: unknown) {
          delegated = received;
          return {
            ok: true,
            compacted: true,
            result: { tokensBefore: 100, tokensAfter: 20 },
          };
        },
      }),
  );
  assertEquals(delegated, params);
  assertEquals(result.compacted, true);
});

Deno.test("adapter refuses unsafe generic compaction for a native harness", async () => {
  let loaded = false;
  const result = await __moonTest.delegateCompaction(
    {
      sessionId: "session-codex",
      sessionFile: "/tmp/session-codex.jsonl",
      force: true,
      runtimeContext: { agentHarnessId: "codex" },
    },
    () => {
      loaded = true;
      return Promise.resolve({});
    },
  );
  assertEquals(loaded, false);
  assertEquals(result.ok, true);
  assertEquals(result.compacted, false);
  assert(String(result.reason).includes("native automatic compaction"));
});

Deno.test("adapter records content-free compaction metrics", async () => {
  const calls: Record<string, unknown>[] = [];
  const api = createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    { mode: "hybrid" },
  );
  const settings = __moonTest.resolveSettings(api);
  const worker = {
    request(operation: Record<string, unknown>) {
      calls.push(operation);
      return { event_id: METRIC_REQUEST_ID };
    },
  };
  const outcome = await __moonTest.observeCompaction(
    api,
    settings,
    { runtimeContext: { agentHarnessId: "openclaw" } },
    worker,
    () =>
      Promise.resolve({
        ok: true,
        compacted: true,
        result: { tokensBefore: 100, tokensAfter: 20 },
      }),
  );
  assertEquals(outcome.compacted, true);
  assertEquals(calls.length, 1);
  assertEquals(calls[0].op, "runtime_metric");
  assertEquals(calls[0].event_kind, "compaction");
  assertEquals(calls[0].compacted, true);
  assertEquals(calls[0].tokens_before, 100);
  assertEquals(calls[0].tokens_after, 20);
  assert(!("sessionId" in calls[0]));
});

Deno.test("adapter records one completed turn and distills a validated durable memory", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  let modelCalls = 0;
  const user = {
    role: "user",
    timestamp: 100,
    content: [{ type: "text", text: "I prefer concise answers." }],
  };
  const assistant = {
    role: "assistant",
    timestamp: 200,
    content: [{
      type: "text",
      text: "Understood. I will keep answers concise.",
    }],
  };
  const preview = __moonTest.completedTurnFromParams({
    sessionId: "session-1",
    messages: [user, assistant],
    prePromptMessageCount: 0,
  });
  assert(preview);
  const expectedEvidenceId = preview.evidenceSessionId;
  const baseApi = createApi(
    { code: 0, stdout: "", stderr: "" },
    calls,
    {
      learningEnabled: true,
      primaryModel: "vllm/qwen3.8-27b-uncensored-fp8",
      fallbackModel: "openai/gpt-5.6-luna",
      primaryReasoning: "high",
      fallbackReasoning: "medium",
    },
  );
  const api = {
    ...baseApi,
    runtime: {
      ...baseApi.runtime,
      system: {
        runCommandWithTimeout(
          argv: string[],
          options: { timeoutMs: number; input?: string },
        ) {
          calls.push({
            argv,
            timeoutMs: options.timeoutMs,
            input: options.input,
          });
          if (argv.includes("record")) {
            return {
              code: 0,
              stdout: JSON.stringify({
                session_id: expectedEvidenceId,
                changed: true,
              }),
              stderr: "",
            };
          }
          if (argv.includes("context")) {
            return {
              code: 0,
              stdout: JSON.stringify({ memories: [], references: [] }),
              stderr: "",
            };
          }
          if (argv.includes("distill-batch")) {
            return {
              code: 0,
              stdout: JSON.stringify({ distilled: 1, outcomes: [] }),
              stderr: "",
            };
          }
          if (argv.includes("record-runtime")) {
            return {
              code: 0,
              stdout: JSON.stringify({ event_id: METRIC_REQUEST_ID }),
              stderr: "",
            };
          }
          throw new Error(`unexpected command ${argv.join(" ")}`);
        },
      },
      agent: {
        runEmbeddedPiAgent(params: Record<string, unknown>) {
          modelCalls += 1;
          assertEquals(params.provider, "vllm");
          assertEquals(params.model, "qwen3.8-27b-uncensored-fp8");
          assertEquals(params.thinkLevel, "high");
          assertEquals(params.reasoningLevel, "off");
          return {
            payloads: [{
              text: JSON.stringify({
                eligible: true,
                memories: [{
                  canonical_key: "user:preference:response-style",
                  kind: "preference",
                  title: "Response style",
                  content: "The user prefers concise answers.",
                  evidence_quote: "I prefer concise answers.",
                  importance: 0.8,
                  confidence: 0.95,
                  supersedes_document_id: null,
                }],
              }),
            }],
          };
        },
      },
    },
  };
  const engine = __moonTest.createMoonContextEngine(api);
  await engine.afterTurn({
    sessionId: "session-1",
    sessionKey: "agent:main:discord:channel:123",
    sessionFile: "/tmp/session-1.jsonl",
    messages: [user, assistant],
    prePromptMessageCount: 0,
  });
  assertEquals(modelCalls, 1);
  assertEquals(calls.length, 4);
  assert(calls[0].argv.includes("record"));
  assertEquals(
    calls[0].input,
    "User:\nI prefer concise answers.\n\nAssistant:\nUnderstood. I will keep answers concise.",
  );
  assert(!calls[0].argv.includes("I prefer concise answers."));
  assert(calls[2].argv.includes("distill-batch"));
  assert(!calls[2].argv.includes("--proposal-json"));
  assert(!calls[2].argv.includes("The user prefers concise answers."));
  const proposal = JSON.parse(calls[2].input ?? "")[0];
  assertEquals(proposal.evidence_quote, "I prefer concise answers.");
  assert(calls[3].argv.includes("record-runtime"));
  assert(calls[3].argv.includes("--evidence-changed"));
  assert(calls[3].argv.includes("--learning-eligible"));
  assert(calls[3].argv.includes("--proposed-memories"));
});

Deno.test("adapter omits remote-provider arguments in lexical mode", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  const argv = __moonTest.contextArguments(settings, "query");
  assert(!argv.includes("--provider"));
  assert(!argv.includes("--api-key-env"));
});

Deno.test("hybrid mode uses the private local stdio worker", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    { mode: "hybrid" },
  ));
  assertEquals(
    __moonTest.stdioWorkerArguments(settings),
    [
      "/tmp/bin/moon",
      "--home",
      "/tmp/moon-home",
      "--dimensions",
      "384",
      "serve",
      "--provider",
      "local",
    ],
  );
  const request = __moonTest.contextWorkerRequest(
    settings,
    "Recall my Moon plan",
    false,
  );
  assertEquals(request.op, "context");
  assertEquals(request.mode, "hybrid");
  assertEquals(request.structured, false);
  assertEquals(request.observe, false);
  assertEquals(
    __moonTest.contextWorkerRequest(
      settings,
      "Recall my Moon plan",
      false,
      true,
    ).observe,
    true,
  );
});

Deno.test("plugin manifest is a strict context-engine manifest", async () => {
  const manifest = JSON.parse(
    await Deno.readTextFile(
      new URL("./openclaw.plugin.json", import.meta.url),
    ),
  );
  assertEquals(manifest.id, "moon");
  assertEquals(manifest.kind, "context-engine");
  assertEquals(manifest.configSchema.additionalProperties, false);
  assert(!("apiKeyEnv" in manifest.configSchema.properties));
  assert(!("endpoint" in manifest.configSchema.properties));
  assert(
    !Object.keys(manifest.configSchema.properties).some((key) =>
      key.toLowerCase().includes("codex")
    ),
  );
});

Deno.test("plugin registers the context engine and local compaction provider", async () => {
  const services: Record<string, unknown>[] = [];
  const engineFactories: Array<() => unknown> = [];
  const compactionProviders: Record<string, unknown>[] = [];
  moonPlugin.register({
    registerService(value: Record<string, unknown>) {
      services.push(value);
    },
    registerContextEngine(_id: string, factory: () => unknown) {
      engineFactories.push(factory);
    },
    registerCompactionProvider(provider: Record<string, unknown>) {
      compactionProviders.push(provider);
    },
  });
  const service = services[0];
  assertEquals(service.id, "moon-local-embedding-worker");
  assert(typeof service.start === "function");
  assert(typeof service.stop === "function");
  assert(typeof engineFactories[0] === "function");
  assertEquals(compactionProviders[0]?.id, "moon-local");
  assert(typeof compactionProviders[0]?.summarize === "function");
  await (service.stop as () => Promise<void>)();
});

Deno.test("model routing uses the OpenClaw primary model", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const embeddedCalls: Record<string, unknown>[] = [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    calls,
    {},
    (params) => {
      embeddedCalls.push(params);
      return { payloads: [{ text: "READY" }] };
    },
  );
  const settings = __moonTest.resolveSettings(api);
  const result = await __moonTest.runModelWithFallback(
    api,
    settings,
    "Return READY.",
    { sessionFile: "/tmp/moon-test-session.jsonl" },
  );
  assertEquals(result.modelRoute, "primary");
  assertEquals(result.reasoning, "off");
  assertEquals(result.output, "READY");
  assertEquals(embeddedCalls[0]?.provider, "vllm");
  assertEquals(embeddedCalls[0]?.model, "local-primary");
  assertEquals(embeddedCalls[0]?.thinkLevel, "off");
  assertEquals(embeddedCalls[0]?.reasoningLevel, "off");
  assertEquals(embeddedCalls[0]?.modelFallbacksOverride, []);
  assertEquals(embeddedCalls[0]?.modelRun, true);
  assertEquals(embeddedCalls[0]?.promptMode, "none");
  assertEquals(calls.length, 0);
});

Deno.test("model routing uses a provider-neutral fallback", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const baseApi = createApi(
    { code: 1, stdout: "", stderr: "" },
    calls,
    { fallbackReasoning: "ultra" },
  );
  const modelCalls: Array<Record<string, unknown>> = [];
  const api = {
    ...baseApi,
    runtime: {
      ...baseApi.runtime,
      agent: {
        runEmbeddedPiAgent(params: Record<string, unknown>) {
          modelCalls.push(params);
          if (params.provider === "vllm") {
            throw new Error("primary unavailable");
          }
          return { payloads: [{ text: "READY" }] };
        },
      },
    },
  };
  const settings = __moonTest.resolveSettings(api);
  const result = await __moonTest.runModelWithFallback(
    api,
    settings,
    "private canary prompt",
    { sessionFile: "/tmp/moon-test-session.jsonl" },
  );
  assertEquals(result.modelRoute, "fallback");
  assertEquals(result.model, "openai/remote-fallback");
  assertEquals(modelCalls.map((call) => `${call.provider}/${call.model}`), [
    "vllm/local-primary",
    "openai/remote-fallback",
  ]);
  assertEquals(modelCalls.map((call) => call.thinkLevel), ["off", "ultra"]);
  assertEquals(modelCalls.map((call) => call.reasoningLevel), ["off", "off"]);
  assertEquals(calls.length, 0);
});

Deno.test("local compaction uses an isolated reasoning-off model run", async () => {
  const embeddedCalls: Record<string, unknown>[] = [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {
      compactionModel: "vllm/local-compactor",
      compactionReasoning: "off",
      compactionMaxTokens: 2048,
    },
    (params) => {
      embeddedCalls.push(params);
      return { payloads: [{ text: "## Goal\nContinue local work safely." }] };
    },
  );
  const output = await __moonTest.summarizeCompaction(api, {
    messages: [{
      role: "assistant",
      content: [{ type: "toolCall", id: "call_123", name: "exec" }],
    }, {
      role: "toolResult",
      toolCallId: "call_123",
      content: [{ type: "text", text: "completed" }],
    }],
    previousSummary: "Earlier work used only local models.",
    customInstructions: "Preserve opaque identifiers exactly.",
    compressionRatio: 0.25,
  });
  assertEquals(output, "## Goal\nContinue local work safely.");
  assertEquals(embeddedCalls.length, 1);
  assertEquals(embeddedCalls[0]?.provider, "vllm");
  assertEquals(embeddedCalls[0]?.model, "local-compactor");
  assertEquals(embeddedCalls[0]?.thinkLevel, "off");
  assertEquals(embeddedCalls[0]?.reasoningLevel, "off");
  assertEquals(embeddedCalls[0]?.modelFallbacksOverride, []);
  assertEquals(embeddedCalls[0]?.streamParams, { maxTokens: 2048 });
  assertEquals(embeddedCalls[0]?.modelRun, true);
  assertEquals(embeddedCalls[0]?.promptMode, "none");
  assert(String(embeddedCalls[0]?.prompt).includes("call_123"));
  assert(
    String(embeddedCalls[0]?.prompt).includes(
      "Preserve opaque identifiers exactly.",
    ),
  );
  assert(
    String(embeddedCalls[0]?.prompt).includes(
      "Earlier work used only local models.",
    ),
  );
  assert(String(embeddedCalls[0]?.sessionFile).includes("moon-compaction-"));
});

Deno.test("local compaction redacts provider failure details", async () => {
  const api = createApi(
    { code: 1, stdout: "", stderr: "" },
    [],
    { compactionModel: "vllm/local-compactor" },
    () => {
      throw new Error("private transcript and TOKEN=must-not-leak");
    },
  );
  let message = "";
  try {
    await __moonTest.summarizeCompaction(api, { messages: [] });
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("Moon local compaction model request failed"));
  assert(!message.includes("must-not-leak"));
  assert(!message.includes("private transcript"));
});

Deno.test("local compaction rejects an OpenClaw error payload", async () => {
  const api = createApi(
    { code: 1, stdout: "", stderr: "" },
    [],
    { compactionModel: "vllm/local-compactor" },
    () => ({
      payloads: [{
        text: "⚠️ Agent couldn't generate a response. Please try again.",
        isError: true,
      }],
    }),
  );
  let message = "";
  try {
    await __moonTest.summarizeCompaction(api, { messages: [] });
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("Moon local compaction model request failed"));
  assert(!message.includes("couldn't generate"));
});

Deno.test("model routing falls back when primary output fails validation", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const modelCalls: string[] = [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "" },
    calls,
    {},
    (params) => {
      const modelRef = `${params.provider}/${params.model}`;
      modelCalls.push(modelRef);
      return {
        payloads: [{
          text: params.provider === "vllm" ? "not json" : '{"ok":true}',
        }],
      };
    },
  );
  const settings = __moonTest.resolveSettings(api);
  const result = await __moonTest.runModelWithFallback(
    api,
    settings,
    "Return JSON.",
    {
      sessionFile: "/tmp/moon-test-session.jsonl",
      validateOutput: JSON.parse,
    },
  );
  assertEquals(result.modelRoute, "fallback");
  assertEquals(result.validatedOutput, { ok: true });
  assertEquals(modelCalls, ["vllm/local-primary", "openai/remote-fallback"]);
});

Deno.test("model routing inherits OpenClaw primary and fallback models", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  assertEquals(settings.primaryModel, "vllm/local-primary");
  assertEquals(settings.fallbackModel, "openai/remote-fallback");
  assertEquals(settings.primaryReasoning, "off");
  assertEquals(settings.fallbackReasoning, "off");
  assertEquals(settings.compactionModel, "vllm/local-primary");
  assertEquals(settings.compactionReasoning, "off");
  assertEquals(settings.compactionTimeoutMs, 180_000);
  assertEquals(settings.compactionMaxTokens, 4_096);
});

Deno.test("plugin model routing overrides OpenClaw defaults", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    {
      primaryModel: "anthropic/claude-sonnet",
      fallbackModel: "google/gemini-pro",
      primaryReasoning: "high",
      fallbackReasoning: "low",
    },
  ));
  assertEquals(settings.primaryModel, "anthropic/claude-sonnet");
  assertEquals(settings.fallbackModel, "google/gemini-pro");
  assertEquals(settings.primaryReasoning, "high");
  assertEquals(settings.fallbackReasoning, "low");
});

Deno.test("duplicate fallback models are ignored", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    {
      primaryModel: "vllm/same-model",
      fallbackModel: "vllm/same-model",
    },
  ));
  assertEquals(settings.primaryModel, "vllm/same-model");
  assertEquals(settings.fallbackModel, null);
});

Deno.test("learning settings use a smaller packet", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  assertEquals(settings.maxChars, 3_500);
});

Deno.test("packet budgeting counts Unicode characters like Rust", () => {
  assertEquals(__moonTest.unicodeLength("Moon 🌙"), 6);
  assertEquals("Moon 🌙".length, 7);
});

Deno.test("learning evidence must support every numeric claim", () => {
  assert(__moonTest.evidenceSupportsContent(
    "Einstein was born on 14 March 1879 at 11:30 in Ulm.",
    "The birth certificate records 14 March 1879 at 11:30 in Ulm.",
  ));
  assert(
    !__moonTest.evidenceSupportsContent(
      "Einstein's Ascendant is Cancer 11°38′16″.",
      "The birth certificate records 14 March 1879 at 11:30 in Ulm.",
    ),
  );
});

Deno.test("automatic supersession requires an explicit correction and active head", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  const raw = {
    canonical_key: "user:preference:model",
    kind: "preference",
    title: "Preferred model",
    content: "The preferred model is Luna.",
    evidence_quote: "Actually, the preferred model is Luna.",
    importance: 0.8,
    confidence: 0.95,
    supersedes_document_id: 42,
  };
  const corrected = __moonTest.normalizeProposal(
    raw,
    {
      userText: "Actually, update my preference.",
      transcript:
        "User:\nActually, update my preference.\n\nAssistant:\nActually, the preferred model is Luna.",
    },
    settings,
    new Set([42]),
  );
  assert(corrected);
  assertEquals(corrected.supersedesDocumentId, 42);
  const uncorrected = __moonTest.normalizeProposal(
    raw,
    {
      userText: "Tell me about my preference.",
      transcript:
        "User:\nTell me about my preference.\n\nAssistant:\nActually, the preferred model is Luna.",
    },
    settings,
    new Set([42]),
  );
  assert(uncorrected);
  assertEquals(uncorrected.supersedesDocumentId, null);
});

Deno.test("assistant recall cannot create circular confirmation evidence", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  const raw = {
    canonical_key: "user:preference:model",
    kind: "preference",
    title: "Preferred model",
    content: "The preferred model is Luna.",
    evidence_quote: "The preferred model is Luna.",
    importance: 0.8,
    confidence: 0.95,
    supersedes_document_id: null,
  };
  const recalled = __moonTest.normalizeProposal(
    raw,
    {
      userText: "What is my preferred model?",
      transcript:
        "User:\nWhat is my preferred model?\n\nAssistant:\nThe preferred model is Luna.",
    },
    settings,
    new Set([42]),
    new Set(["user:preference:model"]),
  );
  assertEquals(recalled, null);

  const confirmed = __moonTest.normalizeProposal(
    {
      ...raw,
      evidence_quote: "The preferred model is Luna.",
    },
    {
      userText: "The preferred model is Luna.",
      transcript:
        "User:\nThe preferred model is Luna.\n\nAssistant:\nConfirmed.",
    },
    settings,
    new Set([42]),
    new Set(["user:preference:model"]),
  );
  assert(confirmed);
});

Deno.test("model routing does not expose provider error bodies", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const baseApi = createApi({ code: 0, stdout: "", stderr: "" }, calls);
  const api = {
    ...baseApi,
    runtime: {
      ...baseApi.runtime,
      agent: {
        runEmbeddedPiAgent() {
          throw new Error("remote body API_KEY=must-not-print");
        },
      },
    },
  };
  const settings = __moonTest.resolveSettings(api);
  let message = "";
  try {
    await __moonTest.runModelWithFallback(
      api,
      settings,
      "Return READY.",
      { sessionFile: "/tmp/moon-test-session.jsonl" },
    );
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("primary and fallback model requests failed"));
  assert(!message.includes("API_KEY"));
  assert(!message.includes("must-not-print"));
  assertEquals(calls.length, 0);
});

Deno.test("adapter invokes a real Moon binary when configured", async () => {
  const binaryPermission = await Deno.permissions.query({
    name: "env",
    variable: "MOON_TEST_BINARY",
  });
  const homePermission = await Deno.permissions.query({
    name: "env",
    variable: "MOON_TEST_HOME",
  });
  if (
    binaryPermission.state !== "granted" ||
    homePermission.state !== "granted"
  ) {
    return;
  }
  const binary = Deno.env.get("MOON_TEST_BINARY");
  const home = Deno.env.get("MOON_TEST_HOME");
  if (!binary || !home) {
    return;
  }
  const mode = Deno.env.get("MOON_TEST_MODE") ?? "lexical";
  const query = Deno.env.get("MOON_TEST_QUERY") ??
    "roomKey redemptionKey participant reenter";
  const expected = Deno.env.get("MOON_TEST_EXPECTED");
  const api = {
    pluginConfig: {
      moonPath: binary,
      moonHome: home,
      mode,
      embeddingEnabled: false,
      maxChars: 6_000,
    },
    resolvePath(value: string) {
      return value;
    },
    runtime: {
      system: {
        async runCommandWithTimeout(argv: string[]) {
          const output = await new Deno.Command(argv[0], {
            args: argv.slice(1),
            stdout: "piped",
            stderr: "piped",
          }).output();
          return {
            code: output.code,
            stdout: new TextDecoder().decode(output.stdout),
            stderr: new TextDecoder().decode(output.stderr),
          };
        },
      },
    },
    logger: {
      error() {},
    },
  };
  const engine = __moonTest.createMoonContextEngine(api);
  const result = await engine.assemble({
    prompt: query,
    messages: [{
      role: "user",
      content: [{
        type: "text",
        text: "Recall the participant reentry design",
      }],
    }],
  });
  await engine.dispose();
  assertEquals(result.messages.length, 2);
  const packet = result.messages[0].content[0].text;
  assert(packet.startsWith("# Moon Context"));
  if (mode === "lexical") {
    assert(packet.includes("## Retrieved references"));
    assert(packet.includes("legacy://"));
  } else {
    assert(packet.includes("## Canonical memories"));
  }
  if (expected) {
    for (const phrase of expected.split("|")) {
      assert(
        packet.includes(phrase),
        `expected real Moon packet to include ${JSON.stringify(phrase)}`,
      );
    }
  }
});

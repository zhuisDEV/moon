import moonPlugin, { __moonTest } from "./index.js";
import { EventEmitter } from "node:events";
import type { spawn } from "node:child_process";

const METRIC_REQUEST_ID = "0123456789abcdef0123456789abcdef";

async function optionalTestEnv(name: string) {
  const permission = await Deno.permissions.query({
    name: "env",
    variable: name,
  });
  return permission.state === "granted" ? Deno.env.get(name) : undefined;
}

const realMoonBinary = await optionalTestEnv("MOON_TEST_BINARY");
const realMoonHome = await optionalTestEnv("MOON_TEST_HOME");
const requireRealMoon =
  await optionalTestEnv("MOON_REQUIRE_REAL_BINARY") === "1";
if (requireRealMoon && (!realMoonBinary || !realMoonHome)) {
  throw new Error(
    "required real-binary tests need MOON_TEST_BINARY and MOON_TEST_HOME",
  );
}
const realMoonConfig = realMoonBinary && realMoonHome
  ? { binary: realMoonBinary, home: realMoonHome }
  : null;

function acceptedParams(messages: Array<Record<string, unknown>>) {
  const admission = {
    agentId: "main",
    sessionId: "session-1",
    sessionKey: "agent:main:discord:channel:123",
    storePath: "/tmp/openclaw-agent.sqlite",
    generation: "generation-1",
    entryId: "user-1",
    rawSeq: 1,
    effectiveParentId: null,
    activeMessagePosition: 0,
    logicalTurnId: "turn-1",
    role: "user",
  };
  return {
    advancementKey: "advancement-1",
    admission,
    terminal: { ...admission, entryId: "assistant-1", rawSeq: 2 },
    sessionId: admission.sessionId,
    sessionKey: admission.sessionKey,
    messages,
  };
}

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

function learningConfig(l1 = {}, l2 = {}) {
  return {
    path: "/tmp/moon-home/moon.toml",
    present: true,
    learning: {
      observation_ttl_hours: 24,
      l1: { enabled: true, ...l1 },
      l2: {
        enabled: false,
        daily_at: "03:00",
        timezone: "Australia/Sydney",
        batch_size: 32,
        max_input_chars: 64_000,
        max_batches_per_day: 8,
        max_actions: 16,
        ...l2,
      },
    },
  };
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
          options: { timeoutMs: number; input?: string; signal?: AbortSignal },
        ): typeof result | Promise<typeof result> {
          calls.push({
            argv,
            timeoutMs: options.timeoutMs,
            input: options.input,
          });
          return result;
        },
      },
      agent: {
        runEmbeddedAgent: embeddedRunner,
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
    calls[0].argv.slice(0, 9),
    [
      "/tmp/bin/moon",
      "--home",
      "/tmp/moon-home",
      "--database",
      "/tmp/moon-home/state/moon.sqlite",
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
  const preview = __moonTest.acceptedTurnFromParams(
    acceptedParams([user, assistant]),
  );
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
          if (argv.includes("config")) {
            return {
              code: 0,
              stdout: JSON.stringify(learningConfig()),
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
        runEmbeddedAgent(params: Record<string, unknown>) {
          modelCalls += 1;
          assertEquals(params.provider, "vllm");
          assertEquals(params.model, "qwen3.8-27b-uncensored-fp8");
          assertEquals(params.thinkLevel, "high");
          assertEquals(params.reasoningLevel, "off");
          assertEquals(params.workspaceDir, "/tmp/task-workspace");
          assertEquals(params.timeoutMs, 120_000);
          assertEquals(params.sessionPersistence, "detached");
          assert(!("sessionFile" in params));
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
  const committed = await engine.commitTurn({
    ...acceptedParams([user, assistant]),
    runtimeContext: { cwd: "/tmp/task-workspace" },
  });
  assertEquals(committed, { status: "committed" });
  assert(!("afterTurn" in engine));
  assertEquals(modelCalls, 1);
  assertEquals(calls.length, 5);
  assert(calls[0].argv.includes("record"));
  assertEquals(
    calls[0].input,
    "User:\nI prefer concise answers.\n\nAssistant:\nUnderstood. I will keep answers concise.",
  );
  assert(!calls[0].argv.includes("I prefer concise answers."));
  assert(calls[1].argv.includes("config"));
  assert(calls[3].argv.includes("distill-batch"));
  assert(!calls[3].argv.includes("--proposal-json"));
  assert(!calls[3].argv.includes("The user prefers concise answers."));
  const proposal = JSON.parse(calls[3].input ?? "")[0];
  assertEquals(proposal.evidence_quote, "I prefer concise answers.");
  assert(calls[4].argv.includes("record-runtime"));
  assert(calls[4].argv.includes("--evidence-changed"));
  assert(calls[4].argv.includes("--learning-eligible"));
  assert(calls[4].argv.includes("--proposed-memories"));
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
      "--database",
      "/tmp/moon-home/state/moon.sqlite",
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

Deno.test("adapter commands pin every explicit home to its own database", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    { moonHome: "/tmp/moon isolated home/", mode: "hybrid" },
  ));
  const turn = {
    evidenceSessionId: "isolated-turn",
    completedAtMs: 100,
    metadata: {},
  };
  for (
    const argv of [
      __moonTest.contextArguments(settings, "query"),
      __moonTest.stdioWorkerArguments(settings),
      __moonTest.recordArguments(settings, turn),
      __moonTest.distillBatchArguments(settings, turn.evidenceSessionId),
      __moonTest.metricInjectionArguments(settings, METRIC_REQUEST_ID, true),
      __moonTest.runtimeMetricArguments(settings, {
        event_kind: "learning",
        status: "ok",
        duration_us: 0,
      }),
    ]
  ) {
    const databaseFlag = argv.indexOf("--database");
    assert(databaseFlag > 0, "explicit --home must override MOON_DATABASE");
    assertEquals(
      argv[databaseFlag + 1],
      "/tmp/moon isolated home/state/moon.sqlite",
    );
  }
  const implicit = { ...settings, moonHome: null };
  assert(
    !__moonTest.contextArguments(implicit, "query").includes("--database"),
  );
  assert(!__moonTest.stdioWorkerArguments(implicit).includes("--database"));
});

function fakeMoonWorker() {
  const written: string[] = [];
  return Object.assign(new EventEmitter(), {
    stdout: Object.assign(new EventEmitter(), { setEncoding() {} }),
    stdin: Object.assign(new EventEmitter(), {
      write(value: string) {
        written.push(value);
        return true;
      },
    }),
    written,
    killed: false,
    killCount: 0,
    kill() {
      this.killed = true;
      this.killCount += 1;
      return true;
    },
  });
}

function fakeMoonSpawner(children: ReturnType<typeof fakeMoonWorker>[]) {
  // The injected child implements only the stdio/process events used by Moon.
  return (() => {
    const child = fakeMoonWorker();
    children.push(child);
    return child;
  }) as unknown as typeof spawn;
}

Deno.test("worker timeout recovery ignores retired output errors and exit", async () => {
  const children: ReturnType<typeof fakeMoonWorker>[] = [];
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  const client = new __moonTest.MoonStdioClient(
    settings,
    fakeMoonSpawner(children),
  );
  const timedOut = await client.request({ op: "context" }, 1).catch(
    (error: Error) => error.message,
  );
  assertEquals(timedOut, "moon worker request timed out after 1ms");
  const retired = children[0];
  const replacement = client.request({ op: "context" }, 1_000);
  const active = children[1];
  const { id } = JSON.parse(active.written[0]);
  active.stdout.emit("data", `{"id":${id},"ok":true,"result":`);
  retired.stdout.emit("data", "invalid retired output\n");
  retired.emit("error", new Error("retired process error"));
  retired.stdout.emit("error", new Error("retired stdout error"));
  retired.stdin.emit("error", new Error("retired stdin error"));
  retired.emit("exit", null, "SIGTERM");
  retired.emit("close", null, "SIGTERM");
  active.stdout.emit("data", '"recovered"}\n');
  assertEquals(await replacement, "recovered");
  assertEquals(active.killCount, 0);
  assert(Object.is(client.child, active));
  assertEquals(retired.killCount, 1);
  const disposed = client.dispose();
  active.emit("exit", null, "SIGTERM");
  active.emit("close", null, "SIGTERM");
  await disposed;
});

Deno.test("worker disposal waits for retiring children and is idempotent", async () => {
  const children: ReturnType<typeof fakeMoonWorker>[] = [];
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  const client = new __moonTest.MoonStdioClient(
    settings,
    fakeMoonSpawner(children),
  );
  const timedOut = client.request({ op: "context" }, 1).catch(() => {});
  await timedOut;
  const replacement = client.request({ op: "context" }, 1_000).catch(
    (error: Error) => error.message,
  );
  let disposed = false;
  const first = client.dispose().then(() => {
    disposed = true;
  });
  const repeated = client.dispose();
  assertEquals(await replacement, "moon worker disposed");
  assertEquals(children.map((child) => child.killCount), [1, 1]);
  children[1].emit("close", null, "SIGTERM");
  await Promise.resolve();
  assertEquals(disposed, false);
  children[0].emit("close", null, "SIGTERM");
  await Promise.all([first, repeated]);
  assertEquals(disposed, true);
  assertEquals(client.pending.size, 0);
  assertEquals(client.childClosures.size, 0);
  await client.dispose();
  assertEquals(children.map((child) => child.killCount), [1, 1]);
});

Deno.test("worker pipe failures reject pending work and clear request timers", async () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  for (const failure of ["write", "stdin", "stdout"] as const) {
    const children: ReturnType<typeof fakeMoonWorker>[] = [];
    const client = new __moonTest.MoonStdioClient(
      settings,
      fakeMoonSpawner(children),
    );
    client.start();
    const child = children[0];
    if (failure === "write") {
      child.stdin.write = () => {
        throw new Error("pipe failed");
      };
    }
    const request = client.request({ op: "context" }, 1_000).catch(
      (error: Error) => error.message,
    );
    if (failure !== "write") {
      child[failure].emit(
        "error",
        new Error("pipe failed"),
      );
    }
    assertEquals(await request, "pipe failed");
    assertEquals(client.pending.size, 0);
    assertEquals(client.child, null);
    assertEquals(child.killCount, 1);
    const disposed = client.dispose();
    child.emit("close", null, "SIGTERM");
    await disposed;
  }
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

async function beforeServiceStopDeadline<T>(promise: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        // Leave margin inside OpenClaw's five-second replacement deadline.
        timer = setTimeout(
          () => reject(new Error("service did not settle")),
          4000,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

Deno.test("service shutdown interrupts L2 embedding and restart owns a fresh worker", async () => {
  const prototype = __moonTest.MoonStdioClient.prototype;
  const originalRequest = prototype.request;
  const originalDispose = prototype.dispose;
  const workers: object[] = [];
  const blocked = new Map<object, (error: Error) => void>();
  let embeddingEntered = Promise.withResolvers<void>();
  let configs = 0;
  const api = createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    { embeddingEnabled: true },
    () => ({ payloads: [{ text: '{"actions":[]}' }] }),
  );
  api.runtime.system.runCommandWithTimeout = (argv) => {
    let output;
    if (argv.includes("config")) {
      configs += 1;
      output = learningConfig({}, {
        enabled: true,
        fallback_enabled: false,
        max_batches_per_day: configs,
      });
    } else if (argv.includes("prepare")) {
      const key = argv[argv.indexOf("--run-key") + 1];
      output = configs > 1 && key.endsWith(":0") ? { status: "committed" } : {
        status: "prepared",
        run_id: `run-${configs}`,
        scope: "global",
        evidence: [],
        memories: [],
      };
    } else if (argv.includes("apply")) {
      output = { status: "committed", action_count: 0, processed_evidence: 0 };
    } else throw new Error("unexpected command");
    return { code: 0, stdout: JSON.stringify(output), stderr: "" };
  };
  const services: Array<{ start(): void; stop(): Promise<void> }> = [];
  moonPlugin.register({
    ...api,
    registerService(service: typeof services[number]) {
      services.push(service);
    },
    registerContextEngine() {},
    registerCompactionProvider() {},
  });
  const service = services[0];
  prototype.request = function (
    this: typeof prototype,
    operation: { op: string },
  ) {
    assertEquals(operation.op, "embed");
    workers.push(this);
    embeddingEntered.resolve();
    return new Promise((_, reject) => blocked.set(this, reject));
  };
  prototype.dispose = function (this: typeof prototype) {
    blocked.get(this)?.(new Error("fixture worker disposed"));
    blocked.delete(this);
    return originalDispose.call(this);
  };
  try {
    service.start();
    await beforeServiceStopDeadline(embeddingEntered.promise);
    await beforeServiceStopDeadline(service.stop());
    assertEquals(blocked.size, 0);

    embeddingEntered = Promise.withResolvers<void>();
    service.start();
    await beforeServiceStopDeadline(embeddingEntered.promise);
    assertEquals(workers.length, 2);
    assert(workers[0] !== workers[1]);
    await beforeServiceStopDeadline(service.stop());
    assertEquals(blocked.size, 0);
  } finally {
    for (const release of blocked.values()) {
      release(new Error("fixture cleanup"));
    }
    blocked.clear();
    await service.stop();
    prototype.request = originalRequest;
    prototype.dispose = originalDispose;
  }
});

Deno.test("scheduler shutdown aborts pending config, prepare and apply commands", async () => {
  for (const phase of ["config", "prepare", "apply"]) {
    const entered = Promise.withResolvers<void>();
    const blocked = Promise.withResolvers<
      { code: number; stdout: string; stderr: string }
    >();
    let cancelled = false;
    let cleanupTimeout: number | undefined;
    const api = createApi(
      { code: 0, stdout: "", stderr: "" },
      [],
      {},
      () => ({ payloads: [{ text: '{"actions":[]}' }] }),
    );
    api.runtime.system.runCommandWithTimeout = (argv, options) => {
      if (argv.includes(phase)) {
        assert(options.signal instanceof AbortSignal);
        options.signal.addEventListener("abort", () => {
          cancelled = true;
          blocked.reject(new Error("fixture command cancelled"));
        }, { once: true });
        entered.resolve();
        return blocked.promise;
      }
      const output = argv.includes("config")
        ? learningConfig({}, { enabled: true, fallback_enabled: false })
        : argv.includes("prepare")
        ? {
          status: "prepared",
          run_id: "run",
          scope: "global",
          evidence: [],
          memories: [],
        }
        : { status: "failed" };
      if (argv.includes("fail")) {
        cleanupTimeout = options.timeoutMs;
        assert(options.signal === undefined);
      }
      return { code: 0, stdout: JSON.stringify(output), stderr: "" };
    };
    const scheduler = __moonTest.createLearningScheduler(api);
    const running = scheduler.tick();
    try {
      await beforeServiceStopDeadline(entered.promise);
      await beforeServiceStopDeadline(scheduler.stop());
      assert(cancelled);
      if (phase === "apply") assertEquals(cleanupTimeout, 1000);
    } finally {
      blocked.reject(new Error("fixture cleanup"));
      await scheduler.stop();
      await running;
    }
  }
});

Deno.test("late L2 apply completion cannot create an embedding worker after service stop", async () => {
  const entered = Promise.withResolvers<void>();
  const completed = Promise.withResolvers<
    { code: number; stdout: string; stderr: string }
  >();
  const prototype = __moonTest.MoonStdioClient.prototype;
  const originalRequest = prototype.request;
  let embeddingRequests = 0;
  const api = createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    { embeddingEnabled: true },
    () => ({ payloads: [{ text: '{"actions":[]}' }] }),
  );
  api.runtime.system.runCommandWithTimeout = (argv) => {
    if (argv.includes("apply")) {
      entered.resolve();
      // Model a command that committed just as cancellation was requested.
      return completed.promise;
    }
    const output = argv.includes("config")
      ? learningConfig({}, {
        enabled: true,
        fallback_enabled: false,
        max_batches_per_day: 1,
      })
      : {
        status: "prepared",
        run_id: "run",
        scope: "global",
        evidence: [],
        memories: [],
      };
    return { code: 0, stdout: JSON.stringify(output), stderr: "" };
  };
  const services: Array<{ start(): void; stop(): Promise<void> }> = [];
  moonPlugin.register({
    ...api,
    registerService(service: typeof services[number]) {
      services.push(service);
    },
    registerContextEngine() {},
    registerCompactionProvider() {},
  });
  prototype.request = () => {
    embeddingRequests += 1;
    return Promise.resolve({ embedded: 0, remaining: 0 });
  };
  try {
    services[0].start();
    await beforeServiceStopDeadline(entered.promise);
    const stopping = services[0].stop();
    completed.resolve({
      code: 0,
      stdout: JSON.stringify({
        status: "committed",
        action_count: 0,
        processed_evidence: 0,
      }),
      stderr: "",
    });
    await beforeServiceStopDeadline(stopping);
    assertEquals(embeddingRequests, 0);
  } finally {
    completed.resolve({ code: 1, stdout: "", stderr: "fixture cleanup" });
    await services[0].stop();
    prototype.request = originalRequest;
  }
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
  assertEquals(embeddedCalls[0]?.disableTools, true);
  assertEquals(embeddedCalls[0]?.sessionPersistence, "detached");
  assert(!("sessionFile" in embeddedCalls[0]));
  assertEquals(embeddedCalls[0]?.timeoutMs, 120_000);
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
        runEmbeddedAgent(params: Record<string, unknown>) {
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

Deno.test("model attempts use detached identities without a file-backed transcript target", async () => {
  const sessionIds: string[] = [];
  const sessionKeys: string[] = [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {},
    (params) => {
      assertEquals(params.sessionPersistence, "detached");
      assert(!("sessionFile" in params));
      sessionIds.push(String(params.sessionId));
      sessionKeys.push(String(params.sessionKey));
      if (params.provider === "vllm") {
        throw new Error("primary unavailable");
      }
      return { payloads: [{ text: "READY" }] };
    },
  );
  await __moonTest.runModelWithFallback(
    api,
    __moonTest.resolveSettings(api),
    "Return READY.",
    {
      sessionFile: "/tmp/live-transcript-must-not-be-used.jsonl",
      sessionId: "live-session",
      sessionKey: "agent:research:discord:channel:123",
    },
  );
  assertEquals(new Set(sessionIds).size, 2);
  assert(!sessionIds.includes("live-session"));
  assertEquals(new Set(sessionKeys).size, 2);
  assert(
    sessionKeys.every((key) =>
      /^agent:research:internal-session-effects:incognito-[^:]+$/.test(key)
    ),
  );
});

Deno.test("model routing accepts only final answer payloads", async () => {
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {},
    () => ({
      payloads: [
        { text: "private reasoning", isReasoning: true },
        { text: "working on it", isCommentary: true },
        { text: '{"eligible":false}' },
      ],
    }),
  );
  const result = await __moonTest.runModelWithFallback(
    api,
    __moonTest.resolveSettings(api),
    "Return JSON.",
    { validateOutput: JSON.parse },
  );
  assertEquals(result.output, '{"eligible":false}');
  assertEquals(result.validatedOutput, { eligible: false });
});

Deno.test("model helpers use native ephemeral identities with the correct agent owner", async () => {
  const cases = [
    {
      agents: {
        entries: { research: {}, system: {} },
        defaults: { systemAgent: { agentId: "system" } },
      },
      params: {
        sessionKey: "agent:research:discord:channel:123",
        agentId: "research",
      },
      owner: "research",
    },
    {
      agents: { entries: { research: {}, system: {} } },
      params: { agentId: "research" },
      owner: "research",
    },
    {
      agents: {
        entries: { research: {}, system: {} },
        defaults: { systemAgent: { agentId: "system" } },
      },
      params: {},
      owner: "system",
    },
    { agents: { entries: { research: {} } }, params: {}, owner: "research" },
    { agents: { list: [{ id: "research" }] }, params: {}, owner: "research" },
    {
      agents: { entries: { research: { default: true }, system: {} } },
      params: {},
      owner: "research",
    },
    {
      agents: { list: [{ id: "research", default: true }, { id: "system" }] },
      params: {},
      owner: "research",
    },
    { agents: { defaults: {} }, params: {}, owner: "main" },
  ];
  for (const example of cases) {
    const runs: Array<Record<string, unknown>> = [];
    const api = createApi(
      { code: 1, stdout: "", stderr: "unexpected command" },
      [],
      {},
      (params) => {
        runs.push(params);
        return { payloads: [{ text: "READY" }] };
      },
    );
    Object.assign(api.config, { agents: example.agents });
    await __moonTest.runOpenClawModel(
      api,
      __moonTest.resolveSettings(api),
      "Return READY",
      {
        ...example.params,
        modelRef: "openai/gpt-6-astra",
        reasoning: "low",
        route: "primary",
      },
    );
    const run = runs[0];
    assertEquals(run.agentId, example.owner);
    const key = String(run.sessionKey);
    assert(
      /^agent:[a-z0-9][a-z0-9_-]{0,63}:internal-session-effects:incognito-[^:]+$/
        .test(key),
    );
    assert(
      key.startsWith(
        `agent:${example.owner}:internal-session-effects:incognito-`,
      ),
    );
    const suffix = key.split(":incognito-")[1];
    assertEquals(run.sessionId, `internal-session-effects-${suffix}`);
    assertEquals(run.sessionPersistence, "detached");
    assertEquals(run.toolsAllow, []);
    assertEquals(run.disableTools, true);
    assertEquals(run.disableMessageTool, true);
    assert(run.config === api.config);
  }
});

Deno.test("model helpers reject ambiguous and conflicting agent owners before inference", async () => {
  const cases = [
    {
      agents: {},
      params: { sessionKey: "agent:research:discord:123", agentId: "other" },
    },
    { agents: {}, params: { sessionKey: "agent::discord:123" } },
    { agents: {}, params: { agentId: "bad:owner" } },
    { agents: { entries: {} }, params: {} },
    { agents: { list: [] }, params: {} },
    { agents: { entries: { first: {}, second: {} } }, params: {} },
    {
      agents: {
        ownership: "explicit",
        entries: { first: { default: true }, second: {} },
      },
      params: {},
    },
    {
      agents: {
        entries: { first: {} },
        defaults: { systemAgent: { agentId: "" } },
      },
      params: {},
    },
  ];
  for (const example of cases) {
    let runs = 0;
    const api = createApi(
      { code: 1, stdout: "", stderr: "unexpected command" },
      [],
      {},
      () => {
        runs += 1;
        return { payloads: [{ text: "READY" }] };
      },
    );
    Object.assign(api.config, { agents: example.agents });
    let rejected = false;
    try {
      await __moonTest.runOpenClawModel(
        api,
        __moonTest.resolveSettings(api),
        "Return READY",
        {
          ...example.params,
          modelRef: "openai/gpt-6-astra",
          reasoning: "low",
          route: "primary",
        },
      );
    } catch {
      rejected = true;
    }
    assert(rejected);
    assertEquals(runs, 0);
  }
});

Deno.test("model routing honours cancellation before starting inference", async () => {
  let calls = 0;
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {},
    () => {
      calls += 1;
      return { payloads: [{ text: "READY" }] };
    },
  );
  const controller = new AbortController();
  controller.abort(new Error("private cancellation details"));
  let message = "";
  try {
    await __moonTest.runModelWithFallback(
      api,
      __moonTest.resolveSettings(api),
      "Return READY.",
      { signal: controller.signal },
    );
  } catch (error) {
    message = String(error);
  }
  assertEquals(calls, 0);
  assert(message.includes("model request cancelled"));
  assert(!message.includes("private cancellation details"));
});

Deno.test("cancelling an active model request stops fallback routing", async () => {
  const controller = new AbortController();
  let calls = 0;
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {},
    (params) => {
      calls += 1;
      assert(params.abortSignal === controller.signal);
      assertEquals(params.timeoutMs, 4_000);
      controller.abort();
      return { payloads: [{ text: "partial response" }] };
    },
  );
  let message = "";
  try {
    await __moonTest.runModelWithFallback(
      api,
      __moonTest.resolveSettings(api),
      "Return READY.",
      { signal: controller.signal, timeoutMs: 4_000 },
    );
  } catch (error) {
    message = String(error);
  }
  assertEquals(calls, 1);
  assert(message.includes("model request cancelled"));
});

Deno.test("runtime abort metadata rejects partial output without a fallback", async () => {
  let calls = 0;
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {},
    () => {
      calls += 1;
      return {
        payloads: [{ text: "partial response" }],
        meta: { aborted: true },
      };
    },
  );
  let message = "";
  try {
    await __moonTest.runModelWithFallback(
      api,
      __moonTest.resolveSettings(api),
      "Return READY.",
    );
  } catch (error) {
    message = String(error);
  }
  assertEquals(calls, 1);
  assert(message.includes("model request cancelled"));
});

Deno.test("runtime error metadata rejects partial output and allows model fallback", async () => {
  const calls: string[] = [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {},
    (params) => {
      calls.push(String(params.provider));
      return params.provider === "vllm"
        ? {
          payloads: [{ text: "partial response" }],
          meta: {
            error: { kind: "incomplete_turn", message: "private details" },
          },
        }
        : { payloads: [{ text: "READY" }] };
    },
  );
  const outcome = await __moonTest.runModelWithFallback(
    api,
    __moonTest.resolveSettings(api),
    "Return READY.",
  );
  assertEquals(calls, ["vllm", "openai"]);
  assertEquals(outcome.output, "READY");
  assertEquals(outcome.modelRoute, "fallback");
});

Deno.test("runtime timeout metadata allows fallback without accepting partial output", async () => {
  const calls: string[] = [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    [],
    {},
    (params) => {
      calls.push(String(params.provider));
      return params.provider === "vllm"
        ? {
          payloads: [{ text: "partial response" }],
          meta: { aborted: true, timeoutPhase: "provider" },
        }
        : { payloads: [{ text: "READY" }] };
    },
  );
  const outcome = await __moonTest.runModelWithFallback(
    api,
    __moonTest.resolveSettings(api),
    "Return READY.",
  );
  assertEquals(calls, ["vllm", "openai"]);
  assertEquals(outcome.output, "READY");
  assertEquals(outcome.modelRoute, "fallback");
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
  assertEquals(embeddedCalls[0]?.sessionPersistence, "detached");
  assert(!("sessionFile" in embeddedCalls[0]));
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

Deno.test("local compaction follows host identifier-preservation policy", () => {
  const custom = __moonTest.compactionPrompt({
    messages: [],
    summarizationInstructions: {
      identifierPolicy: "custom",
      identifierInstructions: "Keep only ticket identifiers exactly.",
    },
  });
  assert(custom.includes("Keep only ticket identifiers exactly."));
  assert(!custom.includes("Preserve exact opaque identifiers"));
  const off = __moonTest.compactionPrompt({
    messages: [],
    summarizationInstructions: { identifierPolicy: "off" },
  });
  assert(!off.includes("Preserve exact opaque identifiers"));
  const strict = __moonTest.compactionPrompt({ messages: [] });
  assert(strict.includes("Preserve exact opaque identifiers"));
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

Deno.test("learning numeric grounding compares complete values without precision loss", () => {
  for (
    const [claim, quote] of [
      ["100", "1000"],
      ["10", "10.5"],
      ["-100", "100"],
      ["100", "-100"],
      ["+100", "-100"],
      ["0.5", ".5"],
      ["5", ".5"],
      ["1", "1e3"],
      ["1e3", "1e30"],
      ["2026-09-07", "2026-09-07-01"],
      ["2026", "2026-09-07"],
      ["11:30", "11:30:59"],
      ["2.5", "2.5.3"],
      ["1/2", "1/20"],
      ["100", "1,000"],
      ["1000", "10,00"],
      ["1000", "1,0000"],
      ["1234567", "12,34,567"],
      ["1000", "1,000.00"],
      ["9007199254740992", "9007199254740993"],
      ["123456789012345678901", "1234567890123456789010"],
    ]
  ) {
    assert(
      !__moonTest.evidenceSupportsContent(
        `The recorded value is ${claim}.`,
        `The recorded value is ${quote}.`,
      ),
      `${JSON.stringify(claim)} must not be supported by ${
        JSON.stringify(quote)
      }`,
    );
  }
  for (
    const [claim, quote] of [
      ["100", "100"],
      ["-100", "-100"],
      ["+100", "+100"],
      ["-100", "−100"],
      [".5", ".5"],
      ["-.5", "−.5"],
      ["10.5", "10.5"],
      ["1e-3", "1E-3"],
      ["2026-09-07", "2026-09-07"],
      ["11:30:59", "11:30:59"],
      ["2.5.3", "2.5.3"],
      ["1/2", "1/2"],
      ["1000", "1,000"],
      ["1,000", "1000"],
      ["1000.50", "1,000.50"],
      ["-1000.50", "-1,000.50"],
      ["+1000", "+1,000"],
      ["1234567", "1,234,567"],
      ["123456789012345678901", "123456789012345678901"],
      ["123456789012345678901", "123,456,789,012,345,678,901"],
    ]
  ) {
    assert(
      __moonTest.evidenceSupportsContent(
        `The recorded value is ${claim}.`,
        `The recorded value is ${quote}.`,
      ),
      `${JSON.stringify(claim)} should be supported by ${
        JSON.stringify(quote)
      }`,
    );
  }
});

Deno.test("learning proposals reject a truncated amount and accept a complete grouped amount", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  const quote = "The budget is 1,000 dollars.";
  const raw = {
    canonical_key: "project:budget",
    kind: "fact",
    title: "Project budget",
    content: "The budget is 100 dollars.",
    evidence_quote: quote,
    importance: 0.9,
    confidence: 0.99,
    supersedes_document_id: null,
  };
  const turn = { userText: quote, transcript: `User:\n${quote}` };
  assertEquals(
    __moonTest.normalizeProposal(raw, turn, settings, new Set()),
    null,
  );
  const valid = __moonTest.normalizeProposal(
    { ...raw, content: "The budget is 1000 dollars." },
    turn,
    settings,
    new Set(),
  );
  assertEquals(valid?.content, "The budget is 1000 dollars.");
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
      userText: "Actually, the preferred model is Luna.",
      transcript:
        "User:\nActually, the preferred model is Luna.\n\nAssistant:\nUnderstood.",
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
  assertEquals(uncorrected, null);
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
        runEmbeddedAgent() {
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

Deno.test({
  name: "adapter invokes a real Moon binary when configured",
  ignore: !realMoonConfig,
  fn: async () => {
    assert(realMoonConfig);
    const { binary, home } = realMoonConfig;
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
  },
});

Deno.test("durable commits declare host fencing and use the accepted range only", () => {
  const params = acceptedParams([
    { role: "user", content: "Hello", timestamp: 100 },
    { role: "assistant", content: "Hello there", timestamp: 200 },
  ]);
  const first = __moonTest.acceptedTurnFromParams(params);
  assert(first);
  const replay = __moonTest.acceptedTurnFromParams({
    ...params,
    prePromptMessageCount: 99,
  });
  assertEquals(first, replay);
  assertEquals(first?.completedAtMs, 200);
  assert(first?.evidenceSessionId.match(/^openclaw:accepted:[a-f0-9]{64}$/));
  const next = __moonTest.acceptedTurnFromParams({
    ...params,
    advancementKey: "advancement-2",
  });
  assert(first.evidenceSessionId !== next?.evidenceSessionId);
  const engine = __moonTest.createMoonContextEngine(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  assertEquals(engine.info.transcriptSemantics, {
    currentTurnFence: "before-current-turn-entry-v1",
    turnAdvancementIdempotency: "atomic-idempotent-v1",
  });
  assert(!("afterTurn" in engine));
});

Deno.test("durable commits reject mismatched boundaries and unstable timestamps", () => {
  const params = acceptedParams([
    { role: "user", content: "Hello", timestamp: 100 },
    { role: "assistant", content: "Hello there", timestamp: 200 },
  ]);
  const invalid = [
    { ...params, advancementKey: "" },
    { ...params, sessionId: "different-session" },
    { ...params, terminal: { ...params.terminal, generation: "rotated" } },
    { ...params, terminal: { ...params.terminal, rawSeq: 0 } },
    { ...params, messages: [{ role: "assistant", content: "no admission" }] },
    {
      ...params,
      messages: params.messages.map(({ role, content }) => ({ role, content })),
    },
  ];
  for (const input of invalid) {
    let rejected = false;
    try {
      __moonTest.acceptedTurnFromParams(input);
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
});

Deno.test("durable storage failures propagate even with failOpen enabled", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const engine = __moonTest.createMoonContextEngine(createApi(
    { code: 1, stdout: "", stderr: "database unavailable" },
    calls,
    { failOpen: true },
  ));
  let rejected = false;
  try {
    await engine.commitTurn(acceptedParams([
      { role: "user", content: "Hello", timestamp: 100 },
      { role: "assistant", content: "Hello there", timestamp: 200 },
    ]));
  } catch {
    rejected = true;
  }
  assert(rejected);
  assertEquals(calls.length, 1);
  assert(calls[0].argv.includes("record"));
});

Deno.test("durable duplicate acknowledgement never repeats learning", async () => {
  const params = acceptedParams([
    { role: "user", content: "Remember my preference for tea", timestamp: 100 },
    { role: "assistant", content: "Understood", timestamp: 200 },
  ]);
  const turn = __moonTest.acceptedTurnFromParams(params);
  assert(turn);
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const engine = __moonTest.createMoonContextEngine(createApi({
    code: 0,
    stdout: JSON.stringify({
      session_id: turn.evidenceSessionId,
      changed: false,
    }),
    stderr: "",
  }, calls));
  assertEquals(await engine.commitTurn(params), { status: "duplicate" });
  assertEquals(calls.length, 1);
});

Deno.test("heartbeat and disabled-learning commits have no durable side effects", async () => {
  const params = acceptedParams([
    { role: "user", content: "Hello", timestamp: 100 },
    { role: "assistant", content: "Hello there", timestamp: 200 },
  ]);
  for (const learningEnabled of [false, true]) {
    const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
      [];
    const engine = __moonTest.createMoonContextEngine(createApi(
      { code: 1, stdout: "", stderr: "must not run" },
      calls,
      { learningEnabled },
    ));
    const input = { ...params, isHeartbeat: learningEnabled };
    assertEquals(await engine.commitTurn(input), { status: "committed" });
    assertEquals(await engine.commitTurn(input), { status: "committed" });
    assertEquals(calls.length, 0);
  }
});

Deno.test({
  name: "durable commit survives a lost acknowledgement with real SQLite",
  ignore: !realMoonConfig,
  fn: async () => {
    assert(realMoonConfig);
    const { binary, home } = realMoonConfig;
    const params = acceptedParams([
      { role: "user", content: "Hello", timestamp: 100 },
      { role: "assistant", content: "Hello there", timestamp: 200 },
    ]);
    params.advancementKey = `real-sqlite-${crypto.randomUUID()}`;
    let loseAcknowledgement = true;
    const api = createApi({ code: 0, stdout: "", stderr: "" }, [], {
      moonPath: binary,
      moonHome: home,
      failOpen: true,
    });
    api.runtime.system.runCommandWithTimeout = async (argv, options) => {
      const child = new Deno.Command(argv[0], {
        args: argv.slice(1),
        stdin: "piped",
        stdout: "piped",
        stderr: "piped",
      }).spawn();
      const writer = child.stdin.getWriter();
      await writer.write(new TextEncoder().encode(options.input ?? ""));
      await writer.close();
      const output = await child.output();
      if (loseAcknowledgement && output.code === 0 && argv.includes("record")) {
        loseAcknowledgement = false;
        throw new Error("simulated lost acknowledgement after SQLite commit");
      }
      return {
        code: output.code,
        stdout: new TextDecoder().decode(output.stdout),
        stderr: new TextDecoder().decode(output.stderr),
      };
    };
    let rejected = false;
    try {
      await __moonTest.createMoonContextEngine(api).commitTurn(params);
    } catch {
      rejected = true;
    }
    assert(rejected);
    const replay = __moonTest.createMoonContextEngine(api);
    assertEquals(await replay.commitTurn(params), { status: "duplicate" });
    const changed = {
      ...params,
      messages: [params.messages[0], {
        role: "assistant",
        content: "Conflicting answer",
        timestamp: 200,
      }],
    };
    let conflict = false;
    try {
      await replay.commitTurn(changed);
    } catch {
      conflict = true;
    }
    assert(conflict);
    const next = {
      ...params,
      advancementKey: `${params.advancementKey}-concurrent`,
    };
    const outcomes = await Promise.all([
      __moonTest.createMoonContextEngine(api).commitTurn(next),
      __moonTest.createMoonContextEngine(api).commitTurn(next),
    ]);
    assertEquals(outcomes.map((result) => result.status).sort(), [
      "committed",
      "duplicate",
    ]);
  },
});

Deno.test("optional extraction failure does not undo an accepted evidence commit", async () => {
  const params = acceptedParams([
    { role: "user", content: "Remember my preference for tea", timestamp: 100 },
    { role: "assistant", content: "Understood", timestamp: 200 },
  ]);
  const evidenceId = __moonTest.acceptedTurnFromParams(params)
    ?.evidenceSessionId;
  let records = 0;
  const api = createApi({ code: 0, stdout: "", stderr: "" }, [], {
    failOpen: false,
  });
  api.runtime.system.runCommandWithTimeout = (argv) => {
    if (argv.includes("record")) {
      records += 1;
      return {
        code: 0,
        stdout: JSON.stringify({
          session_id: evidenceId,
          changed: records === 1,
        }),
        stderr: "",
      };
    }
    if (argv.includes("context")) {
      throw new Error("optional extraction unavailable");
    }
    return {
      code: 0,
      stdout: JSON.stringify({ event_id: METRIC_REQUEST_ID }),
      stderr: "",
    };
  };
  const engine = __moonTest.createMoonContextEngine(api);
  assertEquals(await engine.commitTurn(params), { status: "committed" });
  assertEquals(await engine.commitTurn(params), { status: "duplicate" });
});

Deno.test("a turn without a visible final answer commits as an idempotent no-op", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const api = createApi({ code: 1, stdout: "", stderr: "must not run" }, calls);
  const engine = __moonTest.createMoonContextEngine(api);
  const params = acceptedParams([
    { role: "user", content: "Hello", timestamp: 100 },
    {
      role: "assistant",
      content: [{ type: "thinking", thinking: "private reasoning" }],
      timestamp: 200,
    },
  ]);
  assertEquals(await engine.commitTurn(params), { status: "committed" });
  assertEquals(calls, []);
});

Deno.test("L1 and L2 settings are independent and preserve the native Codex host route", async () => {
  const runs: Array<Record<string, unknown>> = [];
  const api = createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    {},
    (params) => {
      runs.push(params);
      return { payloads: [{ text: '{"actions":[]}' }] };
    },
  );
  Object.assign(api.config, {
    plugins: {
      entries: {
        codex: {
          config: {
            appServer: {
              homeScope: "user",
              command: "/Applications/ChatGPT.app/Contents/Resources/codex",
            },
          },
        },
      },
    },
  });
  const base = __moonTest.resolveSettings(api);
  const config = learningConfig(
    {
      model: "openai/gpt-6-astra",
      reasoning: "low",
      fallback_enabled: false,
      timeout_ms: 120_000,
      max_output_tokens: 8192,
    },
    {
      enabled: true,
      model: "openai/gpt-6-astra",
      reasoning: "xhigh",
      fallback_enabled: false,
      timeout_ms: 600_000,
      max_output_tokens: 32768,
    },
  );
  for (const stage of ["l1", "l2"]) {
    const settings = __moonTest.stageSettings(base, config, stage);
    await __moonTest.runModelWithFallback(api, settings, "Return JSON", {
      maxTokens: settings.maxOutputTokens,
    });
  }
  assertEquals(runs.map((run) => run.thinkLevel), ["low", "xhigh"]);
  assertEquals(runs.map((run) => run.timeoutMs), [120_000, 600_000]);
  assertEquals(runs.map((run) => run.streamParams), [{ maxTokens: 8192 }, {
    maxTokens: 32768,
  }]);
  for (const run of runs) {
    assert(
      run.config === api.config,
      "native host configuration must pass through unchanged",
    );
    assertEquals(run.provider, "openai");
    assertEquals(run.model, "gpt-6-astra");
    assertEquals(run.modelFallbacksOverride, []);
    assertEquals(run.sessionPersistence, "detached");
    assertEquals(run.toolsAllow, []);
    assertEquals(run.disableTools, true);
    assert(!("authProfile" in run) && !("apiKey" in run) && !("env" in run));
  }
  assertEquals(base.primaryModel, "vllm/local-primary");
  assertEquals(
    __moonTest.stageSettings(
      base,
      learningConfig({ model: "openai/gpt-6-astra" }),
      "l1",
    ).primaryReasoning,
    "low",
  );
});

Deno.test("learning evidence preserves negation, hypothetical meaning and historical tense", () => {
  for (
    const [claim, quote] of [
      ["Astrofaith API key is stored.", "No Astrofaith API key is stored."],
      [
        "Diana has Virgo on the second-house cusp.",
        "If Diana had Virgo on the second-house cusp, this would be a hypothetical example.",
      ],
      [
        "Astrofaith uses Live Room credentials.",
        "Astrofaith previously used Live Room credentials, which are now retired.",
      ],
      ["服务已经修复。", "假设服务已经修复。"],
      ["密钥已经存储。", "密钥没有存储。"],
    ]
  ) assert(!__moonTest.evidenceSupportsContent(claim, quote), claim);
  assert(
    __moonTest.evidenceSupportsContent(
      "No Astrofaith API key is stored.",
      "No Astrofaith API key is stored.",
    ),
  );
  assert(
    __moonTest.evidenceSupportsContent(
      "Astrofaith Live Room is retired.",
      "Astrofaith Live Room is retired.",
    ),
  );
});

Deno.test("correction detection includes retirement and Chinese without creating a competing claim", () => {
  const settings = __moonTest.resolveSettings(
    createApi({ code: 0, stdout: "", stderr: "" }, []),
  );
  for (
    const text of [
      "The Live Room has been retired.",
      "Please see any updates because this has been retired.",
      "更正一下，Live Room 已停用。",
      "我们不再使用这个模型。",
    ]
  ) {
    assert(__moonTest.correctionRequested(text));
    assert(
      __moonTest.isLearningCandidate({ userText: text, assistantText: "OK" }),
    );
  }
  const quote = "Actually, the release colour is violet.";
  const raw = {
    canonical_key: "project:colour",
    kind: "fact",
    content: "The release colour is violet.",
    evidence_quote: quote,
    importance: 0.9,
    confidence: 0.99,
    supersedes_document_id: 77,
  };
  const turn = {
    userText: quote,
    transcript: `User:\n${quote}`,
    completedAtMs: 1000,
  };
  assertEquals(
    __moonTest.normalizeProposal(raw, turn, settings, new Set()),
    null,
  );
  assertEquals(
    __moonTest.normalizeProposal(raw, turn, settings, new Set([77]))
      ?.supersedesDocumentId,
    77,
  );
});

Deno.test("changed canonical keys cannot bypass assistant echo and exploration guards", () => {
  const settings = __moonTest.resolveSettings(
    createApi({ code: 0, stdout: "", stderr: "" }, []),
  );
  const quote = "Diana has Scorpio on the second-house cusp.";
  const raw = {
    canonical_key: "diana:new-key",
    kind: "fact",
    content: quote,
    evidence_quote: quote,
    importance: 0.9,
    confidence: 0.99,
  };
  const old = [{
    document_id: 42,
    canonical_key: "diana:old-key",
    content: quote,
  }];
  for (
    const userText of [
      "Tell me about Diana.",
      "I am just exploring different signs on the cusp.",
    ]
  ) {
    const turn = {
      userText,
      transcript: `User:\n${userText}\n\nAssistant:\n${quote}`,
      completedAtMs: 1000,
    };
    assertEquals(
      __moonTest.normalizeProposal(
        raw,
        turn,
        settings,
        new Set([42]),
        new Set(["diana:old-key"]),
        old,
      ),
      null,
    );
  }
});

Deno.test("temporary observations expire from evidence time while durable preferences remain", () => {
  const settings = {
    ...__moonTest.resolveSettings(
      createApi({ code: 0, stdout: "", stderr: "" }, []),
    ),
    observationTtlHours: 12,
  };
  const quote = "The Proton service is working now.";
  const raw = {
    canonical_key: "proton:status",
    kind: "fact",
    content: quote,
    evidence_quote: quote,
    importance: 0.9,
    confidence: 0.99,
  };
  const turn = {
    userText: quote,
    transcript: `User:\n${quote}`,
    completedAtMs: 1000,
  };
  const result = __moonTest.normalizeProposal(raw, turn, settings, new Set());
  assertEquals(result?.kind, "observation");
  assertEquals(result?.validUntilMs, 43_201_000);
  const preference = "I currently prefer concise answers.";
  const stable = __moonTest.normalizeProposal(
    {
      ...raw,
      kind: "preference",
      content: preference,
      evidence_quote: preference,
    },
    { ...turn, userText: preference, transcript: `User:\n${preference}` },
    settings,
    new Set(),
  );
  assertEquals(stable?.validUntilMs, null);
  const legacy = __moonTest.normalizeProposal(
    { ...raw, kind: "observation" },
    turn,
    settings,
    new Set([42]),
    new Set([raw.canonical_key]),
    [{
      document_id: 42,
      canonical_key: raw.canonical_key,
      kind: "fact",
      content: quote,
    }],
  );
  assertEquals(legacy?.kind, "fact");
  assertEquals(legacy?.validUntilMs, 43_201_000);
});

Deno.test("elliptical retrieval follows the preceding user topic without recycling assistant recall", () => {
  const messages = [
    {
      role: "user",
      content: "How do you interpret Cancer on the second-house cusp?",
    },
    {
      role: "assistant",
      content: "Unrelated discussion of Diana and other people",
    },
    { role: "user", content: "How about Leo?" },
  ];
  const query = __moonTest.queryFromParams({ messages });
  assert(query.includes("second-house cusp"));
  assert(query.includes("Leo"));
  assert(!query.includes("Diana"));
  assertEquals(
    __moonTest.queryFromParams({
      messages,
      prompt: "What is the Proton status?",
    }),
    "What is the Proton status?",
  );
});

Deno.test("daily synthesis cutoffs handle Sydney catch-up and both DST transitions", () => {
  for (
    const [now, at, key, cutoff] of [
      ["2026-09-13T19:00:00Z", "03:00", "2026-09-14", "2026-09-13T17:00:00Z"],
      ["2026-09-13T16:00:00Z", "03:00", "2026-09-13", "2026-09-12T17:00:00Z"],
      ["2026-10-03T16:15:00Z", "02:30", "2026-10-03", "2026-10-02T16:30:00Z"],
      ["2026-10-03T16:45:00Z", "02:30", "2026-10-04", "2026-10-03T16:30:00Z"],
      ["2026-04-04T16:45:00Z", "02:30", "2026-04-05", "2026-04-04T16:30:00Z"],
    ]
  ) {
    assertEquals(
      __moonTest.dailySynthesisWindow(Date.parse(now), at, "Australia/Sydney"),
      { key, cutoffMs: Date.parse(cutoff) },
    );
  }
});

function synthesisFixture() {
  const quote = "I prefer concise answers.";
  return {
    prepared: {
      status: "prepared",
      run_id: "run-1",
      scope: "global",
      evidence: [{
        session_id: "source:new",
        completed_at_ms: 1000,
        selected: true,
        content: `User:\n${quote}\n\nAssistant:\nUnderstood.`,
      }],
      memories: [],
    },
    action: {
      action: "create",
      canonical_key: "user:preference:style",
      kind: "preference",
      title: "Response style",
      content: "The user prefers concise answers.",
      importance: 0.9,
      confidence: 0.99,
      evidence: [{ session_id: "source:new", quote }],
    },
  };
}

Deno.test("L2 validates original citations and rejects unsupported changes even with custom guidance", () => {
  const { prepared, action } = synthesisFixture();
  const settings = __moonTest.stageSettings(
    __moonTest.resolveSettings(
      createApi({ code: 0, stdout: "", stderr: "" }, []),
    ),
    learningConfig(),
    "l2",
  );
  assertEquals(
    __moonTest.normalizeSynthesisResult(
      { actions: [action] },
      prepared,
      settings,
      16,
    ).actions.length,
    1,
  );
  for (
    const changed of [
      { ...action, content: "The user prefers 500-word answers." },
      {
        ...action,
        evidence: [{ session_id: "forged", quote: action.evidence[0].quote }],
      },
      {
        ...action,
        evidence: [{ session_id: "source:new", quote: "The user likes cake." }],
      },
      { ...action, action: "supersede", target_document_id: 999 },
    ]
  ) {
    let rejected = false;
    try {
      __moonTest.normalizeSynthesisResult(
        { actions: [changed] },
        prepared,
        settings,
        16,
      );
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
  const prompt = __moonTest.synthesisPrompt(prepared, {
    ...settings,
    promptText: "Focus on concise preferences.",
  }, 16);
  assert(prompt.includes("Focus on concise preferences."));
  assert(prompt.includes("untrusted data"));
  assert(prompt.includes("original evidence"));
  assert(prompt.includes("at least one selected=true"));
});

Deno.test("L2 rejects malformed structure and missing selected evidence before fallback validation completes", () => {
  const { prepared, action } = synthesisFixture();
  const settings = __moonTest.stageSettings(
    __moonTest.resolveSettings(
      createApi({ code: 0, stdout: "", stderr: "" }, []),
    ),
    learningConfig(),
    "l2",
  );
  const target = {
    document_id: 42,
    canonical_key: action.canonical_key,
    kind: action.kind,
    content: action.content,
  };
  const snapshot = { ...prepared, memories: [target] };
  const valid = { ...action, action: "confirm", target_document_id: 42 };
  const invalid = [
    { ...valid, target_document_id: undefined },
    { ...valid, target_document_id: "42" },
    { ...valid, target_document_id: 99 },
    { ...valid, canonical_key: "another:key" },
    { ...valid, content: "The user prefers very concise answers." },
    { ...valid, confidence: "0.99" },
    { ...valid, importance: null },
    { ...valid, title: 123 },
    { ...valid, extra: true },
    { ...valid, merge_document_ids: [42] },
    { ...valid, action: "create" },
    { ...valid, action: "merge", merge_document_ids: [] },
    { ...valid, action: "merge", merge_document_ids: [42] },
    { action: "review", target_document_id: 42, evidence: action.evidence },
  ];
  for (const proposal of invalid) {
    let rejected = false;
    try {
      __moonTest.normalizeSynthesisResult(
        { actions: [proposal] },
        snapshot,
        settings,
        16,
      );
    } catch {
      rejected = true;
    }
    assert(
      rejected,
      `accepted malformed proposal: ${JSON.stringify(proposal)}`,
    );
  }
  for (
    const modified of [
      {
        ...snapshot,
        evidence: snapshot.evidence.map((item) => ({
          ...item,
          selected: false,
        })),
      },
    ]
  ) {
    let rejected = false;
    try {
      __moonTest.normalizeSynthesisResult(
        { actions: [valid] },
        modified,
        settings,
        16,
      );
    } catch {
      rejected = true;
    }
    assert(rejected);
  }
  const review = __moonTest.normalizeSynthesisResult(
    {
      actions: [{
        ...valid,
        action: "review",
        content: "The preference is uncertain.",
        confidence: 0.3,
      }],
    },
    snapshot,
    settings,
    16,
  );
  assertEquals(review.actions[0].action, "review");
});

Deno.test("L2 can expire a legacy operational workflow while retaining its kind", () => {
  const { prepared, action } = synthesisFixture();
  const quote = "The Proton service is working now.";
  const snapshot = {
    ...prepared,
    evidence: [{
      ...prepared.evidence[0],
      content: `User:\n${quote}\n\nAssistant:\nUnderstood.`,
    }],
    memories: [{
      document_id: 42,
      canonical_key: "proton:status",
      kind: "workflow",
      content: quote,
    }],
  };
  const settings = __moonTest.stageSettings(
    __moonTest.resolveSettings(
      createApi({ code: 0, stdout: "", stderr: "" }, []),
    ),
    learningConfig(),
    "l2",
  );
  const result = __moonTest.normalizeSynthesisResult(
    {
      actions: [{
        ...action,
        action: "confirm",
        target_document_id: 42,
        canonical_key: "proton:status",
        kind: "workflow",
        durability: "temporary",
        content: quote,
        evidence: [{ session_id: "source:new", quote }],
      }],
    },
    snapshot,
    settings,
    16,
  );
  assertEquals(result.actions[0].kind, "workflow");
  assertEquals(result.actions[0].valid_until_ms, 86_401_000);
  assert(!("durability" in result.actions[0]));
});

Deno.test("daily L2 skips committed batches and sends xhigh with a fixed evidence cutoff", async () => {
  const { prepared, action } = synthesisFixture();
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  let prepares = 0;
  let runs = 0;
  const api = createApi(
    { code: 0, stdout: "", stderr: "" },
    calls,
    {},
    (params) => {
      runs += 1;
      assertEquals(params.thinkLevel, "xhigh");
      assertEquals(params.model, "gpt-6-astra");
      assertEquals(params.sessionPersistence, "detached");
      return { payloads: [{ text: JSON.stringify({ actions: [action] }) }] };
    },
  );
  api.runtime.system.runCommandWithTimeout = (argv, options) => {
    calls.push({ argv, timeoutMs: options.timeoutMs, input: options.input });
    let result;
    if (argv.includes("prepare")) {
      result = prepares++ === 0
        ? { status: "committed" }
        : prepares === 2
        ? prepared
        : { status: "empty" };
    } else if (argv.includes("apply")) {
      result = { status: "committed", action_count: 1, processed_evidence: 1 };
    } else throw new Error("unexpected command");
    return { code: 0, stdout: JSON.stringify(result), stderr: "" };
  };
  const config = learningConfig({}, {
    enabled: true,
    model: "openai/gpt-6-astra",
    reasoning: "xhigh",
    fallback_enabled: false,
  });
  const result = await __moonTest.runDailySynthesis(
    api,
    __moonTest.resolveSettings(api),
    config,
    Date.parse("2026-09-13T19:00:00Z"),
  );
  assertEquals(result, { status: "empty", batches: 1 });
  assertEquals(runs, 1);
  const windows = calls.filter((call) => call.argv.includes("prepare")).map((
    call,
  ) => call.argv[call.argv.indexOf("--before-ms") + 1]);
  assertEquals(new Set(windows).size, 1);
  const payload = JSON.parse(
    calls.find((call) => call.argv.includes("apply"))?.input ?? "{}",
  );
  assertEquals(payload.metadata.model, "openai/gpt-6-astra");
  assertEquals(payload.metadata.reasoning, "xhigh");
  assert(/^[a-f0-9]{64}$/.test(payload.metadata.prompt_hash));
  assert(!("prompt" in payload.metadata));
});

Deno.test("failed L2 releases its lease and never applies an invalid proposal", async () => {
  const { prepared } = synthesisFixture();
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const api = createApi(
    { code: 0, stdout: "", stderr: "" },
    calls,
    {},
    () => ({ payloads: [{ text: '{"actions":[{"action":"delete"}]}' }] }),
  );
  api.runtime.system.runCommandWithTimeout = (argv, options) => {
    calls.push({ argv, timeoutMs: options.timeoutMs });
    return {
      code: 0,
      stdout: JSON.stringify(
        argv.includes("prepare") ? prepared : { status: "failed" },
      ),
      stderr: "",
    };
  };
  let rejected = false;
  try {
    await __moonTest.runDailySynthesis(
      api,
      __moonTest.resolveSettings(api),
      learningConfig({}, { enabled: true, fallback_enabled: false }),
      Date.now(),
    );
  } catch {
    rejected = true;
  }
  assert(rejected);
  assert(calls.some((call) => call.argv.includes("fail")));
  assert(!calls.some((call) => call.argv.includes("apply")));
});

Deno.test("Astra inherits supported explicit effort and question-only correction evidence is rejected", () => {
  const api = createApi({ code: 0, stdout: "", stderr: "" }, [], {
    primaryModel: "openai/gpt-6-astra",
    primaryReasoning: "xhigh",
    fallbackModel: "openai/gpt-6-astra-fallback",
    fallbackReasoning: "max",
  });
  const base = __moonTest.resolveSettings(api);
  const settings = __moonTest.stageSettings(base, learningConfig(), "l1");
  assertEquals(settings.primaryReasoning, "xhigh");
  assertEquals(settings.fallbackReasoning, "max");
  const userText = "Actually, is the Proton service working?";
  const raw = {
    canonical_key: "proton:status",
    kind: "fact",
    content: "The Proton service is working.",
    evidence_quote: userText,
    importance: 0.9,
    confidence: 0.99,
    supersedes_document_id: 4,
  };
  assertEquals(
    __moonTest.normalizeProposal(
      raw,
      {
        userText,
        transcript: `User:\n${userText}\n\nAssistant:\nYes.`,
        completedAtMs: 1000,
      },
      settings,
      new Set([4]),
    ),
    null,
  );
  assert(
    __moonTest.evidenceSupportsContent(
      "When TLS fails, check the certificate.",
      "When TLS fails, check the certificate.",
    ),
  );
});

Deno.test({
  name:
    "real SQLite L1 capture and daily L2 correction retain evidence and resume idempotently",
  ignore: !realMoonConfig,
  fn: async () => {
    assert(realMoonConfig);
    const home = `${realMoonConfig.home}/learning-${crypto.randomUUID()}`;
    const baseArgs = [
      "--home",
      home,
      "--database",
      `${home}/state/moon.sqlite`,
      "--dimensions",
      "64",
      "--json",
    ];
    const execute = async (
      argv: string[],
      options: { input?: string } = {},
    ) => {
      const child = new Deno.Command(argv[0], {
        args: argv.slice(1),
        stdin: "piped",
        stdout: "piped",
        stderr: "piped",
      }).spawn();
      const writer = child.stdin.getWriter();
      await writer.write(new TextEncoder().encode(options.input ?? ""));
      await writer.close();
      const output = await child.output();
      return {
        code: output.code,
        stdout: new TextDecoder().decode(output.stdout),
        stderr: new TextDecoder().decode(output.stderr),
      };
    };
    const cli = async (args: string[], input?: string) => {
      const result = await execute([
        realMoonConfig.binary,
        ...baseArgs,
        ...args,
      ], { input });
      assertEquals(result.code, 0);
      return JSON.parse(result.stdout);
    };
    await cli(["config", "init"]);
    const oldQuote = "Remember: The release colour for Project Canary is blue.";
    const newQuote =
      "Actually, the release colour for Project Canary is violet.";
    const key = "project:canary:release-colour";
    let l1Runs = 0;
    let l2Runs = 0;
    let prepared: Record<string, unknown> | null = null;
    const api = createApi({ code: 0, stdout: "", stderr: "" }, [], {
      moonPath: realMoonConfig.binary,
      moonHome: home,
      dimensions: 64,
    }, (params) => {
      if (String(params.prompt).includes("L2 memory curator")) {
        l2Runs += 1;
        assertEquals(params.model, "gpt-6-astra");
        assertEquals(params.thinkLevel, "xhigh");
        assert(prepared);
        const memories = prepared.memories as Array<Record<string, unknown>>;
        const evidence = prepared.evidence as Array<Record<string, unknown>>;
        const target = memories.find((memory) => memory.canonical_key === key);
        const source = evidence.find((item) =>
          String(item.content).includes(newQuote)
        );
        assert(target && source);
        return {
          payloads: [{
            text: JSON.stringify({
              actions: [{
                action: "supersede",
                canonical_key: key,
                kind: "fact",
                title: "Canary colour",
                content: "The release colour for Project Canary is violet.",
                importance: 0.9,
                confidence: 0.99,
                target_document_id: target.document_id,
                evidence: [{ session_id: source.session_id, quote: newQuote }],
              }],
            }),
          }],
        };
      }
      l1Runs += 1;
      assertEquals(params.model, "gpt-6-astra");
      assertEquals(params.thinkLevel, "low");
      return {
        payloads: [{
          text: JSON.stringify(
            l1Runs === 1
              ? {
                eligible: true,
                memories: [{
                  canonical_key: key,
                  kind: "fact",
                  content: "The release colour for Project Canary is blue.",
                  evidence_quote: oldQuote,
                  importance: 0.9,
                  confidence: 0.99,
                }],
              }
              : { eligible: false, memories: [] },
          ),
        }],
      };
    });
    api.runtime.system.runCommandWithTimeout = async (argv, options) => {
      const result = await execute(argv, options);
      if (result.code === 0 && argv.includes("prepare")) {
        prepared = JSON.parse(result.stdout);
      }
      return result;
    };
    const engine = __moonTest.createMoonContextEngine(api);
    for (const [index, quote] of [oldQuote, newQuote].entries()) {
      const time = Date.parse("2026-09-10T00:00:00Z") + index * 86_400_000;
      const params = acceptedParams([{
        role: "user",
        content: quote,
        timestamp: time,
      }, {
        role: "assistant",
        content: "Understood.",
        timestamp: time + 1000,
      }]);
      params.advancementKey = `l2-sqlite-${index}`;
      assertEquals(await engine.commitTurn(params), { status: "committed" });
    }
    const before = await cli(["health"]);
    assertEquals(before.evidence_sessions, 2);
    assertEquals(before.active_memories, 1);
    const config = await cli(["config", "show"]);
    config.learning.l2.enabled = true;
    config.learning.l2.fallback_enabled = false;
    const now = Date.parse("2026-09-13T19:00:00Z");
    assertEquals(
      (await __moonTest.runDailySynthesis(
        api,
        __moonTest.resolveSettings(api),
        config,
        now,
      )).batches,
      1,
    );
    await __moonTest.runDailySynthesis(
      api,
      __moonTest.resolveSettings(api),
      config,
      now,
    );
    assertEquals(l1Runs, 2);
    assertEquals(l2Runs, 1);
    const status = await cli(["learning", "status"]);
    assertEquals(status.processed_evidence, 2);
    const packet = await cli([
      "context",
      "--query",
      "Project Canary release colour",
      "--mode",
      "lexical",
    ]);
    assertEquals(packet.memories.length, 1);
    assert(packet.memories[0].content.includes("violet"));
    assert(!packet.memories[0].content.includes("blue"));
    await cli(["embed", "--provider", "hash"]);
    const health = await cli(["health"]);
    assertEquals(health.ok, true);
    assertEquals(health.evidence_sessions, 2);
    assertEquals(health.active_memories, 1);
    assertEquals(health.active_memory_vectors, 1);
    assertEquals(health.evidence_vectors, 0);
    engine.dispose();
  },
});

import moonPlugin, { __moonTest } from "./index.js";

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
  const api = createApi({ code: 0, stdout: packet, stderr: "" }, calls);
  const engine = __moonTest.createMoonContextEngine(api);
  const messages = [
    { role: "assistant", content: [{ type: "text", text: "Earlier answer" }] },
    { role: "user", content: [{ type: "text", text: "Recall SQLite plan" }] },
  ];

  const result = await engine.assemble({ messages });
  assertEquals(calls.length, 1);
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
    { code: 0, stdout: "", stderr: "" },
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
  assertEquals(emptyCalls.length, 1);
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
      learningModel: "gpt-5.6-luna",
      learningReasoning: "medium",
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
          throw new Error(`unexpected command ${argv.join(" ")}`);
        },
      },
      llm: {
        complete(params: { model: string }) {
          modelCalls += 1;
          assertEquals(params.model, "openai/gpt-5.6-luna");
          return {
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
            provider: "openai",
            model: "gpt-5.6-luna",
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
  assertEquals(calls.length, 3);
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
});

Deno.test("plugin registers gateway-lifecycle ownership for the warm worker", async () => {
  const services: Record<string, unknown>[] = [];
  const engineFactories: Array<() => unknown> = [];
  moonPlugin.register({
    registerService(value: Record<string, unknown>) {
      services.push(value);
    },
    registerContextEngine(_id: string, factory: () => unknown) {
      engineFactories.push(factory);
    },
  });
  const service = services[0];
  assertEquals(service.id, "moon-local-embedding-worker");
  assert(typeof service.start === "function");
  assert(typeof service.stop === "function");
  assert(typeof engineFactories[0] === "function");
  await (service.stop as () => Promise<void>)();
});

Deno.test("model auth uses OpenClaw first", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const api = createApi(
    { code: 1, stdout: "", stderr: "should not run" },
    calls,
    {},
    () => ({ payloads: [{ text: "READY" }] }),
  );
  const settings = __moonTest.resolveSettings(api);
  const result = await __moonTest.runModelWithAuthFallback(
    api,
    settings,
    "Return READY.",
    { sessionFile: "/tmp/moon-test-session.jsonl" },
  );
  assertEquals(result.authLevel, "openclaw");
  assertEquals(result.reasoning, "high");
  assertEquals(result.output, "READY");
  assertEquals(calls.length, 0);
});

Deno.test("model auth falls back through Moon without putting prompts in argv", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const api = createApi(
    {
      code: 0,
      stdout: JSON.stringify({
        auth_level: "codex",
        model: "gpt-5.6-sol",
        reasoning: "high",
        output: "READY",
      }),
      stderr: "",
    },
    calls,
    {},
    () => {
      throw new Error("OAuth expired");
    },
  );
  const settings = __moonTest.resolveSettings(api);
  const prompt = "private canary prompt";
  const result = await __moonTest.runModelWithAuthFallback(
    api,
    settings,
    prompt,
    { sessionFile: "/tmp/moon-test-session.jsonl" },
  );
  assertEquals(result.authLevel, "codex");
  assertEquals(calls.length, 1);
  assertEquals(calls[0].input, prompt);
  assert(!calls[0].argv.includes(prompt));
  assertEquals(
    calls[0].argv.slice(-7),
    [
      "--json",
      "auth",
      "exec",
      "--model",
      "gpt-5.6-sol",
      "--reasoning",
      "high",
    ],
  );
});

Deno.test("luna defaults to medium reasoning", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
    { codexModel: "gpt-5.6-luna" },
  ));
  assertEquals(settings.codexReasoning, "medium");
});

Deno.test("learning settings default to Luna medium with a smaller packet", () => {
  const settings = __moonTest.resolveSettings(createApi(
    { code: 0, stdout: "", stderr: "" },
    [],
  ));
  assertEquals(settings.learningModel, "gpt-5.6-luna");
  assertEquals(settings.learningReasoning, "medium");
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

Deno.test("model auth does not credential-hop on non-auth failures", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number; input?: string }> =
    [];
  const api = createApi(
    { code: 0, stdout: "", stderr: "" },
    calls,
    {},
    () => {
      throw new Error("rate limit status 429");
    },
  );
  const settings = __moonTest.resolveSettings(api);
  let message = "";
  try {
    await __moonTest.runModelWithAuthFallback(
      api,
      settings,
      "Return READY.",
      { sessionFile: "/tmp/moon-test-session.jsonl" },
    );
  } catch (error) {
    message = String(error);
  }
  assert(message.includes("429"));
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

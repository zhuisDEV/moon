import { __moonTest } from "./index.js";
import {
  nativeOnlyOpenAiRunner,
  officialOpenAiPolicy,
} from "./fixtures/openclaw-2026.9.7-model-policy.ts";

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function apiFor(
  runner: (params: Record<string, unknown>) => unknown,
  resolvePolicy?: (params: Record<string, unknown>) => unknown,
) {
  return {
    config: { agents: { defaults: { model: "openai/gpt-6-astra" } } },
    pluginConfig: {
      moonPath: "/tmp/bin/moon",
      moonHome: "/tmp/moon-model-diagnostic-fixture",
      embeddingEnabled: false,
    },
    resolvePath: (path: string) => path,
    runtime: {
      modelConfig: { resolveModelRuntimePolicy: resolvePolicy },
      agent: { runEmbeddedAgent: runner },
    },
  };
}

async function modelRun(
  api: ReturnType<typeof apiFor>,
  modelRef = "openai/gpt-6-astra",
) {
  return await __moonTest.runOpenClawModel(
    api,
    __moonTest.resolveSettings(api),
    "Return READY",
    {
      modelRef,
      reasoning: "low",
      route: "primary",
      sessionKey: "agent:research:fixture",
      maxTokens: 4096,
    },
  );
}

Deno.test("output hint must not disqualify native OpenAI authentication", async () => {
  equal(officialOpenAiPolicy({ maxTokens: 4096 }), {
    runtime: "openclaw",
    auth: "rejected",
  });
  equal(officialOpenAiPolicy(undefined), {
    runtime: "codex",
    auth: "deferred",
  });
  const api = apiFor(nativeOnlyOpenAiRunner);
  const outcome = await modelRun(api);
  equal(outcome.output, "READY");
  equal(outcome.model, "openai/gpt-6-astra");
});

Deno.test("explicit host token limits and native runtime policy keep their owners", async () => {
  for (
    const { model, runtime, expected } of [
      { model: "openai/gpt-6-astra", runtime: "openclaw", expected: 4096 },
      { model: "vllm/local", runtime: "openclaw", expected: 4096 },
      { model: "openai/gpt-6-astra", runtime: "codex", expected: undefined },
      { model: "anthropic/fixture", runtime: "claude", expected: undefined },
      { model: "openai/gpt-6-astra", runtime: "auto", expected: undefined },
      { model: "openai/gpt-6-astra", runtime: "default", expected: undefined },
      { model: "vllm/local", runtime: undefined, expected: 4096 },
    ]
  ) {
    const runs: Record<string, unknown>[] = [];
    const policies: Record<string, unknown>[] = [];
    const api = apiFor((params) => {
      runs.push(params);
      return { payloads: [{ text: "READY" }] };
    }, (params) => {
      policies.push(params);
      return runtime ? { policy: { id: runtime } } : {};
    });
    await modelRun(api, model);
    equal(runs[0].streamParams, expected ? { maxTokens: expected } : undefined);
    equal(policies[0].agentId, "research");
    equal(policies[0].provider, model.split("/")[0]);
    equal(policies[0].modelId, model.split("/")[1]);
    equal(policies[0].sessionKey, runs[0].sessionKey);
    equal(policies[0].config === api.config, true);
    equal(runs[0].config === api.config, true);
    equal(runs[0].agentHarnessRuntimeOverride, undefined);
    equal(runs[0].toolsAllow, []);
    equal(runs[0].disableTools, true);
    equal(runs[0].sessionPersistence, "detached");
  }
});

Deno.test("uncertain runtime policy omits the optional token hint without leaking errors", async () => {
  const api = apiFor(nativeOnlyOpenAiRunner, () => {
    throw new Error("private host policy failure");
  });
  equal((await modelRun(api)).output, "READY");
});

Deno.test("fallback recomputes the output hint for its own provider", async () => {
  const runs: Record<string, unknown>[] = [];
  const api = apiFor((params) => {
    runs.push(params);
    if (runs.length === 1) throw new Error("synthetic primary failure");
    return { payloads: [{ text: "READY" }] };
  });
  const settings = {
    ...__moonTest.resolveSettings(api),
    fallbackModel: "vllm/local",
  };
  const outcome = await __moonTest.runModelWithFallback(
    api,
    settings,
    "READY",
    {
      maxTokens: 4096,
    },
  );
  equal(outcome.modelRoute, "fallback");
  equal(runs.map((run) => run.streamParams), [undefined, { maxTokens: 4096 }]);
});

function acceptedTurn() {
  const admission = {
    agentId: "main",
    sessionId: "session-1",
    sessionKey: "agent:main:fixture",
    storePath: "/tmp/moon-model-diagnostic-fixture/openclaw.sqlite",
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
    messages: [
      { role: "user", content: "Remember I prefer tea", timestamp: 100 },
      { role: "assistant", content: "Understood", timestamp: 200 },
    ],
  };
}

Deno.test("L1 reports safe phase and code while preserving evidence and replay idempotency", async () => {
  const privateText = "PRIVATE_PROVIDER_BODY_AND_CREDENTIAL";
  for (
    const { failure, expected } of [
      { failure: "model", expected: "phase=model code=backend_error" },
      { failure: "config", expected: "phase=config code=invalid_response" },
      { failure: "context", expected: "phase=internal code=unknown" },
    ]
  ) {
    const logs: string[] = [];
    const commands: string[][] = [];
    let records = 0;
    let modelCalls = 0;
    const params = acceptedTurn();
    const evidenceId = __moonTest.acceptedTurnFromParams(params)
      ?.evidenceSessionId;
    if (!evidenceId) throw new Error("synthetic accepted turn was rejected");
    const base = apiFor(() => {
      modelCalls += 1;
      throw Object.assign(new Error(privateText), {
        phase: "normalise",
        code: "missing_selected_evidence",
      });
    });
    const api = {
      ...base,
      logger: { error: (message: string) => logs.push(message) },
      runtime: {
        ...base.runtime,
        system: {
          runCommandWithTimeout(argv: string[]) {
            commands.push(argv);
            let result: unknown = {
              event_id: "0123456789abcdef0123456789abcdef",
            };
            if (argv.includes("record")) {
              result = { session_id: evidenceId, changed: ++records === 1 };
            } else if (argv.includes("config")) {
              result = failure === "config" ? { privateText } : {
                learning: {
                  observation_ttl_hours: 24,
                  l1: { enabled: true, fallback_enabled: false },
                  l2: { enabled: false, timezone: "Australia/Sydney" },
                },
              };
            } else if (argv.includes("context")) {
              if (failure === "context") throw new Error(privateText);
              result = { memories: [], references: [] };
            }
            return {
              code: 0,
              stdout: JSON.stringify(result),
              stderr: privateText,
            };
          },
        },
      },
    };
    const engine = __moonTest.createMoonContextEngine(api);
    equal(await engine.commitTurn(params), { status: "committed" });
    equal(await engine.commitTurn(params), { status: "duplicate" });
    equal(records, 2);
    equal(modelCalls, failure === "model" ? 1 : 0);
    equal(logs, [
      `moon learning degraded ${expected}; inspect moon config validate and moon learning status`,
    ]);
    equal(JSON.stringify(logs).includes(privateText), false);
    equal(
      commands.some((argv) =>
        argv.includes("record-runtime") && argv.includes("error")
      ),
      true,
    );
  }
});

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

Deno.test("model execution distinguishes requested settings from returned identity", async () => {
  for (const backend of ["openclaw", "codex", "claude"]) {
    const api = apiFor(() => ({
      meta: {
        agentMeta: {
          provider: "OpenAI",
          model: "OPENAI/gpt-6-astra",
          agentHarnessId: backend,
          fallbackAttempts: [],
          // Neither field is in the current host's server-effort contract.
          thinkLevel: "low",
          reasoningEffort: "low",
          credentialSource: { token: "PRIVATE_METADATA_SECRET" },
        },
      },
      payloads: [{ text: "READY" }],
    }));
    const result = await modelRun(api);
    equal(result.execution, {
      requested: { provider: "openai", model: "gpt-6-astra", reasoning: "low" },
      observed: {
        provider: "openai",
        model: "gpt-6-astra",
        backend,
        reasoning: null,
        fallback_attempts: 0,
      },
      identity: "verified",
    });
    equal(JSON.stringify(result).includes("PRIVATE_METADATA_SECRET"), false);
  }
});

Deno.test("missing host observations stay unknown rather than claiming successful settings", async () => {
  for (
    const [meta, identity, provider, model] of [
      [{}, "unknown", null, null],
      [{ provider: "openai" }, "partial", "openai", null],
      [{ model: "gpt-6-astra" }, "partial", null, "gpt-6-astra"],
    ] as const
  ) {
    const result = await modelRun(apiFor(() => ({
      meta: {
        agentMeta: { ...meta, agentHarnessId: "PRIVATE_UNKNOWN_BACKEND" },
      },
      payloads: [{ text: "READY" }],
    })));
    equal(result.execution.identity, identity);
    equal(result.execution.observed, {
      provider,
      model,
      backend: null,
      reasoning: null,
      fallback_attempts: null,
    });
  }
});

Deno.test("confirmed model changes and unexpected host fallback never reach validation or fallback", async () => {
  const privateValue = "PRIVATE_UNEXPECTED_MODEL_OR_ERROR";
  for (
    const agentMeta of [
      { provider: "other", model: "gpt-6-astra" },
      { provider: "openai", model: privateValue },
      { provider: "openai", model: 12 },
      { provider: "openai", model: "gpt-6-astra", fallbackAttempts: [{}] },
      { provider: "openai", model: "gpt-6-astra", fallbackUsed: true },
      { fallbackAttempts: { secret: privateValue } },
      {
        provider: "openai",
        model: "gpt-6-astra",
        runtimeModelSelection: { provider: "openai", model: privateValue },
      },
    ]
  ) {
    let runs = 0;
    let validations = 0;
    const api = apiFor(() => {
      runs += 1;
      return { meta: { agentMeta }, payloads: [{ text: "UNREVIEWED" }] };
    });
    const settings = {
      ...__moonTest.resolveSettings(api),
      primaryModel: "openai/gpt-6-astra",
      fallbackModel: "vllm/local",
    };
    let caught: unknown;
    try {
      await __moonTest.runModelWithFallback(api, settings, "READY", {
        validateOutput() {
          validations += 1;
        },
      });
    } catch (error) {
      caught = error;
    }
    equal(__moonTest.failureDiagnostic(caught), {
      phase: "model",
      code: "model_route_mismatch",
      dispatch: "completed",
    });
    equal(runs, 1);
    equal(validations, 0);
    equal(String(caught).includes(privateValue), false);
  }
});

Deno.test("known runtime admission failures are safe, terminal, and do not claim dispatch knowledge", async () => {
  const privateValue = "PRIVATE_AUTH_OR_PROVIDER_BODY";
  for (
    const [error, code] of [
      [
        Object.assign(new Error(privateValue), {
          code: "AUTH_PROFILE_INCOMPATIBLE",
        }),
        "authentication_failed",
      ],
      [
        Object.assign(new Error(privateValue), { name: "ProviderAuthError" }),
        "authentication_failed",
      ],
      [
        new Error(
          `Auth profile "${privateValue}" uses oauth auth, but openai/openai-responses requires an OpenAI API key profile.`,
        ),
        "authentication_failed",
      ],
      [
        Object.assign(new Error(privateValue), {
          name: "AgentHarnessPreflightError",
        }),
        "runtime_setup_failed",
      ],
      [
        new Error(
          "Plugin embedded-agent execution requires an active plugin runtime scope.",
        ),
        "runtime_setup_failed",
      ],
      [
        Object.assign(new Error(privateValue), { code: "MODEL_NOT_FOUND" }),
        "model_unavailable",
      ],
    ] as const
  ) {
    let runs = 0;
    const api = apiFor(() => {
      runs += 1;
      throw error;
    });
    let caught: unknown;
    try {
      await __moonTest.runModelWithFallback(api, {
        ...__moonTest.resolveSettings(api),
        fallbackModel: "vllm/local",
      }, "READY");
    } catch (failure) {
      caught = failure;
    }
    equal(runs, 1);
    equal(__moonTest.failureDiagnostic(caught), {
      phase: "model",
      code,
      dispatch: "unknown",
    });
    equal(String(caught).includes(privateValue), false);
  }
});

Deno.test("malformed identity and rerouted timeout metadata cannot trigger fallback", async () => {
  for (
    const meta of [
      { agentMeta: "PRIVATE_INVALID_METADATA" },
      {
        agentMeta: { provider: "openai", model: "unexpected-model" },
        timeoutPhase: "provider",
      },
      {
        agentMeta: { fallbackAttempts: [{}] },
        error: { code: "ETIMEDOUT", message: "PRIVATE_REMOTE_BODY" },
      },
    ]
  ) {
    let runs = 0;
    const api = apiFor(() => {
      runs += 1;
      return { meta, payloads: [{ text: "partial" }] };
    });
    let caught: unknown;
    try {
      await __moonTest.runModelWithFallback(api, {
        ...__moonTest.resolveSettings(api),
        fallbackModel: "vllm/local",
      }, "READY");
    } catch (failure) {
      caught = failure;
    }
    equal(runs, 1);
    equal(__moonTest.failureDiagnostic(caught).code, "model_route_mismatch");
    equal(String(caught).includes("PRIVATE_"), false);
  }
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
    idempotencyKey: "admission-1",
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
            if (argv.includes("reply-history")) {
              result = { received: true };
            } else if (argv.includes("audience")) {
              result = {
                owners: ["discord:111111111111111111"],
                audience: ["discord:111111111111111111"],
                sender: "discord:111111111111111111",
              };
            } else if (argv.includes("record")) {
              result = {
                session_id: evidenceId,
                scope: "private:test",
                changed: ++records === 1,
              };
            } else if (argv.includes("config")) {
              result = failure === "config" ? { privateText } : {
                learning: {
                  observation_ttl_hours: 24,
                  l1: { enabled: true, fallback_enabled: false },
                  l2: { enabled: false, timezone: "Australia/Sydney" },
                },
              };
            } else if (argv.includes("related")) {
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

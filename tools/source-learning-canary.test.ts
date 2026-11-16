// Offline contracts: no filesystem, subprocess, network, or provider permission.
import {
  adapterDiagnostic,
  assertExecution,
  assertModelRequest,
  CanaryError,
  cleanEnvironment,
  credentialGuard,
  executionOwnerInput,
  guardCommand,
  hookHostConfig,
  inspectPacket,
  MODEL,
  reserveCall,
  safeFailure,
  SCOPE,
  selectExecutionOwnerModule,
  SOURCES,
  stopOwnedChild,
  TOPIC,
} from "./source-learning-canary-contract.ts";
import { parseArgs, supervise } from "./source-learning-canary.ts";
import moonPlugin, {
  __moonTest as actualAdapter,
} from "../assets/openclaw-plugin/index.js";

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error("assertion_failed");
  }
}
function fails(fn: () => unknown, code: string) {
  try {
    fn();
  } catch (error) {
    equal(safeFailure(error), { error: code });
    return;
  }
  throw new Error("expected_failure");
}
async function rejects(fn: () => Promise<unknown>, code: string) {
  try {
    await fn();
  } catch (error) {
    equal(safeFailure(error), { error: code });
    return;
  }
  throw new Error("expected_rejection");
}
const execution = () => ({
  requested: { provider: "openai", model: MODEL, reasoning: "medium" },
  observed: {
    provider: "openai",
    model: MODEL,
    backend: "openclaw",
    reasoning: null,
    fallback_attempts: null,
  },
  identity: "verified",
});
const request = () => ({
  provider: "openai",
  model: MODEL,
  thinkLevel: "medium",
  agentId: "main",
  sessionPersistence: "detached",
  sessionKey: "agent:main:internal-session-effects:incognito-canary",
  workspaceDir: "/canary/workspace",
  disableTools: true,
  disableMessageTool: true,
  toolsAllow: [],
  modelFallbacksOverride: [],
  streamParams: undefined,
});

Deno.test("default preflight never authorises inference; explicit smoke is stage-specific", () => {
  equal(parseArgs([]).stage, "preflight");
  fails(() => parseArgs(["--stage", "l1"]), "smoke_flag_required");
  fails(() => parseArgs(["--smoke"]), "smoke_flag_requires_model_stage");
  equal(parseArgs(["--stage", "l2", "--smoke"]).smoke, true);
  fails(
    () => parseArgs(["--stage", "l2", "--stage", "l1"]),
    "duplicate_option",
  );
});
Deno.test("metadata verifies actual route without assuming native Codex or observed effort", () => {
  const value = execution();
  equal(assertExecution(value, "l1"), value);
  equal(value.observed.reasoning, null);
  value.observed.backend = "codex";
  assertExecution(value, "l1");
  value.observed.model = "gpt-wrong";
  fails(() => assertExecution(value, "l1"), "observed_model_mismatch");
  value.observed.model = MODEL;
  value.identity = "unknown";
  fails(() => assertExecution(value, "l1"), "observed_model_identity_missing");
  const fallback = {
    ...execution(),
    observed: { ...execution().observed, fallback_attempts: 1 },
  };
  fails(() => assertExecution(fallback, "l1"), "observed_fallback_attempt");
});
Deno.test("real runner arguments require incognito detached policy with no model tools or fallback", () => {
  assertModelRequest(request(), "l1", "/canary/workspace");
  for (
    const override of [
      { disableTools: false },
      { disableMessageTool: false },
      { toolsAllow: ["exec"] },
      { modelFallbacksOverride: ["other"] },
      { streamParams: {} },
    ]
  ) {
    fails(
      () =>
        assertModelRequest(
          { ...request(), ...override },
          "l1",
          "/canary/workspace",
        ),
      "model_tool_or_transport_policy_failed",
    );
  }
  fails(
    () =>
      assertModelRequest(
        { ...request(), workspaceDir: "/live" },
        "l1",
        "/canary/workspace",
      ),
    "model_session_isolation_failed",
  );
});
Deno.test("inference reservations survive failure and prohibit retries or fourth call", () => {
  const calls: ReturnType<typeof reserveCall>[] = [];
  reserveCall(calls, "l1", "one").status = "failed_or_uncertain";
  fails(() => reserveCall(calls, "l1", "one"), "inference_replay_refused");
  reserveCall(calls, "l1", "two");
  reserveCall(calls, "l2", "topic");
  fails(
    () => reserveCall(calls, "l2", "another"),
    "inference_budget_exhausted",
  );
});
Deno.test("Moon commands cannot escape temporary storage, source keys or scope", () => {
  const base = [
    "/candidate/moon",
    "--home",
    "/canary",
    "--database",
    "/canary/state/moon.sqlite",
  ];
  guardCommand(base, "/candidate/moon", "/canary");
  fails(
    () => guardCommand([...base, "--home", "/live"], base[0], "/canary"),
    "moon_storage_not_isolated",
  );
  fails(
    () => guardCommand([...base, "--scope", "personal"], base[0], "/canary"),
    "unexpected_scope",
  );
  fails(
    () => guardCommand([...base, "--key", "other"], base[0], "/canary"),
    "unexpected_source_or_topic",
  );
});
Deno.test("private config credential guard and clean env omit provider override injection", () => {
  for (
    const key of [
      "accessToken",
      "refresh_token",
      "apiKey",
      "password",
      "secret",
      "credentials",
    ]
  ) {
    fails(
      () => credentialGuard({ nested: { [key]: "SECRET" } }),
      "credential_material_in_selected_config",
    );
  }
  credentialGuard({
    auth: {
      profiles: { "openai:primary": { provider: "openai", mode: "oauth" } },
    },
  });
  const source: Record<string, string> = {
    HOME: "/home",
    PATH: "/bin",
    OPENAI_API_KEY: "SECRET",
    OPENAI_BASE_URL: "https://other",
    NODE_OPTIONS: "injection",
  };
  equal(cleanEnvironment((key) => source[key]), {
    PATH: "/bin",
    HOME: "/home",
  });
  equal(safeFailure(new Error("SECRET provider body")), {
    error: "operation_failed",
  });
});

function citation(index: number) {
  const source = SOURCES[index];
  const quote = source.content.split("\n")[0];
  return {
    source_key: source.key,
    start_byte: 0,
    end_byte: new TextEncoder().encode(quote).length,
    quote,
  };
}
Deno.test("topic own citations must contain both original source premises", () => {
  const packet = {
    used_chars: 100,
    memories: [],
    references: [{
      scope: SCOPE,
      topic: {
        key: TOPIC.key,
        statements: [{
          text: "Question",
          citations: [citation(0), citation(1)],
        }],
      },
    }],
  };
  equal(inspectPacket(packet, true).citations, 2);
  packet.references[0].topic.statements[0].citations.pop();
  fails(() => inspectPacket(packet, true), "topic_missing_premise");
  packet.references[0].topic.statements[0].citations[0].quote = "unsupported";
  fails(() => inspectPacket(packet, false), "citation_bytes_mismatch");
});

const stream = (text: string) =>
  new ReadableStream<Uint8Array>({
    start(c) {
      c.enqueue(new TextEncoder().encode(text));
      c.close();
    },
  });
function fakeHost(record: unknown, success = true) {
  return {
    stdout: stream(
      "foreign diagnostic SECRET\n" + JSON.stringify(record) + "\n",
    ),
    stderr: stream("SECRET remote error body"),
    status: Promise.resolve({ success, code: success ? 0 : 1, signal: null }),
    kill() {},
  };
}
Deno.test("host protocol projects safe fields and suppresses arbitrary diagnostics", async () => {
  const result = await supervise(
    fakeHost({
      protocol: "moon-source-canary-v1",
      status: "passed",
      stage: "SECRET",
      modelCalls: 0,
      nativeAuthUnchanged: true,
      catalogue: "read_only",
      cleanup: { remainingOwnedChildren: 0, servicesSettled: true },
      extra: "SECRET",
    }),
    100,
  );
  equal(JSON.stringify(result).includes("SECRET"), false);
  equal(result.modelCalls, 0);
});
Deno.test("host setup failure is actionable; arbitrary foreign failure text is discarded", async () => {
  await rejects(
    () =>
      supervise(
        fakeHost({
          protocol: "moon-source-canary-v1",
          status: "failed",
          error: "unsupported_host_version_review_required",
        }, false),
        100,
      ),
    "unsupported_host_version_review_required",
  );
  await rejects(
    () =>
      supervise(
        fakeHost({
          protocol: "moon-source-canary-v1",
          status: "failed",
          error: "SECRET foreign error",
        }, false),
        100,
      ),
    "host_bridge_failed",
  );
});
Deno.test("timeout terminates the owned host and closes output streams", async () => {
  let resolve!: (
    status: { success: boolean; code: number; signal: null },
  ) => void;
  const signals: string[] = [];
  const child = {
    stdout: new ReadableStream<Uint8Array>(),
    stderr: new ReadableStream<Uint8Array>(),
    status: new Promise<{ success: boolean; code: number; signal: null }>((r) =>
      resolve = r
    ),
    kill(signal: string) {
      signals.push(signal);
      resolve({ success: false, code: 143, signal: null });
    },
  };
  await rejects(() => supervise(child, 1), "host_stage_timeout");
  equal(signals, ["SIGTERM"]);
});
Deno.test("cleanup escalates only its own child when grace expires", async () => {
  let resolve!: () => void;
  const signals: string[] = [];
  const child = {
    status: new Promise<void>((r) => resolve = r),
    kill(signal: "SIGTERM" | "SIGKILL") {
      signals.push(signal);
      if (signal === "SIGKILL") resolve();
    },
  };
  equal(await stopOwnedChild(child, 1), "killed");
  equal(signals, ["SIGTERM", "SIGKILL"]);
});
Deno.test("safe errors carry authored codes only", () => {
  equal(safeFailure(new CanaryError("inference_not_authorised")), {
    error: "inference_not_authorised",
  });
});

Deno.test("already reaped children are never signalled again", async () => {
  let calls = 0;
  equal(
    await stopOwnedChild({
      status: Promise.resolve(),
      kill() {
        calls++;
      },
    }),
    "already_exited",
  );
  equal(calls, 0);
});
Deno.test("duplicate protocol records and oversized output cannot pass", async () => {
  await rejects(
    () =>
      supervise({
        ...fakeHost({ protocol: "moon-source-canary-v1" }),
        stdout: stream(
          '{"protocol":"moon-source-canary-v1"}\n{"protocol":"moon-source-canary-v1"}\n',
        ),
      }, 100),
    "host_protocol_missing_or_duplicated",
  );
  await rejects(
    () =>
      supervise({
        ...fakeHost({}),
        stdout: stream("x".repeat(1024 * 1024 + 1)),
      }, 100),
    "host_output_limit",
  );
});
Deno.test("inline storage override is rejected", () => {
  const argv = [
    "/candidate/moon",
    "--home",
    "/canary",
    "--database",
    "/canary/state/moon.sqlite",
    "--database=/live.sqlite",
  ];
  fails(
    () => guardCommand(argv, argv[0], "/canary"),
    "moon_storage_not_isolated",
  );
});

Deno.test("private owner capability requires one explicit named export", () => {
  const good = {
    name: "runtime-build.mjs",
    content: "export { acquireAgentRunPreparedModelRuntime, other };",
  };
  equal(selectExecutionOwnerModule([good]), good.name);
  fails(
    () => selectExecutionOwnerModule([]),
    "execution_owner_capability_missing",
  );
  fails(
    () =>
      selectExecutionOwnerModule([{
        name: "alias.mjs",
        content: "export { acquireAgentRunPreparedModelRuntime as t };",
      }]),
    "execution_owner_capability_missing",
  );
  fails(
    () =>
      selectExecutionOwnerModule([good, { ...good, name: "duplicate.mjs" }]),
    "execution_owner_capability_ambiguous",
  );
});
Deno.test("exact owner input matches execution selection instead of catalogue-only owner", () => {
  const input = executionOwnerInput(
    { safe: true },
    "/host/agent",
    "/canary/workspace",
  );
  equal(input.runtimePluginSelections, [{
    provider: "openai",
    modelId: MODEL,
    agentId: "main",
  }]);
  equal(input.preserveWorkspaceDirOnRefresh, false);
  equal(input.agentId, "main");
});
Deno.test("adapter-authored diagnostic retains unknown dispatch and drops all text", async () => {
  // Mimic the adapter's WeakMap capability: foreign properties cannot register
  // themselves as trusted diagnostics, even if they resemble its public shape.
  const localErrors = new WeakMap<Error, unknown>();
  const genuine = new Error("SECRET body not for persistence");
  localErrors.set(genuine, {
    phase: "model",
    code: "authentication_failed",
    detail: "SECRET",
  });
  const read = (error: Error) =>
    localErrors.get(error) ?? { phase: "internal", code: "unknown" };
  const diagnostic = adapterDiagnostic(read(genuine));
  equal(diagnostic, {
    phase: "model",
    code: "authentication_failed",
    dispatch: "unknown",
  });
  equal(
    adapterDiagnostic({
      phase: "model",
      code: "SECRET",
      dispatch: "completed",
    }),
    undefined,
  );
  equal(adapterDiagnostic({ phase: "__proto__", code: "SECRET" }), undefined);
  try {
    await supervise(
      fakeHost({
        protocol: "moon-source-canary-v1",
        status: "failed",
        error: "adapter_failure",
        adapter_failure: diagnostic,
        message: "SECRET",
      }, false),
      100,
    );
    throw new Error("expected_rejection");
  } catch (error) {
    equal(safeFailure(error), {
      error: "adapter_failure",
      adapter_failure: diagnostic,
    });
    equal(JSON.stringify(safeFailure(error)).includes("SECRET"), false);
  }
});

Deno.test("actual adapter classifies original host failure without exposing its body", async () => {
  const api = {
    config: { agents: { defaults: { model: { primary: "openai/" + MODEL } } } },
    pluginConfig: {
      moonPath: "/candidate/moon",
      moonHome: "/canary",
      mode: "lexical",
      embeddingEnabled: false,
    },
    resolvePath: (value: string) => value,
    runtime: {
      agent: {
        runEmbeddedAgent() {
          const error = new Error("SECRET provider payload");
          Object.assign(error, { code: "AUTH_PROFILE_INCOMPATIBLE" });
          throw error;
        },
      },
    },
  };
  let failure;
  try {
    await actualAdapter.runOpenClawModel(
      api,
      actualAdapter.resolveSettings(api),
      "Synthetic prompt",
      {
        modelRef: "openai/" + MODEL,
        reasoning: "medium",
        route: "primary",
      },
    );
  } catch (error) {
    failure = error;
  }
  equal(adapterDiagnostic(actualAdapter.failureDiagnostic(failure)), {
    phase: "model",
    code: "authentication_failed",
    dispatch: "unknown",
  });
  const forged = Object.assign(new Error("SECRET"), {
    phase: "model",
    code: "authentication_failed",
    dispatch: "completed",
  });
  equal(adapterDiagnostic(actualAdapter.failureDiagnostic(forged)), {
    phase: "internal",
    code: "unknown",
    dispatch: "unknown",
  });
});

Deno.test("canary hook config selects Moon so the actual registered hook delivers its packet", async () => {
  const packet = "# Moon Context\n\nSynthetic Aster evidence packet";
  const hooks = new Map<
    string,
    (...args: unknown[]) => Promise<{ prependContext?: string }>
  >();
  const settings = {
    moonPath: "/candidate/moon",
    moonHome: "/canary",
    mode: "lexical",
    scope: SCOPE,
    maxChars: 8000,
    limit: 8,
    embeddingEnabled: false,
    learningEnabled: false,
    failOpen: false,
  };
  let contextRequests = 0;
  moonPlugin.register({
    pluginConfig: settings,
    config: hookHostConfig(settings),
    resolvePath: (p: string) => p,
    logger: { debug() {}, info() {}, warn() {}, error() {} },
    runtime: {
      system: {
        runCommandWithTimeout(argv: string[]) {
          const context = argv.includes("context");
          if (context) contextRequests++;
          return {
            code: 0,
            stderr: "",
            stdout: JSON.stringify(
              context
                ? {
                  request_id: "0123456789abcdef0123456789abcdef",
                  packet,
                  memory_count: 0,
                  reference_count: 1,
                  packet_chars: packet.length,
                  truncated: false,
                }
                : { ok: true },
            ),
          };
        },
      },
    },
    on: (
      name: string,
      hook: (...args: unknown[]) => Promise<{ prependContext?: string }>,
    ) => hooks.set(name, hook),
    registerTool() {},
    registerService() {},
    registerContextEngine() {},
    registerCompactionProvider() {},
  });
  const result = await hooks.get("before_prompt_build")!({
    currentUserMessage: "Explain Aster restart readiness and operator approval",
    currentUserMessageId: "synthetic-turn",
  }, {
    agentId: "main",
    sessionKey: "agent:main:direct:canary",
    runId: "canary-read",
    hookInvocation: { assertActive() {} },
  });
  equal(contextRequests, 1);
  equal(result.prependContext, packet);
});

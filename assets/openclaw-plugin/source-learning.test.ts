import moonPlugin, { __moonTest } from "./index.js";

const JOB = `slj_${"1".repeat(32)}`;
const TOKEN = `slt_${"2".repeat(32)}`;
const HASH = "3".repeat(64);
const SCOPE = "project:source-canary";
const QUOTE = "Canary uses SQLite.";

function assert(value: unknown, message = "assertion failed"): asserts value {
  if (!value) throw new Error(message);
}

function equals(actual: unknown, expected: unknown) {
  assert(JSON.stringify(actual) === JSON.stringify(expected), "values differ");
}

async function rejects(run: () => unknown, code?: string) {
  try {
    await run();
  } catch (error) {
    const text = String(error);
    assert(!text.includes("synthetic-sensitive-value"));
    if (code) assert(text.includes(code), "unexpected fixed error code");
    return;
  }
  throw new Error("expected rejection");
}

type Call = {
  argv: string[];
  options: { timeoutMs: number; input?: string; signal?: AbortSignal };
};

function fixture(options: {
  l1?: Record<string, unknown>;
  plugin?: Record<string, unknown>;
  packet?: Record<string, unknown>;
  model?: (params: Record<string, unknown>, count: number) => unknown;
} = {}) {
  const calls: Call[] = [];
  const models: Array<Record<string, unknown>> = [];
  const context = {
    senderIsOwner: true,
    agentId: "main",
    sessionKey: "agent:main:direct:source-test",
    workspaceDir: "/tmp/source-test-workspace",
    assertInvocationCurrent() {},
    assertInputCommitAllowed() {},
  };
  const config = {
    learning: {
      observation_ttl_hours: 24,
      l1: {
        enabled: true,
        model: "openai/gpt-6.1-terra",
        reasoning: "medium",
        timeout_ms: 15_000,
        fallback_enabled: false,
        ...options.l1,
      },
      l2: { enabled: false, timezone: "Australia/Sydney" },
    },
  };
  const api = {
    config: {},
    pluginConfig: {
      moonPath: "/tmp/source-test-moon",
      moonHome: "/tmp/source-test-home",
      primaryModel: "vllm/base-model",
      timeoutMs: 1000,
      ...options.plugin,
    },
    resolvePath: (path: string) => path,
    runtime: {
      system: {
        runCommandWithTimeout(
          argv: string[],
          opts: Call["options"],
        ):
          | { code: number; stdout: string; stderr: string }
          | Promise<{ code: number; stdout: string; stderr: string }> {
          calls.push({ argv, options: opts });
          let result;
          if (argv.includes("config")) result = config;
          else if (argv.includes("prepare")) {
            result = options.packet ?? {
              schema_version: 1,
              status: "prepared",
              job_id: JOB,
              lease_token: TOKEN,
              source: {
                scope: SCOPE,
                source_key: "canary",
                kind: "external_document",
                content: QUOTE,
              },
            };
          } else if (argv.includes("submit")) {
            result = {
              schema_version: 1,
              status: "review",
              job_id: JOB,
              result_hash: HASH,
            };
          } else if (argv.includes("fail")) result = { status: "queued" };
          else throw new Error("unexpected source command");
          return { code: 0, stdout: JSON.stringify(result), stderr: "" };
        },
      },
      agent: {
        runEmbeddedAgent(params: Record<string, unknown>) {
          models.push(params);
          return options.model?.(params, models.length) ?? {
            payloads: [{
              text: JSON.stringify({
                schema_version: 1,
                claims: [{ quote: QUOTE }],
              }),
            }],
          };
        },
      },
    },
    logger: { error() {} },
  };
  const tool = __moonTest.createSourceLearningTool(api, context);
  assert(tool);
  const execute = (input: unknown = { scope: SCOPE }, signal?: AbortSignal) =>
    tool.execute("source-test", input, signal);
  return { api, context, calls, models, execute };
}

Deno.test("source tool registers optionally with trusted V2 owner context", () => {
  const { api, context } = fixture();
  const registered: Array<Record<string, unknown>> = [];
  moonPlugin.register({
    ...api,
    registerTool(factory: Record<string, unknown>, options: unknown) {
      registered.push({ factory, options });
    },
    registerContextEngine() {},
    registerCompactionProvider() {},
    registerService() {},
  });
  equals(registered.length, 2);
  equals(registered[0].options, {
    name: "moon_source_learn",
    optional: true,
    workspaceAccess: false,
  });
  const factory = registered[0].factory as {
    contextVersion: number;
    create: (value: unknown) => unknown;
  };
  equals(factory.contextVersion, 2);
  assert(factory.create(context));
  equals(factory.create({ ...context, senderIsOwner: false }), null);
  equals(factory.create({ senderIsOwner: true }), null);
});

Deno.test("source tool honours L1 routing and submits only a bounded review proposal", async () => {
  const test = fixture();
  const result = await test.execute({ scope: SCOPE, source_key: "canary" });
  equals(result.details, {
    schema_version: 1,
    status: "review",
    job_id: JOB,
    result_hash: HASH,
    proposal_count: 1,
    automatic_acceptance: false,
    model_execution: {
      requested: {
        provider: "openai",
        model: "gpt-6.1-terra",
        reasoning: "medium",
      },
      observed: {
        provider: null,
        model: null,
        backend: null,
        reasoning: null,
        fallback_attempts: null,
      },
      identity: "unknown",
    },
  });
  const text = JSON.stringify(result);
  assert(!text.includes(TOKEN) && !text.includes(QUOTE));
  equals(test.calls.length, 3);
  const prepare = test.calls[1].argv;
  equals(prepare.slice(prepare.indexOf("source")), [
    "source",
    "learning",
    "prepare",
    "--scope",
    SCOPE,
    "--max-bytes",
    "65536",
    "--lease-ms",
    "27000",
    "--key",
    "canary",
  ]);
  for (const call of test.calls) {
    assert(call.argv.includes("--home") && call.argv.includes("--database"));
    assert(
      !call.argv.some((part) =>
        ["accept", "reject", "delete-apply"].includes(part)
      ),
    );
    assert(!call.argv.includes(QUOTE));
  }
  equals(JSON.parse(test.calls[2].options.input!), {
    schema_version: 1,
    claims: [{ quote: QUOTE }],
  });
  const model = test.models[0];
  equals(model.provider, "openai");
  equals(model.model, "gpt-6.1-terra");
  equals(model.thinkLevel, "medium");
  equals(model.timeoutMs, 15000);
  equals(model.disableTools, true);
  equals(model.disableMessageTool, true);
  equals(model.modelFallbacksOverride, []);
  equals(model.sessionPersistence, "detached");
  assert(String(model.prompt).includes("untrusted DATA"));
  assert(String(model.prompt).includes(QUOTE));
});

Deno.test("source setup preflight fails before claiming work", async () => {
  for (const cause of ["runtime", "owner"]) {
    const test = fixture();
    if (cause === "runtime") {
      Object.defineProperty(test.api.runtime.agent, "runEmbeddedAgent", {
        value: undefined,
      });
    } else test.context.agentId = "other-owner";
    await rejects(
      () => test.execute(),
      cause === "runtime" ? "runtime_unavailable" : "runtime_setup_failed",
    );
    equals(
      test.calls.filter((call) => call.argv.includes("prepare")).length,
      0,
    );
    equals(test.models.length, 0);
  }
});

Deno.test("source failures retain safe categories and never submit model mismatches", async () => {
  for (
    const [cause, code] of [
      ["auth", "authentication_failed"],
      ["timeout", "model_timeout"],
      ["mismatch", "model_route_mismatch"],
    ]
  ) {
    const test = fixture({
      model() {
        if (cause === "auth") {
          throw Object.assign(new Error("synthetic-sensitive-value"), {
            code: "AUTH_PROFILE_INCOMPATIBLE",
          });
        }
        if (cause === "timeout") {
          throw Object.assign(new Error("synthetic-sensitive-value"), {
            name: "TimeoutError",
          });
        }
        return {
          meta: { agentMeta: { provider: "openai", model: "different-model" } },
          payloads: [{
            text: JSON.stringify({ schema_version: 1, claims: [] }),
          }],
        };
      },
    });
    await rejects(() => test.execute(), code);
    equals(test.models.length, 1);
    equals(test.calls.at(-1)?.argv.slice(-2), ["--error-code", code]);
    equals(test.calls.some((call) => call.argv.includes("submit")), false);
    assert(!JSON.stringify(test.calls).includes("synthetic-sensitive-value"));
  }
});

Deno.test("source tool rejects unknown and unsafe request fields before any I/O", async () => {
  for (
    const input of [
      {},
      { scope: SCOPE, accept: true },
      { scope: SCOPE, lease_token: TOKEN },
      { scope: "synthetic-sensitive-value\n" },
      { scope: SCOPE, source_key: "bad key" },
      { scope: "x".repeat(257) },
    ]
  ) {
    const test = fixture();
    await rejects(() => test.execute(input));
    equals(test.calls.length, 0);
    equals(test.models.length, 0);
  }
  const test = fixture({ plugin: { moonHome: "" } });
  await rejects(() => test.execute());
  equals(test.calls.length, 0);
});

Deno.test("source tool does not claim jobs when disabled and skips unavailable work", async () => {
  for (
    const options of [
      { l1: { enabled: false } },
      { plugin: { learningEnabled: false } },
    ]
  ) {
    const test = fixture(options);
    equals((await test.execute()).details.status, "disabled");
    equals(test.calls.length, 1);
    equals(test.models.length, 0);
  }
  for (const status of ["idle", "busy", "input_too_large"]) {
    const test = fixture({
      packet: { schema_version: 1, status, lease_token: TOKEN },
    });
    const result = await test.execute();
    equals(result.details.status, status);
    assert(!JSON.stringify(result).includes(TOKEN));
    equals(test.calls.length, 2);
    equals(test.models.length, 0);
  }
});

Deno.test("source tool uses only explicit fallback and records fixed model failures", async () => {
  const test = fixture({
    l1: {
      fallback_enabled: true,
      fallback_model: "vllm/canary-fallback",
      fallback_reasoning: "low",
    },
    model(_params, count) {
      if (count === 1) throw new Error("synthetic-sensitive-value");
      return { payloads: [{ text: '{"schema_version":1,"claims":[]}' }] };
    },
  });
  equals((await test.execute()).details.status, "review");
  equals(
    test.models.map((model) => [model.provider, model.model, model.thinkLevel]),
    [
      ["openai", "gpt-6.1-terra", "medium"],
      ["vllm", "canary-fallback", "low"],
    ],
  );
  const failed = fixture({
    model() {
      throw new Error("synthetic-sensitive-value");
    },
  });
  await rejects(() => failed.execute(), "model_failed");
  equals(failed.models.length, 1);
  equals(failed.calls.at(-1)?.argv.slice(-2), ["--error-code", "model_failed"]);
});

Deno.test("source tool refuses malformed, oversized or fabricated model output", async () => {
  for (
    const output of [
      "synthetic-sensitive-value",
      "x".repeat(1_048_577),
      JSON.stringify({ schema_version: 1, claims: [], accept: true }),
      JSON.stringify({
        schema_version: 1,
        claims: [{ quote: "A fabricated claim." }],
      }),
      JSON.stringify({
        schema_version: 1,
        claims: [{ quote: QUOTE, command: "delete" }],
      }),
    ]
  ) {
    const test = fixture({
      plugin: { timeoutMs: 300_000 },
      model() {
        return { payloads: [{ text: output }] };
      },
    });
    await rejects(() => test.execute(), "invalid_response");
    assert(!test.calls.some((call) => call.argv.includes("submit")));
    equals(test.calls.at(-1)?.argv.slice(-2), [
      "--error-code",
      "invalid_response",
    ]);
    equals(test.calls.at(-1)?.options.timeoutMs, 5000);
  }
});

Deno.test("source proposal validation enforces exact UTF-8 ranges and repeated-quote disambiguation", async () => {
  const parse = (claim: unknown, text: string) =>
    __moonTest.sourceToolProposal(
      JSON.stringify({ schema_version: 1, claims: [claim] }),
      text,
    );
  parse({ quote: "月", start_byte: 1 }, "A月B");
  parse({ quote: "same", start_byte: 5 }, "same same");
  await rejects(() => parse({ quote: "月", start_byte: 2 }, "A月B"));
  await rejects(() => parse({ quote: "same" }, "same same"));
  await rejects(() => parse({ quote: " A" }, " A"));
  await rejects(() => parse({ quote: QUOTE, valid_until_ms: -1 }, QUOTE));
});

Deno.test("source tool fences revoked or cancelled calls and releases only its own lease", async () => {
  for (const cancellation of ["authority", "signal"]) {
    const controller = new AbortController();
    let current = true;
    const test = fixture({
      plugin: { timeoutMs: 300_000 },
      model() {
        if (cancellation === "signal") controller.abort();
        else current = false;
        return { payloads: [{ text: '{"schema_version":1,"claims":[]}' }] };
      },
    });
    test.context.assertInvocationCurrent = () => {
      if (!current) throw new Error("synthetic-sensitive-value");
    };
    await rejects(
      () => test.execute({ scope: SCOPE }, controller.signal),
      "interrupted",
    );
    assert(!test.calls.some((call) => call.argv.includes("submit")));
    const cleanup = test.calls.at(-1)!;
    assert(cleanup.argv.includes("fail") && cleanup.argv.includes(TOKEN));
    equals(cleanup.options.signal, undefined);
    equals(cleanup.options.timeoutMs, 1000);
    equals(cleanup.argv.slice(-2), ["--error-code", "interrupted"]);
  }
});

Deno.test("source tool cannot disclose to fallback after primary revokes authority", async () => {
  let current = true;
  const test = fixture({
    l1: { fallback_enabled: true, fallback_model: "vllm/canary-fallback" },
    model() {
      current = false;
      throw new Error("synthetic-sensitive-value");
    },
  });
  test.context.assertInvocationCurrent = () => {
    if (!current) throw new Error("synthetic-sensitive-value");
  };
  await rejects(() => test.execute(), "interrupted");
  equals(test.models.length, 1);
  assert(!test.calls.some((call) => call.argv.includes("submit")));
  equals(test.calls.at(-1)?.argv.slice(-2), ["--error-code", "interrupted"]);
});

async function optionalEnv(name: string) {
  const permission = await Deno.permissions.query({
    name: "env",
    variable: name,
  });
  return permission.state === "granted" ? Deno.env.get(name) : undefined;
}
const binary = await optionalEnv("MOON_TEST_BINARY");
const rootHome = await optionalEnv("MOON_TEST_HOME");
const requireReal = await optionalEnv("MOON_REQUIRE_REAL_BINARY") === "1";
if (requireReal && (!binary || !rootHome)) {
  throw new Error("required source tests need an isolated CLI fixture");
}

Deno.test({
  name:
    "real SQLite source tool creates durable review without accepting or recalling claims",
  ignore: !binary || !rootHome,
  async fn() {
    assert(binary && rootHome);
    const home = `${rootHome}/source-tool-${crypto.randomUUID()}`;
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
      const result = await child.output();
      return {
        code: result.code,
        stdout: new TextDecoder().decode(result.stdout),
        stderr: new TextDecoder().decode(result.stderr),
      };
    };
    const cli = async (args: string[], input?: string) => {
      const result = await execute([
        binary,
        "--home",
        home,
        "--database",
        `${home}/state/moon.sqlite`,
        "--dimensions",
        "64",
        "--json",
        ...args,
      ], { input });
      equals(result.code, 0);
      return JSON.parse(result.stdout);
    };
    await cli(
      ["source", "import", "--scope", SCOPE, "--allow-learning"],
      JSON.stringify({
        schema_version: 1,
        source_key: "canary",
        kind: "external_document",
        content: QUOTE,
      }),
    );
    const test = fixture({
      plugin: { moonPath: binary, moonHome: home, dimensions: 64 },
    });
    test.api.runtime.system.runCommandWithTimeout = async (argv, options) => {
      test.calls.push({ argv, options });
      return await execute(argv, options);
    };
    const result = await test.execute({ scope: SCOPE, source_key: "canary" });
    equals(result.details.status, "review");
    equals(test.models.length, 1);
    const status = await cli([
      "source",
      "learning",
      "status",
      "--scope",
      SCOPE,
      "--job-id",
      result.details.job_id,
    ]);
    equals(status.jobs[0].status, "review");
    equals(status.jobs[0].result_hash, result.details.result_hash);
    const claims = await cli(["source", "claims", "list", "--scope", SCOPE]);
    equals(claims.claims, []);
    assert(test.calls.every((call) => !call.argv.includes("accept")));
  },
});

import moonPlugin, { __moonTest } from "./index.js";

const SCOPE = "project:topic-canary";
const CLAIM = `scl_${"1".repeat(32)}`;
const TOPIC = `top_${"2".repeat(32)}`;
const JOB = `tcj_${"3".repeat(32)}`;
const TOKEN = `tct_${"4".repeat(32)}`;
const HASH = "5".repeat(64);
const QUOTE = "Canary uses SQLite, but migrations need a backup.";
const INPUTS = [{
  input_id: CLAIM,
  kind: "claim",
  claim_kind: "author_statement",
  content: QUOTE,
  citations: [],
}];
const PROPOSAL = {
  schema_version: 1,
  review_complete: true,
  statements: [{ kind: "fact", text: QUOTE, input_ids: [CLAIM] }],
};

function assert(value: unknown, message = "assertion failed"): asserts value {
  if (!value) throw new Error(message);
}
function equals(actual: unknown, expected: unknown) {
  assert(JSON.stringify(actual) === JSON.stringify(expected), "values differ");
}
async function rejects(fn: () => unknown, code?: string) {
  try {
    await fn();
  } catch (error) {
    assert(!String(error).includes("synthetic-sensitive-value"));
    if (code) assert(String(error).includes(code));
    return;
  }
  throw new Error("expected rejection");
}
type Result = { code: number; stdout: string; stderr: string };
type Call = {
  argv: string[];
  options: { input?: string; timeoutMs: number; signal?: AbortSignal };
};
function fixture(options: {
  packet?: Record<string, unknown>;
  l2?: Record<string, unknown>;
  plugin?: Record<string, unknown>;
  model?: (params: Record<string, unknown>, count: number) => unknown;
} = {}) {
  const calls: Call[] = [];
  const models: Array<Record<string, unknown>> = [];
  const context = {
    senderIsOwner: true,
    agentId: "main",
    sessionKey: "agent:main:topic-test",
    workspaceDir: "/tmp/topic-workspace",
    assertInvocationCurrent() {},
    assertInputCommitAllowed() {},
  };
  const api = {
    config: {},
    pluginConfig: {
      moonPath: "/tmp/topic-moon",
      moonHome: "/tmp/topic-home",
      timeoutMs: 1000,
      ...options.plugin,
    },
    resolvePath: (path: string) => path,
    logger: { error() {} },
    runtime: {
      system: {
        runCommandWithTimeout(
          argv: string[],
          callOptions: Call["options"],
        ): Result | Promise<Result> {
          calls.push({ argv, options: callOptions });
          let result;
          if (argv.includes("config")) {
            result = {
              learning: {
                l1: { enabled: false, model: "vllm/wrong-l1" },
                l2: {
                  enabled: true,
                  timezone: "Australia/Sydney",
                  model: "openai/gpt-6.1-terra",
                  reasoning: "max",
                  fallback_enabled: false,
                  timeout_ms: 120000,
                  ...options.l2,
                },
              },
            };
          } else if (argv.includes("prepare")) {
            result = options.packet ??
              {
                schema_version: 1,
                status: "prepared",
                job_id: JOB,
                lease_token: TOKEN,
                topic: {
                  topic_id: TOPIC,
                  scope: SCOPE,
                  key: "canary",
                  title: "Canary storage",
                  generation: 1,
                },
                inputs: INPUTS,
              };
          } else if (argv.includes("submit")) {
            result = {
              schema_version: 1,
              status: "review",
              job_id: JOB,
              result_hash: HASH,
            };
          } else if (argv.includes("fail")) result = { status: "queued" };
          else throw new Error("unexpected topic command");
          return { code: 0, stdout: JSON.stringify(result), stderr: "" };
        },
      },
      agent: {
        runEmbeddedAgent(params: Record<string, unknown>) {
          models.push(params);
          return options.model?.(params, models.length) ??
            { payloads: [{ text: JSON.stringify(PROPOSAL) }] };
        },
      },
    },
  };
  const tool = __moonTest.createTopicCompileTool(api, context);
  assert(tool);
  return {
    api,
    context,
    calls,
    models,
    execute: (
      input: unknown = { scope: SCOPE, topic_key: "canary" },
      signal?: AbortSignal,
    ) => tool.execute("topic-test", input, signal),
  };
}

Deno.test("topic tool registration matches manifest contracts and fails closed for untrusted owners", async () => {
  const manifest = JSON.parse(
    await Deno.readTextFile(new URL("./openclaw.plugin.json", import.meta.url)),
  );
  const { api, context } = fixture();
  const registrations: Array<Record<string, unknown>> = [];
  moonPlugin.register({
    ...api,
    registerContextEngine() {},
    registerCompactionProvider() {},
    registerService() {},
    registerTool(
      factory: { contextVersion: number },
      options: { name: string; optional: boolean },
    ) {
      assert(manifest.contracts.tools.includes(options.name));
      equals(manifest.toolMetadata[options.name].optional, options.optional);
      equals(factory.contextVersion, 2);
      registrations.push({ factory, options });
    },
  });
  equals(registrations.length, 2);
  equals(
    __moonTest.createTopicCompileTool(api, {
      ...context,
      senderIsOwner: false,
    }),
    null,
  );
  equals(__moonTest.createTopicCompileTool(api, { senderIsOwner: true }), null);
});

Deno.test("topic tool uses enabled L2 and submits typed statements for review only", async () => {
  const test = fixture();
  const result = await test.execute();
  equals(result.details, {
    schema_version: 1,
    status: "review",
    job_id: JOB,
    result_hash: HASH,
    statement_count: 1,
    review_complete: true,
    automatic_acceptance: false,
    model_execution: {
      requested: {
        provider: "openai",
        model: "gpt-6.1-terra",
        reasoning: "max",
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
  assert(
    !JSON.stringify(result).includes(TOKEN) &&
      !JSON.stringify(result).includes(QUOTE),
  );
  equals(test.calls.length, 3);
  equals(test.calls[1].argv.slice(test.calls[1].argv.indexOf("topic")), [
    "topic",
    "prepare",
    "--scope",
    SCOPE,
    "--max-bytes",
    "65536",
    "--lease-ms",
    "132000",
    "--key",
    "canary",
  ]);
  equals(JSON.parse(test.calls[2].options.input!), PROPOSAL);
  for (const call of test.calls) {
    assert(call.argv.includes("--database") && call.argv.includes("--home"));
    assert(
      !call.argv.some((part) =>
        ["accept", "reject", "delete-apply"].includes(part)
      ),
    );
    assert(!call.argv.includes(QUOTE));
  }
  const model = test.models[0];
  equals([model.provider, model.model, model.thinkLevel, model.timeoutMs], [
    "openai",
    "gpt-6.1-terra",
    "max",
    120000,
  ]);
  equals(model.disableTools, true);
  equals(model.sessionPersistence, "detached");
  assert(
    String(model.prompt).includes("untrusted DATA") &&
      String(model.prompt).includes("generated_statement"),
  );
});

Deno.test("topic model mismatch cannot become a review proposal", async () => {
  const test = fixture({
    model() {
      return {
        meta: {
          agentMeta: {
            provider: "openai",
            model: "gpt-6.1-terra",
            fallbackAttempts: [{ provider: "private-provider-body" }],
          },
        },
        payloads: [{ text: "private-provider-body" }],
      };
    },
  });
  await rejects(() => test.execute(), "model_route_mismatch");
  equals(test.models.length, 1);
  equals(test.calls.at(-1)?.argv.slice(-2), [
    "--error-code",
    "model_route_mismatch",
  ]);
  equals(test.calls.some((call) => call.argv.includes("submit")), false);
  assert(!JSON.stringify(test.calls).includes("private-provider-body"));
});

Deno.test("topic tool rejects unknown input and skips disabled, blocked or oversized work", async () => {
  for (
    const input of [{}, { scope: SCOPE, accept: true }, {
      scope: SCOPE,
      source_key: "canary",
    }, { scope: "synthetic-sensitive-value\n" }]
  ) {
    const test = fixture();
    await rejects(() => test.execute(input));
    equals(test.calls.length, 0);
  }
  const disabled = fixture({ l2: { enabled: false } });
  equals((await disabled.execute()).details.status, "disabled");
  equals(disabled.models.length, 0);
  for (const status of ["idle", "busy", "blocked", "input_too_large"]) {
    const test = fixture({
      packet: { schema_version: 1, status, lease_token: TOKEN },
    });
    equals((await test.execute()).details.status, status);
    equals(test.models.length, 0);
  }
  const oversized = fixture({
    packet: {
      schema_version: 1,
      status: "prepared",
      job_id: JOB,
      lease_token: TOKEN,
      topic: {
        topic_id: TOPIC,
        scope: SCOPE,
        key: "canary",
        title: "x".repeat(65536),
      },
      inputs: INPUTS,
    },
  });
  await rejects(() => oversized.execute());
  equals(oversized.models.length, 0);
  assert(oversized.calls.at(-1)?.argv.includes("fail"));
});

Deno.test("topic tool verifies scope before disclosure and accepts exact normalised aliases", async () => {
  const packet = {
    schema_version: 1,
    status: "prepared",
    job_id: JOB,
    lease_token: TOKEN,
    topic: {
      topic_id: TOPIC,
      scope: SCOPE,
      key: "canary",
      aliases: ["Canary Database", "金丝雀存储"],
    },
    inputs: INPUTS,
  };
  for (const topic_key of ["CANARY   DATABASE", "金丝雀存储"]) {
    const test = fixture({ packet });
    equals(
      (await test.execute({ scope: SCOPE, topic_key })).details.status,
      "review",
    );
  }
  for (
    const invalid of [{
      ...packet,
      topic: { ...packet.topic, scope: "other-scope" },
    }, packet]
  ) {
    const test = fixture({ packet: invalid });
    await rejects(() =>
      test.execute({
        scope: SCOPE,
        topic_key: invalid === packet ? "unknown alias" : "canary",
      })
    );
    equals(test.models.length, 0);
    assert(test.calls.at(-1)?.argv.includes("fail"));
  }
});

Deno.test("topic validation preserves exact facts and rejects generated-source laundering", async () => {
  const parse = (statements: unknown[], inputs: unknown[] = INPUTS) =>
    __moonTest.topicToolProposal(
      JSON.stringify({ schema_version: 1, review_complete: true, statements }),
      inputs,
    );
  for (
    const statement of [
      { kind: "fact", text: "Canary uses SQLite.", input_ids: [CLAIM] },
      { kind: "fact", text: QUOTE, input_ids: [] },
      {
        kind: "interpretation",
        text: "A backup may reduce risk.",
        input_ids: [],
      },
      { kind: "disagreement", text: "Accounts differ.", input_ids: [CLAIM] },
      { kind: "question", text: "Why?", input_ids: ["unknown"] },
      { kind: "question", text: "Why?", input_ids: [], accept: true },
    ]
  ) await rejects(() => parse([statement]));
  await rejects(() =>
    parse(PROPOSAL.statements, [{
      ...INPUTS[0],
      claim_kind: "generated_statement",
    }])
  );
  parse([{
    kind: "interpretation",
    text: "A backup may reduce risk.",
    input_ids: [CLAIM],
  }, { kind: "question", text: "Who verifies restoration?", input_ids: [] }]);
  parse([{ kind: "fact", text: QUOTE, input_ids: [TOPIC] }], [{
    input_id: TOPIC,
    kind: "topic",
    statements: PROPOSAL.statements,
  }]);
  await rejects(() =>
    parse([{ kind: "fact", text: QUOTE, input_ids: [TOPIC] }], [{
      input_id: TOPIC,
      kind: "topic",
      statements: [{ kind: "interpretation", text: QUOTE }],
    }])
  );
});

Deno.test("topic tool records safe failures and never submits invalid generated output", async () => {
  for (
    const output of [
      "synthetic-sensitive-value",
      "x".repeat(65537),
      JSON.stringify({ ...PROPOSAL, accept: true }),
      JSON.stringify({ ...PROPOSAL, statements: [] }),
    ]
  ) {
    const test = fixture({
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
  }
  const failed = fixture({
    model() {
      throw new Error("synthetic-sensitive-value");
    },
  });
  await rejects(() => failed.execute(), "model_failed");
});

Deno.test("topic tool rechecks authority before fallback and bounds cancelled cleanup", async () => {
  const controller = new AbortController();
  for (const mode of ["revoked", "cancelled"]) {
    let current = true;
    const test = fixture({
      plugin: { timeoutMs: 300000 },
      l2: { fallback_enabled: true, fallback_model: "vllm/topic-fallback" },
      model() {
        current = false;
        if (mode === "cancelled") controller.abort();
        throw new Error("synthetic-sensitive-value");
      },
    });
    test.context.assertInvocationCurrent = () => {
      if (!current) throw new Error("synthetic-sensitive-value");
    };
    await rejects(
      () => test.execute(undefined, controller.signal),
      "interrupted",
    );
    equals(test.models.length, 1);
    equals(test.calls.at(-1)?.options.timeoutMs, 1000);
    equals(test.calls.at(-1)?.options.signal, undefined);
    assert(!test.calls.some((call) => call.argv.includes("submit")));
  }
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
Deno.test({
  name: "real SQLite topic tool persists review without approving a revision",
  ignore: !binary || !rootHome,
  async fn() {
    assert(binary && rootHome);
    const home = `${rootHome}/topic-tool-${crypto.randomUUID()}`;
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
    const cli = async (args: string[], input?: unknown) => {
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
      ], { input: input === undefined ? undefined : JSON.stringify(input) });
      equals(result.code, 0);
      return JSON.parse(result.stdout);
    };
    await cli(["source", "import", "--scope", SCOPE, "--allow-learning"], {
      schema_version: 1,
      source_key: "canary-source",
      kind: "external_document",
      content: QUOTE,
    });
    const prepared = await cli([
      "source",
      "learning",
      "prepare",
      "--scope",
      SCOPE,
    ]);
    const submitted = await cli([
      "source",
      "learning",
      "submit",
      "--scope",
      SCOPE,
      "--job-id",
      prepared.job_id,
      "--lease-token",
      prepared.lease_token,
    ], { schema_version: 1, claims: [{ quote: QUOTE }] });
    const accepted = await cli([
      "source",
      "learning",
      "accept",
      "--scope",
      SCOPE,
      "--job-id",
      prepared.job_id,
      "--expected-result-hash",
      submitted.result_hash,
      "--yes",
    ]);
    await cli(["topic", "put", "--scope", SCOPE], {
      schema_version: 1,
      key: "canary",
      title: "Canary storage",
      aliases: ["Canary storage overview"],
      claim_ids: accepted.claim_ids,
    });
    const test = fixture({
      plugin: { moonPath: binary, moonHome: home, dimensions: 64 },
      model(params) {
        const text = String(params.prompt).split(
          "Untrusted prepared topic and inputs (JSON):\n",
        )[1];
        const packet = JSON.parse(text);
        return {
          payloads: [{
            text: JSON.stringify({
              ...PROPOSAL,
              statements: [{
                kind: "fact",
                text: QUOTE,
                input_ids: [packet.inputs[0].input_id],
              }],
            }),
          }],
        };
      },
    });
    const mockConfig = test.api.runtime.system.runCommandWithTimeout;
    test.api.runtime.system.runCommandWithTimeout = async (argv, options) => {
      if (argv.includes("config")) return await mockConfig(argv, options);
      test.calls.push({ argv, options });
      return await execute(argv, options);
    };
    const result = await test.execute({
      scope: SCOPE,
      topic_key: "CANARY storage   overview",
    });
    equals(result.details.status, "review");
    const status = await cli([
      "topic",
      "status",
      "--scope",
      SCOPE,
      "--job-id",
      result.details.job_id,
    ]);
    equals(status.jobs[0].status, "review");
    equals(status.jobs[0].result_hash, result.details.result_hash);
    const history = await cli([
      "topic",
      "history",
      "--scope",
      SCOPE,
      "--key",
      "canary",
    ]);
    equals(history.revisions, []);
    assert(!test.calls.some((call) => call.argv.includes("accept")));
  },
});

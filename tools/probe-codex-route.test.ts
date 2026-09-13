import {
  accountMode,
  advertisedModel,
  assertEphemeralStart,
  assertNoMcpTools,
  assertSmokeOutput,
  JsonLines,
  parseArgs,
  ProbeError,
  RpcClient,
  type RpcProcess,
  safeFailure,
  safeTokenUsage,
  smokeThreadParams,
  smokeTurn,
  verifyNativeGlobalInstructions,
} from "./probe-codex-route.ts";

function assert(value: unknown, message = "assertion failed"): asserts value {
  if (!value) throw new Error(message);
}

function equal(actual: unknown, expected: unknown) {
  assert(JSON.stringify(actual) === JSON.stringify(expected), "values differ");
}

function rejectsCode(run: () => unknown, code: string) {
  try {
    run();
  } catch (error) {
    assert(error instanceof ProbeError && error.code === code);
    return;
  }
  throw new Error("expected rejection");
}

class FakeProcess implements RpcProcess {
  private output!: ReadableStreamDefaultController<Uint8Array>;
  private errors!: ReadableStreamDefaultController<Uint8Array>;
  private stopped = false;
  private finish!: (status: { success: boolean; code: number }) => void;
  status = new Promise<{ success: boolean; code: number }>((resolve) =>
    this.finish = resolve
  );
  stdout = new ReadableStream<Uint8Array>({
    start: (controller) => this.output = controller,
  });
  stderr = new ReadableStream<Uint8Array>({
    start: (controller) => this.errors = controller,
  });
  stdin: WritableStream<Uint8Array>;
  requests: Record<string, unknown>[] = [];

  constructor(
    onRequest: (request: Record<string, unknown>, fake: FakeProcess) => void,
  ) {
    this.stdin = new WritableStream({
      write: (bytes) => {
        const request = JSON.parse(new TextDecoder().decode(bytes));
        this.requests.push(request);
        onRequest(request, this);
      },
    });
  }

  emit(value: unknown) {
    this.output.enqueue(new TextEncoder().encode(JSON.stringify(value) + "\n"));
  }

  emitStderr(bytes: Uint8Array) {
    this.errors.enqueue(bytes);
  }

  kill(_signal: Deno.Signal) {
    if (this.stopped) return;
    this.stopped = true;
    this.output.close();
    this.errors.close();
    this.finish({ success: false, code: 1 });
  }
}

Deno.test("options make inference opt-in and reject ambiguous or unbounded input", () => {
  equal(parseArgs([]), {
    codex: "codex",
    model: "gpt-6-astra",
    smoke: false,
    timeoutMs: 60_000,
  });
  assert(
    parseArgs(["--smoke", "--codex=/opt/codex", "--timeout-ms", "1000"]).smoke,
  );
  rejectsCode(() => parseArgs(["--smoke=false"]), "invalid_option");
  rejectsCode(() => parseArgs(["--model", "--smoke"]), "missing_option_value");
  rejectsCode(() => parseArgs(["--timeout-ms", "Infinity"]), "invalid_timeout");
  rejectsCode(() => parseArgs(["--timeout-ms", "120001"]), "invalid_timeout");
  rejectsCode(
    () => parseArgs(["--codex", "one", "--codex", "two"]),
    "duplicate_option",
  );
});

Deno.test("framing accepts fragmented UTF-8 and bounds incomplete lines and total output", () => {
  const bytes = new TextEncoder().encode(
    '{"value":"月"}\n{"id":2,"result":{}}\n',
  );
  const parser = new JsonLines();
  const messages = [];
  for (const byte of bytes) {
    messages.push(...parser.push(new Uint8Array([byte])));
  }
  equal(messages, [{ value: "月" }, { id: 2, result: {} }]);
  rejectsCode(
    () => new JsonLines(100, 3).push(new TextEncoder().encode("xxxx")),
    "line_limit",
  );
  rejectsCode(
    () => new JsonLines(3, 100).push(new TextEncoder().encode("xxxx")),
    "stdout_limit",
  );
  rejectsCode(
    () => new JsonLines().push(new TextEncoder().encode("private raw error\n")),
    "invalid_json_rpc",
  );
});

Deno.test("report projection drops account identities, unknown auth and raw errors", () => {
  equal(
    accountMode({
      account: {
        type: "chatgpt",
        email: "do-not-emit@example.test",
        accessToken: "DO_NOT_EMIT",
      },
    }),
    "chatgpt",
  );
  equal(accountMode({ account: { type: "DO_NOT_EMIT" } }), "unknown");
  equal(safeFailure(new Error("DO_NOT_EMIT")), { error: "probe_failed" });
  equal(
    advertisedModel({
      data: [{
        id: "gpt-6-astra",
        model: "gpt-6-astra",
        description: "DO_NOT_EMIT",
        supportedReasoningEfforts: [{
          reasoningEffort: "low",
          description: "DO_NOT_EMIT",
        }, { reasoningEffort: "xhigh" }],
      }],
    }, "gpt-6-astra"),
    {
      id: "gpt-6-astra",
      advertised: true,
      supported_reasoning: ["low", "xhigh"],
    },
  );
});

Deno.test("smoke admission requires ephemeral no-instructions read-only exact model", () => {
  const params = smokeThreadParams("/tmp/probe", "gpt-6-astra", [
    "private_mcp",
  ]);
  assert(params.ephemeral && params.allowProviderModelFallback === false);
  equal(params.environments, []);
  equal(params.dynamicTools, []);
  equal(params.runtimeWorkspaceRoots, []);
  equal(params.config.mcp_servers, { private_mcp: { enabled: false } });
  assert(
    (params.config as Record<string, unknown>)["features.shell_tool"] === false,
  );
  const start = {
    thread: { id: "t", ephemeral: true, path: null },
    model: "gpt-6-astra",
    modelProvider: "openai",
    cwd: "/tmp/probe",
    approvalPolicy: "never",
    sandbox: { type: "readOnly", networkAccess: false },
    instructionSources: [],
  };
  equal(assertEphemeralStart(start, "gpt-6-astra", "/tmp/probe"), "t");
  const nativeSource = "/native-home/AGENTS.md";
  equal(
    assertEphemeralStart(
      { ...start, instructionSources: [nativeSource] },
      "gpt-6-astra",
      "/tmp/probe",
      nativeSource,
    ),
    "t",
  );
  for (
    const sources of [
      ["/tmp/probe/AGENTS.md"],
      ["/native-home/nested/AGENTS.md"],
      ["/native-home-other/AGENTS.md"],
      ["/native-home/AGENTS.override.md"],
      [nativeSource, "/tmp/probe/AGENTS.md"],
      [nativeSource, nativeSource],
      [null],
      [42],
    ]
  ) {
    rejectsCode(() =>
      assertEphemeralStart(
        { ...start, instructionSources: sources },
        "gpt-6-astra",
        "/tmp/probe",
        nativeSource,
      ), "personal_instructions_loaded");
  }
  rejectsCode(
    () =>
      assertEphemeralStart(
        { ...start, thread: { id: "t", ephemeral: false } },
        "gpt-6-astra",
        "/tmp/probe",
      ),
    "thread_not_ephemeral",
  );
  rejectsCode(
    () =>
      assertEphemeralStart(
        { ...start, instructionSources: ["private-path"] },
        "gpt-6-astra",
        "/tmp/probe",
      ),
    "personal_instructions_loaded",
  );
  rejectsCode(
    () =>
      assertEphemeralStart(
        { ...start, model: "gpt-5.6-luna" },
        "gpt-6-astra",
        "/tmp/probe",
      ),
    "model_route_changed",
  );
  assertNoMcpTools({
    data: [{ name: "private_mcp", serverInfo: null, tools: {} }],
    nextCursor: null,
  });
  rejectsCode(
    () =>
      assertNoMcpTools({ data: [{ serverInfo: {}, tools: { secret: {} } }] }),
    "mcp_attestation_failed",
  );
  rejectsCode(
    () => assertNoMcpTools({ data: [], nextCursor: "more" }),
    "mcp_attestation_failed",
  );
});

Deno.test("native global allowance checks metadata only and refuses symlinks", async () => {
  const seen: string[] = [];
  equal(
    await verifyNativeGlobalInstructions("/native-home", (path) => {
      seen.push(path);
      return Promise.resolve({ isFile: true, isSymlink: false });
    }),
    "/native-home/AGENTS.md",
  );
  equal(seen, ["/native-home/AGENTS.md"]);
  for (
    const info of [{ isFile: true, isSymlink: true }, {
      isFile: false,
      isSymlink: false,
    }]
  ) {
    try {
      await verifyNativeGlobalInstructions(
        "/native-home",
        () => Promise.resolve(info),
      );
      throw new Error("expected rejection");
    } catch (error) {
      equal(safeFailure(error), {
        error: "native_global_instructions_not_regular",
      });
    }
  }
  equal(
    await verifyNativeGlobalInstructions(
      "/native-home",
      () => Promise.reject(new Deno.errors.NotFound()),
    ),
    undefined,
  );
});

Deno.test("token projection emits only bounded numeric last-turn counters", () => {
  equal(
    safeTokenUsage({
      last: {
        totalTokens: 12,
        inputTokens: 9,
        outputTokens: 3,
        identity: "DO_NOT_EMIT",
        cachedInputTokens: -1,
        reasoningOutputTokens: "DO_NOT_EMIT",
      },
      total: { totalTokens: 9999 },
    }),
    { totalTokens: 12, inputTokens: 9, outputTokens: 3 },
  );
  equal(safeTokenUsage(null), undefined);
});

Deno.test("smoke result rejects tool activity and only accepts the constant", () => {
  assertSmokeOutput([{ type: "reasoning" }, {
    type: "agentMessage",
    text: '{"ok":"MOON_CODEX_OK"}',
  }]);
  rejectsCode(
    () =>
      assertSmokeOutput([{ type: "commandExecution" }, {
        type: "agentMessage",
        text: '{"ok":"MOON_CODEX_OK"}',
      }]),
    "unexpected_turn_item",
  );
  rejectsCode(
    () =>
      assertSmokeOutput([{
        type: "agentMessage",
        text: '{"ok":"MOON_CODEX_OK","email":"DO_NOT_EMIT"}',
      }]),
    "unexpected_smoke_output",
  );
});

Deno.test("personal instruction admission failure sends no turn request", async () => {
  const fake = new FakeProcess((request, fake) =>
    fake.emit({
      id: request.id,
      result: {
        thread: { id: "ephemeral-thread", ephemeral: true, path: null },
        model: "gpt-6-astra",
        modelProvider: "openai",
        cwd: "/tmp/probe",
        approvalPolicy: "never",
        sandbox: { type: "readOnly", networkAccess: false },
        instructionSources: ["/private-fixture/AGENTS.md"],
      },
    })
  );
  const client = new RpcClient(fake);
  try {
    try {
      await smokeTurn(client, "/tmp/probe", "gpt-6-astra", "low", [], 1000);
      throw new Error("expected rejection");
    } catch (error) {
      equal(safeFailure(error), {
        error: "personal_instructions_loaded",
        method: "thread/start",
      });
    }
    equal(client.turnAttempts, 0);
    assert(!fake.requests.some((request) => request.method === "turn/start"));
    assert(
      !Object.values(smokeThreadParams("/tmp/probe", "gpt-6-astra", []).config)
        .some((value) => value === null),
    );
  } finally {
    await client.close();
  }
});

Deno.test("RPC tolerates notifications before replies and sanitizes rejected requests", async () => {
  const fake = new FakeProcess((request, fake) => {
    fake.emit({ method: "notification", params: { detail: "DO_NOT_EMIT" } });
    if (request.method === "reject") {
      fake.emit({
        id: request.id,
        error: {
          code: -32600,
          message: "DO_NOT_EMIT",
          data: { credential: "DO_NOT_EMIT" },
        },
      });
    } else fake.emit({ id: request.id, result: { ok: true } });
  });
  const client = new RpcClient(fake);
  let notifications = 0;
  client.listen(() => notifications++);
  try {
    equal(await client.request("probe"), { ok: true });
    try {
      await client.request("reject");
      throw new Error("expected rejection");
    } catch (error) {
      equal(safeFailure(error), {
        error: "rpc_rejected",
        method: "reject",
        rpc_code: -32600,
      });
    }
    equal(notifications, 2);
  } finally {
    await client.close();
  }
});

Deno.test("RPC rejects server tool requests without replying or authorising them", async () => {
  const fake = new FakeProcess((_request, fake) =>
    fake.emit({
      id: "server-request",
      method: "item/tool/call",
      params: { arguments: "DO_NOT_EMIT" },
    })
  );
  const client = new RpcClient(fake);
  try {
    try {
      await client.request("probe");
      throw new Error("expected rejection");
    } catch (error) {
      equal(safeFailure(error), { error: "unexpected_server_request" });
    }
    equal(fake.requests.length, 1);
  } finally {
    await client.close();
  }
});

Deno.test("smoke handles item completion before the turn response with exactly two turns", async () => {
  let sequence = 0;
  const fake = new FakeProcess((request, fake) => {
    const params = request.params as Record<string, unknown>;
    if (request.method === "thread/start") {
      sequence++;
      fake.emit({
        id: request.id,
        result: {
          thread: { id: `thread-${sequence}`, ephemeral: true, path: null },
          model: "gpt-6-astra",
          modelProvider: "openai",
          cwd: "/tmp/probe",
          approvalPolicy: "never",
          sandbox: { type: "readOnly", networkAccess: false },
          instructionSources: [],
        },
      });
    } else if (request.method === "mcpServerStatus/list") {
      fake.emit({ id: request.id, result: { data: [], nextCursor: null } });
    } else if (request.method === "turn/start") {
      const turn = {
        id: `turn-${sequence}`,
        status: "completed",
        error: null,
        items: [],
      };
      fake.emit({
        method: "item/completed",
        params: {
          threadId: params.threadId,
          turnId: turn.id,
          item: {
            id: `item-${sequence}`,
            type: "agentMessage",
            text: '{"ok":"MOON_CODEX_OK"}',
          },
        },
      });
      fake.emit({
        method: "turn/completed",
        params: { threadId: params.threadId, turn },
      });
      fake.emit({
        id: request.id,
        result: { turn: { ...turn, status: "inProgress" } },
      });
    } else fake.emit({ id: request.id, result: {} });
  });
  const client = new RpcClient(fake);
  try {
    for (const effort of ["low", "xhigh"]) {
      const outcome = await smokeTurn(
        client,
        "/tmp/probe",
        "gpt-6-astra",
        effort,
        [],
        1000,
      );
      equal({
        effort: outcome.effort,
        passed: outcome.passed,
        ephemeral: outcome.ephemeral,
      }, { effort, passed: true, ephemeral: true });
      equal(outcome.model, "gpt-6-astra");
      equal(outcome.response, { ok: "MOON_CODEX_OK" });
      equal(outcome.native_global_instructions_loaded, false);
      assert(
        Number.isSafeInteger(outcome.latency_ms) && outcome.latency_ms >= 0,
      );
    }
    const turns = fake.requests.filter((request) =>
      request.method === "turn/start"
    );
    equal(
      turns.map((request) =>
        (request.params as Record<string, unknown>).effort
      ),
      ["low", "xhigh"],
    );
    assert(
      !fake.requests.some((request) => request.method === "turn/interrupt"),
    );
    equal(
      fake.requests.filter((request) => request.method === "thread/unsubscribe")
        .length,
      2,
    );
  } finally {
    await client.close();
  }
});

Deno.test("RPC response deadline and stderr cap terminate safely", async () => {
  const idle = new RpcClient(new FakeProcess(() => {}));
  try {
    try {
      await idle.request("probe", {}, 5);
      throw new Error("expected rejection");
    } catch (error) {
      equal(safeFailure(error), { error: "rpc_timeout" });
    }
  } finally {
    await idle.close();
  }
  const noisy = new RpcClient(
    new FakeProcess((_request, fake) =>
      fake.emitStderr(new Uint8Array(1024 * 1024 + 1))
    ),
  );
  try {
    try {
      await noisy.request("probe");
      throw new Error("expected rejection");
    } catch (error) {
      equal(safeFailure(error), { error: "stderr_limit" });
    }
  } finally {
    await noisy.close();
  }
});

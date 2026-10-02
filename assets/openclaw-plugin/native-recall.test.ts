import moonPlugin, { __moonTest } from "./index.js";

type Hook = (
  event: Record<string, unknown>,
  ctx?: Record<string, unknown>,
) => unknown | Promise<unknown>;
type CommandResult = { code: number; stdout: string; stderr: string };

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
    "required native recall integration needs an explicit test binary and home",
  );
}

const packet =
  "# Moon Context\n\nTrust: untrusted retrieved data\nSynthetic recall";
const requestId = "0123456789abcdef0123456789abcdef";
const context = {
  agentId: "main",
  sessionKey: "agent:main:discord:channel:synthetic",
  runId: "run-1",
  trigger: "user",
};
const event = {
  prompt: "Reconstructed history: an unrelated old request",
  currentUserMessage: "Recall the SQLite plan",
  currentUserMessageId: "admission-1",
  messages: [{ role: "user", content: "Unrelated old request" }],
};

function assert(value: unknown, message = "assertion failed"): asserts value {
  if (!value) throw new Error(message);
}

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function envelope(content: string | null = packet) {
  return {
    request_id: requestId,
    packet: content,
    memory_count: content ? 1 : 0,
    reference_count: 0,
    packet_chars: content ? Array.from(content).length : 0,
    truncated: false,
  };
}

function fixture(options: {
  hooks?: Record<string, unknown>;
  hostHooks?: boolean;
  pluginConfig?: Record<string, unknown>;
  result?: (argv: string[]) => CommandResult | Promise<CommandResult>;
} = {}) {
  const calls: string[][] = [];
  const errors: string[] = [];
  const hooks = new Map<string, Hook>();
  let factory:
    | (() => ReturnType<typeof __moonTest.createMoonContextEngine>)
    | undefined;
  let service: { stop(): Promise<void> } | undefined;
  const api = {
    config: {
      plugins: {
        slots: { contextEngine: "moon" },
        entries: {
          moon: {
            hooks: options.hooks ?? { allowConversationAccess: true },
          },
        },
      },
    },
    pluginConfig: {
      moonPath: "/tmp/synthetic-bin/moon",
      moonHome: "/tmp/moon-native-recall-test",
      mode: "lexical",
      embeddingEnabled: false,
      ...options.pluginConfig,
    },
    resolvePath(value: string) {
      return value;
    },
    runtime: {
      system: {
        runCommandWithTimeout(argv: string[]) {
          calls.push(argv);
          return options.result?.(argv) ?? {
            code: 0,
            stdout: JSON.stringify(envelope()),
            stderr: "",
          };
        },
      },
    },
    logger: {
      error(message: string) {
        errors.push(message);
      },
      info() {},
    },
    ...(options.hostHooks === false ? {} : {
      on(name: string, handler: Hook) {
        hooks.set(name, handler);
      },
    }),
    registerService(value: { stop(): Promise<void> }) {
      service = value;
    },
    registerContextEngine(
      _id: string,
      value: () => ReturnType<typeof __moonTest.createMoonContextEngine>,
    ) {
      factory = value;
    },
    registerCompactionProvider() {},
  };
  moonPlugin.register(api);
  assert(factory);
  assert(service);
  const engine = factory();
  const prepare = (input = event, ctx: Record<string, unknown> = context) => {
    const hook = hooks.get("before_prompt_build");
    assert(hook, "before_prompt_build was not registered");
    return hook(input, ctx);
  };
  return { api, calls, errors, hooks, engine, prepare, service };
}

Deno.test("native recall uses the explicit current request and contributes only prompt data", async () => {
  const f = fixture();
  try {
    equal(await f.prepare(), { prependContext: packet });
    const command = f.calls.find((argv) => argv.includes("context"));
    assert(command);
    equal(command[command.indexOf("--query") + 1], event.currentUserMessage);
    equal(
      command[command.indexOf("--home") + 1],
      "/tmp/moon-native-recall-test",
    );
    assert(f.calls[1].includes("--injected"));
    const messages = [{ role: "user", content: event.currentUserMessage }];
    const assembled = await f.engine.assemble({ messages });
    assert(assembled.messages === messages);
    equal(f.calls.length, 2);
    assert(f.engine.info.ownsCompaction === false);
  } finally {
    await f.service.stop();
  }
});

Deno.test("recall keeps assemble fallback when hooks or explicit permission are unavailable", async () => {
  for (
    const options of [
      { hostHooks: false },
      { hooks: {} },
      { hooks: { allowConversationAccess: false } },
      { hooks: { allowConversationAccess: true, allowPromptInjection: false } },
    ]
  ) {
    const f = fixture(options);
    try {
      assert(!f.hooks.has("before_prompt_build"));
      const result = await f.engine.assemble({
        messages: [{ role: "user", content: "Recall the SQLite plan" }],
      });
      equal(result.messages.length, 2);
      equal(f.calls.length, 2);
      assert(
        f.errors.some((message) =>
          message.includes("native recall unavailable")
        ),
      );
    } finally {
      await f.service.stop();
    }
  }
});

Deno.test("recall skips empty current requests, greetings, heartbeats and model helpers", async () => {
  const f = fixture();
  try {
    equal(await f.prepare({ ...event, currentUserMessage: "" }), undefined);
    equal(await f.prepare({ ...event, currentUserMessage: "  " }), undefined);
    equal(
      await f.prepare({ ...event, currentUserMessage: "hello" }),
      undefined,
    );
    for (
      const ctx of [
        { ...context, trigger: "heartbeat" },
        { ...context, runId: "moon-model-synthetic" },
        { ...context, sessionId: "internal-session-effects-synthetic" },
        {
          ...context,
          sessionKey: "agent:main:internal-session-effects:incognito-synthetic",
        },
      ]
    ) {
      equal(await f.prepare(event, ctx), undefined);
    }
    f.api.config.plugins.slots.contextEngine = "another-engine";
    equal(await f.prepare(), undefined);
    equal(f.calls.length, 0);
  } finally {
    await f.service.stop();
  }
});

Deno.test("legacy prompt hooks retain query extraction when explicit request is omitted", async () => {
  const f = fixture();
  try {
    const hook = f.hooks.get("before_prompt_build")!;
    equal(
      await hook({ prompt: "Recall the Rust plan", messages: [] }, context),
      {
        prependContext: packet,
      },
    );
    equal(
      f.calls[0][f.calls[0].indexOf("--query") + 1],
      "Recall the Rust plan",
    );
  } finally {
    await f.service.stop();
  }
});

Deno.test("prompt recall requires an explicitly selected Moon context engine", async () => {
  const f = fixture();
  try {
    for (
      const selected of [undefined, null, "", "  ", "legacy", "another-engine"]
    ) {
      if (selected === undefined) {
        Reflect.deleteProperty(f.api.config.plugins.slots, "contextEngine");
      } else {
        Object.assign(f.api.config.plugins.slots, { contextEngine: selected });
      }
      equal(await f.prepare(), undefined);
    }
    equal(f.calls.length, 0);
  } finally {
    await f.service.stop();
  }
});

Deno.test("native admission rebuilds reuse retrieval but equal text without an ID does not", async () => {
  const f = fixture();
  try {
    equal(await f.prepare(), { prependContext: packet });
    equal(
      await f.prepare({ ...event, prompt: "Different reconstructed history" }),
      {
        prependContext: packet,
      },
    );
    equal(f.calls.length, 2);
    await f.prepare({ ...event, currentUserMessageId: "admission-2" });
    equal(f.calls.length, 4);
    const withoutAdmission = { ...event, currentUserMessageId: "" };
    await f.prepare(withoutAdmission);
    await f.prepare(withoutAdmission);
    equal(f.calls.length, 8);
    await f.hooks.get("agent_end")!({ runId: context.runId }, context);
    await f.prepare();
    equal(f.calls.length, 10);
  } finally {
    await f.service.stop();
  }
});

Deno.test("identical admission IDs in separate chats do not share retrieval", async () => {
  const f = fixture();
  try {
    await f.prepare();
    await f.prepare(event, {
      ...context,
      sessionKey: "agent:main:discord:other",
    });
    equal(f.calls.filter((argv) => argv.includes("context")).length, 2);
  } finally {
    await f.service.stop();
  }
});

Deno.test("empty recall packets record a non-injection", async () => {
  const f = fixture({
    result: () => ({
      code: 0,
      stdout: JSON.stringify(envelope(null)),
      stderr: "",
    }),
  });
  try {
    equal(await f.prepare(), undefined);
    equal(f.calls.length, 2);
    assert(f.calls[1].includes("mark-injection"));
    assert(!f.calls[1].includes("--injected"));
  } finally {
    await f.service.stop();
  }
});

Deno.test("a failed recall hook fails open with content-free diagnostics", async () => {
  const f = fixture({
    result: () => ({ code: 1, stdout: "", stderr: "private fixture text" }),
  });
  try {
    equal(await f.prepare(), undefined);
    equal(f.calls.length, 1);
    assert(f.errors.length > 0);
    assert(!f.errors.join(" ").includes("private fixture text"));
  } finally {
    await f.service.stop();
  }
});

Deno.test("a hook expiring during retrieval never marks the packet injected", async () => {
  let active = true;
  const f = fixture({
    result: () => {
      active = false;
      return { code: 0, stdout: JSON.stringify(envelope()), stderr: "" };
    },
  });
  try {
    equal(
      await f.prepare(event, {
        ...context,
        hookInvocation: {
          assertActive() {
            if (!active) throw new Error("expired");
          },
        },
      }),
      undefined,
    );
    equal(f.calls.length, 1);
  } finally {
    await f.service.stop();
  }
});

Deno.test("a hook expiring during its metric write rolls back the injection mark", async () => {
  let active = true;
  const f = fixture({
    result: (argv) => {
      if (argv.includes("--injected")) active = false;
      return { code: 0, stdout: JSON.stringify(envelope()), stderr: "" };
    },
  });
  try {
    equal(
      await f.prepare(event, {
        ...context,
        hookInvocation: {
          assertActive() {
            if (!active) throw new Error("expired");
          },
        },
      }),
      undefined,
    );
    equal(f.calls.length, 3);
    assert(f.calls[1].includes("--injected"));
    assert(f.calls[2].includes("mark-injection"));
    assert(!f.calls[2].includes("--injected"));
  } finally {
    await f.service.stop();
  }
});

Deno.test("service shutdown discards an in-flight lexical recall contribution", async () => {
  const command = Promise.withResolvers<CommandResult>();
  const f = fixture({ result: () => command.promise });
  const pending = f.prepare();
  await f.service.stop();
  command.resolve({ code: 0, stdout: JSON.stringify(envelope()), stderr: "" });
  equal(await pending, undefined);
  equal(f.calls.length, 1);
});

Deno.test("hook and context engine share the service-owned local worker", async () => {
  const prototype = __moonTest.MoonStdioClient.prototype;
  const originalRequest = prototype.request;
  const originalDispose = prototype.dispose;
  const workers = new Set<object>();
  let disposed = 0;
  prototype.request = function (request: Record<string, unknown>) {
    workers.add(this);
    return Promise.resolve(
      request.op === "context" ? envelope() : { updated: true },
    );
  };
  prototype.dispose = function () {
    workers.add(this);
    disposed += 1;
    return Promise.resolve();
  };
  const f = fixture({ pluginConfig: { mode: "hybrid" } });
  try {
    equal(await f.prepare(), { prependContext: packet });
    await f.engine.assemble({
      messages: [{ role: "user", content: "Recall SQLite" }],
    });
    await f.engine.dispose();
    equal(disposed, 0);
    await f.service.stop();
    equal(disposed, 1);
    equal(workers.size, 1);
    equal(f.calls.length, 0);
  } finally {
    await f.service.stop();
    prototype.request = originalRequest;
    prototype.dispose = originalDispose;
  }
});

Deno.test({
  name:
    "native recall hook retrieves and marks a real Moon packet exactly once",
  ignore: !realMoonBinary || !realMoonHome,
  async fn() {
    assert(realMoonBinary && realMoonHome);
    const query = await optionalTestEnv("MOON_TEST_QUERY") ??
      "roomKey redemptionKey participant reenter";
    const expected = await optionalTestEnv("MOON_TEST_EXPECTED") ??
      "redemptionKey";
    const run = async (argv: string[]): Promise<CommandResult> => {
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
    };
    let observedRequestId: string | undefined;
    const f = fixture({
      pluginConfig: {
        moonPath: realMoonBinary,
        moonHome: realMoonHome,
        mode: "lexical",
      },
      async result(argv) {
        const result = await run(argv);
        assert(result.code === 0, "synthetic Moon command failed");
        if (argv.includes("context")) {
          observedRequestId = JSON.parse(result.stdout).request_id;
        }
        return result;
      },
    });
    try {
      const input = {
        ...event,
        currentUserMessage: query,
        currentUserMessageId: `synthetic-${crypto.randomUUID()}`,
      };
      const result = await f.prepare(input);
      assert(
        result !== null && typeof result === "object" &&
          "prependContext" in result &&
          typeof result.prependContext === "string",
      );
      assert(result.prependContext.startsWith("# Moon Context"));
      assert(result.prependContext.includes("## Retrieved references"));
      for (const phrase of expected.split("|")) {
        assert(result.prependContext.includes(phrase));
      }
      assert(observedRequestId && /^[0-9a-f]{32}$/.test(observedRequestId));
      equal(f.calls.length, 2);
      assert(f.calls[1].includes("mark-injection"));
      assert(f.calls[1].includes("--injected"));
      assert(f.calls[1].includes(observedRequestId));

      const messages = [{ role: "user", content: query }];
      assert((await f.engine.assemble({ messages })).messages === messages);
      equal(await f.prepare(input), result);
      equal(f.calls.length, 2);

      const recent = await run([
        realMoonBinary,
        "--home",
        realMoonHome,
        "--database",
        `${realMoonHome}/state/moon.sqlite`,
        "--dimensions",
        "384",
        "--json",
        "metrics",
        "recent",
        "--since",
        "1h",
        "--limit",
        "100",
      ]);
      equal(recent.code, 0);
      const rows = JSON.parse(recent.stdout) as Array<Record<string, unknown>>;
      const observed = rows.filter((row) =>
        row.request_id === observedRequestId
      );
      equal(observed.length, 1);
      equal(observed[0].status, "ok");
      equal(observed[0].adapter_injected, true);
      assert(Number(observed[0].reference_count) > 0);
      equal(f.errors, []);
    } finally {
      await f.service.stop();
    }
  },
});

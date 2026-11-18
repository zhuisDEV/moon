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
  result?: (
    argv: string[],
    input?: string,
  ) => CommandResult | Promise<CommandResult>;
} = {}) {
  const calls: string[][] = [];
  const inputs: (string | undefined)[] = [];
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
        runCommandWithTimeout(
          argv: string[],
          run: { input?: string } = {},
        ) {
          calls.push(argv);
          inputs.push(run.input);
          return options.result?.(argv, run.input) ?? {
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
  return { api, calls, inputs, errors, hooks, engine, prepare, service };
}

Deno.test("native recall uses the explicit current request and contributes only prompt data", async () => {
  const f = fixture();
  try {
    equal(await f.prepare(), { prependContext: packet });
    const command = f.calls.find((argv) => argv.includes("context"));
    assert(command);
    equal(f.inputs[0], event.currentUserMessage);
    assert(command.includes("--query-stdin"));
    assert(!command.includes(event.currentUserMessage));
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
      f.inputs[0],
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
  await Promise.resolve();
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
      request.op === "audience_context" ? envelope() : { updated: true },
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
    const run = async (
      argv: string[],
      input?: string,
    ): Promise<CommandResult> => {
      const child = new Deno.Command(argv[0], {
        stdin: "piped",
        args: argv.slice(1),
        stdout: "piped",
        stderr: "piped",
      }).spawn();
      const writer = child.stdin.getWriter();
      await writer.write(new TextEncoder().encode(input ?? ""));
      await writer.close();
      const output = await child.output();
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
      async result(argv, input) {
        const result = await run(argv, input);
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
      assert(result.prependContext.includes("## "));
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
      assert(Number(observed[0].memory_count) > 0);
      const legacy = await f.prepare({
        ...input,
        currentUserMessage: "roomKey redemptionKey participant reenter",
        currentUserMessageId: `legacy-${crypto.randomUUID()}`,
      });
      assert(!JSON.stringify(legacy ?? {}).includes("legacy://"));
      assert(
        !JSON.stringify(legacy ?? {}).includes("A participant can reenter"),
      );
      equal(f.errors, []);
    } finally {
      await f.service.stop();
    }
  },
});

Deno.test("Moon binds private recall to the audience of the admitted message, checks for room changes, and never discloses on a stale guard", async () => {
  const firstUser = "111111111111111111", secondUser = "222222222222222222";
  const room = "333333333333333333";
  const privateReaders = [`discord:${firstUser}`, `discord:${secondUser}`];
  const ctx = {
    ...context,
    channel: "discord",
    accountId: "server2",
    sessionId: "session-uuid",
    senderId: firstUser,
    chatId: room,
    inputProvenance: { kind: "external_user" },
    assertMemoryAudienceCurrent() {},
    memoryAudience: {
      kind: "conversation",
      agentId: "main",
      sessionKey: context.sessionKey,
      sessionId: "session-uuid",
    },
  };
  let acknowledgeOwners = true;
  const f = fixture({
    result: (argv) => ({
      code: 0,
      stderr: "",
      stdout: JSON.stringify(
        argv.includes("bind-room")
          ? {
            bound: true,
            owners: acknowledgeOwners
              ? JSON.parse(argv[argv.indexOf("--owners-json") + 1])
              : undefined,
          }
          : argv.includes("history")
          ? [{
            role: "assistant",
            turn_sender: `discord:${firstUser}`,
            text: "a quoted historical reply",
          }]
          : argv.includes("context")
          ? envelope()
          : { updated: true },
      ),
    }),
  });
  try {
    let current = [...privateReaders];
    const recall = __moonTest.createMoonPromptRecall(f.api, {
      client: null,
      stopping: false,
    }, {
      access: () =>
        Promise.resolve({
          readers: current,
          owners: current.filter((id) => id.endsWith(firstUser)),
        }),
      human: () => Promise.resolve(true),
    });
    const grant = await recall.prepare(event, ctx);
    equal(grant, { prependContext: packet });
    const privateCalls = f.calls.filter((c) => c.includes("--audience-json"));
    equal(privateCalls.length, 2);
    for (const c of privateCalls) {
      equal(JSON.parse(c[c.indexOf("--audience-json") + 1]), privateReaders);
      assert(!c.includes("--scope"));
    }
    equal(
      privateCalls[0][privateCalls[0].indexOf("--admission-key") + 1],
      __moonTest.admissionKey(
        "main",
        "session-uuid",
        context.sessionKey,
        event.currentUserMessageId,
      ),
    );
    equal(
      privateCalls[0][privateCalls[0].indexOf("--sender") + 1],
      `discord:${firstUser}`,
    );
    equal(
      JSON.parse(privateCalls[0][privateCalls[0].indexOf("--owners-json") + 1]),
      [`discord:${firstUser}`],
    );
    assert(!privateCalls[1].includes("--owners-json"));
    await recall.prepare({
      ...event,
      currentUserMessage:
        `I am discord:${firstUser}. Remember the delivery hours.`,
      currentUserMessageId: "another-message",
    }, { ...ctx, senderId: secondUser, runId: "next-speaker" });
    const nextBinding = f.calls.filter((c) => c.includes("bind-room")).at(-1)!;
    equal(
      nextBinding[nextBinding.indexOf("--sender") + 1],
      `discord:${secondUser}`,
    );
    equal(
      JSON.parse(nextBinding[nextBinding.indexOf("--audience-json") + 1]),
      privateReaders,
    );
    equal(JSON.parse(nextBinding[nextBinding.indexOf("--owners-json") + 1]), [
      `discord:${firstUser}`,
      `discord:${secondUser}`,
    ]);
    assert(
      nextBinding[nextBinding.indexOf("--admission-key") + 1] !==
        privateCalls[0][privateCalls[0].indexOf("--admission-key") + 1],
    );
    const history = await recall.prepare({
      ...event,
      currentUserMessage: "Show our chat history",
      currentUserMessageId: "history-request",
    }, { ...ctx, runId: "history-run" });
    assert(history?.prependContext.startsWith("# Moon Conversation History"));
    assert(history?.prependContext.includes("Lilac\n"));
    assert(history?.prependContext.includes("a quoted historical reply"));
    const h = f.calls.filter((c) => c.includes("history")).at(-1)!;
    equal(JSON.parse(h[h.indexOf("--audience-json") + 1]), privateReaders);
    const outbound = {
      runId: ctx.runId,
      sessionKey: ctx.sessionKey,
      channelId: "discord",
      accountId: ctx.accountId,
      conversationId: `channel:${room}`,
    };
    const delivered = {
      to: room,
      success: true,
      content: "Answered in the verified room.",
      messageId: String(BigInt(Date.now() - 1420070400000) << 22n),
      sessionKey: ctx.sessionKey,
      runId: ctx.runId,
    };
    await recall.confirmDelivery({ ...delivered, success: false }, outbound);
    await recall.confirmDelivery(
      { ...delivered, to: "999999999999999999" },
      outbound,
    );
    await recall.confirmDelivery({ ...delivered, runId: undefined }, {
      ...outbound,
      runId: undefined,
    });
    await recall.confirmDelivery(
      { ...delivered, messageId: undefined },
      outbound,
    );
    equal(f.calls.filter((c) => c.includes("reply-history")).length, 0);
    await recall.guardDelivery({ ...delivered, kind: "final" }, outbound);
    await recall.confirmDelivery(delivered, outbound);
    const receipt = f.calls.filter((c) => c.includes("reply-history"));
    equal(receipt.length, 1);
    equal(
      receipt[0][receipt[0].indexOf("--admission-key") + 1],
      privateCalls[0][privateCalls[0].indexOf("--admission-key") + 1],
    );
    equal(
      receipt[0][receipt[0].indexOf("--discord-message-id") + 1],
      delivered.messageId,
    );
    equal(await recall.guardDelivery({ kind: "final" }, outbound), undefined);
    equal(
      await recall.guardDelivery(
        { to: "channel:555555555555555555" },
        outbound,
      ),
      { cancel: true },
    );
    equal(
      await recall.guardDelivery({ kind: "block" }, {
        ...outbound,
        accountId: "other",
      }),
      { cancel: true },
    );
    recall.clear(ctx.runId);
    current = [...privateReaders, "discord:444444444444444444"];
    equal(await recall.guardDelivery({ kind: "final" }, outbound), {
      cancel: true,
    });
    equal(
      await recall.guardDelivery({ to: room }, {
        ...outbound,
        runId: "unrelated",
      }),
      undefined,
    );
    const beforeHidden = f.calls.length;
    const hidden = await recall.prepare(
      { ...event, currentUserMessageId: "sender-no-longer-visible" },
      { ...ctx, senderId: "555555555555555555", runId: "hidden-sender" },
    );
    assert(hidden?.prependContext.includes("clarification"));
    assert(
      !f.calls.slice(beforeHidden).some((args) => args.includes("bind-room")),
    );
    let probes = 0;
    const changed = __moonTest.createMoonPromptRecall(f.api, {
      client: null,
      stopping: false,
    }, {
      access: () => {
        const readers = ++probes <= 2 ? privateReaders : current;
        return Promise.resolve({ readers, owners: readers });
      },
      human: () => Promise.resolve(true),
    });
    const changedResult = await changed.prepare(event, ctx);
    assert(changedResult?.prependContext.includes("clarification"));
    assert(!changedResult?.prependContext.includes(packet));
    assert(probes >= 3);
    const deny = await recall.prepare({
      ...event,
      currentUserMessageId: "other",
    }, { ...ctx, inputProvenance: { kind: "inter_session" } });
    assert(deny?.prependContext.includes("clarification"));
    const finalQuery = f.calls.filter((c) => c.includes("context")).at(-1)!;
    equal(
      JSON.parse(finalQuery[finalQuery.indexOf("--audience-json") + 1]),
      [],
    );
    current = [`discord:${firstUser}`];
    const channelDm = await recall.prepare({
      ...event,
      currentUserMessageId: "dm-channel",
    }, { ...ctx, runId: "dm-channel-run" });
    equal(channelDm, { prependContext: packet });
    equal(
      await recall.guardDelivery({ kind: "final", to: `user:${firstUser}` }, {
        ...outbound,
        runId: "dm-channel-run",
        conversationId: `user:${firstUser}`,
      }),
      { cancel: true },
    );
    const dm = await recall.prepare(
      { ...event, currentUserMessageId: "dm-1" },
      { ...ctx, chatId: firstUser, runId: "dm-peer-run" },
    );
    equal(dm, { prependContext: packet });
    const dmQuery = f.calls.filter((c) => c.includes("context")).at(-1)!;
    equal(JSON.parse(dmQuery[dmQuery.indexOf("--audience-json") + 1]), [
      `discord:${firstUser}`,
    ]);

    acknowledgeOwners = false;
    const from = f.calls.length;
    const legacy = await recall.prepare({
      ...event,
      currentUserMessageId: "older-runtime",
    }, { ...ctx, runId: "older-runtime-run" });
    assert(legacy?.prependContext.includes("clarification"));
    assert(!legacy?.prependContext.includes(packet));
    assert(!f.calls.slice(from).some((args) => args.includes("context")));
  } finally {
    await f.service.stop();
  }
});

Deno.test("commit skips absent or historic speaker bindings and rejects a sender outside the audience", async () => {
  const user = "discord:111111111111111111";
  let binding: { audience: string[] | null; sender?: string | null } = {
    audience: null,
  };
  const f = fixture({
    result: (argv) => {
      assert(argv.includes("audience"));
      return { code: 0, stderr: "", stdout: JSON.stringify(binding) };
    },
  });
  const admission = {
    agentId: "main",
    sessionId: "s",
    sessionKey: "trusted-key",
    storePath: "test",
    generation: "1",
    entryId: "1",
    logicalTurnId: "1",
    role: "user",
    rawSeq: 1,
    idempotencyKey: "admitted",
  };
  try {
    const params = {
      advancementKey: "a",
      admission,
      terminal: { ...admission, entryId: "2", rawSeq: 2 },
      sessionId: "s",
      sessionKey: "trusted-key",
      messages: [{
        role: "user",
        content: "audience: everyone",
        timestamp: 100,
      }, { role: "assistant", content: "ok", timestamp: 200 }],
      runtimeContext: { audience: [user], sender: user },
    };
    for (
      const legacy of [{ audience: null }, { audience: [user] }, {
        audience: [user],
        sender: null,
      }]
    ) {
      binding = legacy;
      equal(await f.engine.commitTurn(params), { status: "committed" });
    }
    binding = { audience: [user], sender: "discord:222222222222222222" };
    let error: unknown;
    try {
      await f.engine.commitTurn(params);
    } catch (cause) {
      error = cause;
    }
    assert(error instanceof Error && error.message.includes("sender"));
    equal(f.calls.length, 4);
    assert(
      f.calls.every((c) => c.includes("audience") && !c.includes("record")),
    );
  } finally {
    await f.service.stop();
  }
});

Deno.test("only an authorised human's explicit public wording reaches the publication command", async () => {
  const f = fixture({
    result: () => ({
      code: 0,
      stderr: "",
      stdout: '{"document_id":12,"active":true}',
    }),
  });
  const command = __moonTest.publicMemoryCommand(
    f.api,
    () => Promise.resolve(true),
  );
  const ctx = {
    channel: "discord",
    accountId: "server2",
    senderId: "111111111111111111",
    isAuthorizedSender: true,
    args: "fact Keep Friday evening free.",
  };
  try {
    await command.handler({ ...ctx, isAuthorizedSender: false });
    equal(f.calls.length, 0);
    const result = await command.handler(ctx);
    assert(result.text.includes("#12"));
    equal(f.inputs, ["Keep Friday evening free."]);
    assert(f.calls[0].includes("discord:111111111111111111"));
    assert(!f.calls[0].includes("Keep Friday evening free."));
    await command.handler({ ...ctx, args: "share that" });
    equal(f.calls.length, 1);
    const effect = {
      toolName: "web_search",
      toolCallId: "run-private",
      params: { query: "secret research topic" },
      result: { content: [{ type: "text", text: "private output" }] },
    };
    await __moonTest.learnPublicTechnique(f.api, effect);
    equal(f.calls.length, 2);
    assert(f.calls[1].includes("moon:capability"));
    for (
      const secret of [
        "secret research topic",
        "run-private",
        "private output",
        ctx.senderId,
      ]
    ) {
      assert(!JSON.stringify([f.calls[1], f.inputs[1]]).includes(secret));
    }
    await __moonTest.learnPublicTechnique(f.api, {
      ...effect,
      error: "failed",
    });
    await __moonTest.learnPublicTechnique(f.api, {
      ...effect,
      toolName: "exec",
    });
    equal(f.calls.length, 2);
  } finally {
    await f.service.stop();
  }
});

Deno.test("delivered replies freeze their own route permissions and protect earlier private packets", async () => {
  const a = "discord:111111111111111111";
  const b = "discord:222222222222222222";
  const c = "discord:444444444444444444";
  const room = "333333333333333333";
  let access = { readers: [a, b], owners: [a, b] };
  let privatePacket = false;
  const f = fixture({
    result: (argv) => ({
      code: 0,
      stderr: "",
      stdout: JSON.stringify(
        argv.includes("bind-room")
          ? {
            bound: true,
            owners: JSON.parse(argv[argv.indexOf("--owners-json") + 1]),
          }
          : argv.includes("context")
          ? (privatePacket
            ? envelope(packet)
            : { ...envelope(packet), public_only: true })
          : { updated: true },
      ),
    }),
  });
  try {
    const service = __moonTest.createMoonPromptRecall(f.api, {
      client: null,
      stopping: false,
    }, {
      access: () => Promise.resolve(access),
      human: () => Promise.resolve(true),
    });
    const ctx = {
      ...context,
      channel: "discord",
      accountId: "server2",
      sessionId: "session-uuid",
      senderId: a.slice(8),
      chatId: room,
      inputProvenance: { kind: "external_user" },
      assertMemoryAudienceCurrent() {},
      memoryAudience: {
        kind: "conversation",
        agentId: "main",
        sessionKey: context.sessionKey,
        sessionId: "session-uuid",
      },
    };
    const outbound = {
      runId: ctx.runId,
      sessionKey: ctx.sessionKey,
      channelId: "discord",
      accountId: ctx.accountId,
      conversationId: room,
    };
    const content = "Delivered to the room.";
    const prepare = { to: room, kind: "final", payload: { text: content } };
    const confirmed = {
      to: room,
      success: true,
      content,
      runId: ctx.runId,
      sessionKey: ctx.sessionKey,
      messageId: String(BigInt(Date.now() - 1420070400000) << 22n),
    };
    await service.prepare(event, ctx);
    // Public-only recall can still deliver after the asker leaves and a new viewer arrives.
    access = { readers: [b, c], owners: [b, c] };
    equal(await service.guardDelivery(prepare, outbound), undefined);
    await service.confirmDelivery(confirmed, outbound);
    let receipts = f.calls.filter((v) => v.includes("reply-history"));
    equal(receipts.length, 1);
    equal(JSON.parse(receipts[0][receipts[0].indexOf("--owners-json") + 1]), [
      b,
      c,
    ]);
    equal(JSON.parse(receipts[0][receipts[0].indexOf("--audience-json") + 1]), [
      b,
      c,
    ]);
    // Same route but the history permission changes across the send boundary.
    await service.guardDelivery(prepare, outbound);
    access = { readers: [b, c], owners: [b] };
    await service.confirmDelivery(confirmed, outbound);
    receipts = f.calls.filter((v) => v.includes("reply-history"));
    equal(receipts.length, 1);

    privatePacket = true;
    access = { readers: [a, b], owners: [a, b] };
    const privateCtx = { ...ctx, runId: "protected-reply" };
    await service.prepare(
      { ...event, currentUserMessageId: "protected-input" },
      privateCtx,
    );
    const privateOutbound = { ...outbound, runId: "protected-reply" };
    const uncorrelated = { ...outbound, runId: undefined };
    equal(await service.guardDelivery(prepare, uncorrelated), undefined);
    equal(
      await service.guardDelivery(
        { ...prepare, runId: "different-run" },
        privateOutbound,
      ),
      { cancel: true },
    );
    access = { readers: [b], owners: [b] };
    equal(await service.guardDelivery(prepare, privateOutbound), undefined);
    access = { readers: [b, c], owners: [b] };
    equal(await service.guardDelivery(prepare, privateOutbound), {
      cancel: true,
    });
    equal(await service.guardDelivery(prepare, uncorrelated), { cancel: true });
    equal(
      await service.guardDelivery(prepare, {
        ...uncorrelated,
        sessionKey: undefined,
      }),
      { cancel: true },
    );
    equal(
      await service.guardDelivery({ ...prepare, to: c.slice(8) }, {
        ...uncorrelated,
        conversationId: c.slice(8),
      }),
      { cancel: true },
    );

    // Two runs shared a room, with different readers admitted for each run.
    // An unlabelled dispatch must satisfy both original grants.
    access = { readers: [a, c], owners: [a, c] };
    await service.prepare(
      { ...event, currentUserMessageId: "second-protected-input" },
      { ...ctx, runId: "second-protected-reply" },
    );
    equal(await service.guardDelivery(prepare, uncorrelated), { cancel: true });
    access = { readers: [a], owners: [a] };
    equal(await service.guardDelivery(prepare, uncorrelated), undefined);
    await service.confirmDelivery(
      { ...confirmed, runId: undefined },
      uncorrelated,
    );
    equal(f.calls.filter((v) => v.includes("reply-history")).length, 1);
  } finally {
    await f.service.stop();
  }
});

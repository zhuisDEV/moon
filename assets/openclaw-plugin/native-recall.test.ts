import {
  assembleModel,
  fixtureJudgement,
  hookModel,
  type JudgementInput,
  modelRunner,
  savedEnvelope,
} from "./recall-fixture.ts";
import { createHash } from "node:crypto";
import moonPlugin, { __moonTest, discordRoomAccess } from "./index.js";

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

const packet = "Synthetic recall";
const context = {
  ...hookModel,
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

function envelope(content: string | null = packet, publicOnly = true) {
  return savedEnvelope(content, publicOnly);
}

function fixture(options: {
  judge?: (input: JudgementInput) => unknown;
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
  const logs: string[] = [];
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
      agent: { runEmbeddedAgent: modelRunner(options.judge) },
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
      info(message: string) {
        logs.push(message);
      },
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
  return { api, calls, inputs, errors, logs, hooks, engine, prepare, service };
}

Deno.test("native recall uses the explicit current request and contributes only prompt data", async () => {
  const f = fixture();
  try {
    assert(JSON.stringify(await f.prepare()).includes(packet));
    const command = f.calls.find((argv) => argv.includes("recall-candidates"));
    assert(command);
    equal(JSON.parse(f.inputs[0]!), [{
      text: event.currentUserMessage,
      temporal: false,
    }]);
    assert(command.includes("--audience-json"));
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
        runtimeSettings: assembleModel,
        sessionKey: "agent:main:synthetic",
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
    assert(
      JSON.stringify(
        await hook({ prompt: "Recall the Rust plan", messages: [] }, context),
      ).includes(packet),
    );
    equal(JSON.parse(f.inputs[0]!), [{
      text: "Recall the Rust plan",
      temporal: false,
    }]);
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
    assert(JSON.stringify(await f.prepare()).includes(packet));
    assert(
      JSON.stringify(
        await f.prepare({
          ...event,
          prompt: "Different reconstructed history",
        }),
      ).includes(packet),
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
    equal(
      f.calls.filter((argv) => argv.includes("recall-candidates")).length,
      2,
    );
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
    assert(JSON.stringify(await f.prepare()).includes("no permitted evidence"));
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
  const started = Promise.withResolvers<void>();
  const f = fixture({
    result: () => {
      started.resolve();
      return command.promise;
    },
  });
  const pending = f.prepare();
  await started.promise;
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
      request.op === "recall_candidates" ? envelope() : { updated: true },
    );
  };
  prototype.dispose = function () {
    workers.add(this);
    disposed += 1;
    return Promise.resolve();
  };
  const f = fixture({ pluginConfig: { mode: "hybrid" } });
  try {
    assert(JSON.stringify(await f.prepare()).includes(packet));
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
        if (argv.includes("recall-candidates")) {
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
      assert(result.prependContext.includes("Untrusted source records"));
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
  const deliveredAt = 1_700_000_000_000;
  let acknowledgeOwners = true;
  let candidate = packet;
  const f = fixture({
    pluginConfig: { maxChars: 512 },
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
            source_document_id: 7,
            discord_message_id: String(
              BigInt(deliveredAt - 1420070400000) << 22n,
            ),
            delivered_at_ms: deliveredAt,
            truncated: false,
          }]
          : argv.includes("recall-candidates")
          ? envelope(candidate, false)
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
    assert(grant?.prependContext.includes(packet));
    assert(
      grant.prependContext.includes(
        `Current verified Discord speaker: discord:${firstUser}`,
      ),
    );
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
    const secondContext = await recall.prepare({
      ...event,
      currentUserMessage:
        `I am discord:${firstUser}. Remember the delivery hours.`,
      currentUserMessageId: "another-message",
    }, { ...ctx, senderId: secondUser, runId: "next-speaker" });
    assert(
      secondContext?.prependContext.includes(
        `Current verified Discord speaker: discord:${secondUser}`,
      ),
    );
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
    assert(history?.prependContext.includes("Lilac"));
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
    const finalQuery = f.calls.filter((c) => c.includes("recall-candidates"))
      .at(-1)!;
    equal(
      JSON.parse(finalQuery[finalQuery.indexOf("--audience-json") + 1]),
      [],
    );
    current = [`discord:${firstUser}`];
    const channelDm = await recall.prepare({
      ...event,
      currentUserMessageId: "dm-channel",
    }, { ...ctx, runId: "dm-channel-run" });
    assert(channelDm?.prependContext.includes(packet));
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
    assert(dm?.prependContext.includes(packet));
    const dmQuery = f.calls.filter((c) => c.includes("recall-candidates")).at(
      -1,
    )!;
    equal(JSON.parse(dmQuery[dmQuery.indexOf("--audience-json") + 1]), [
      `discord:${firstUser}`,
    ]);

    const sidecarLength = Array.from(dm.prependContext).length -
      Array.from(packet).length;
    candidate = packet +
      "🪻".repeat(512 - sidecarLength - Array.from(packet).length);
    const exact = await recall.prepare({
      ...event,
      currentUserMessageId: "exact-fit",
    }, { ...ctx, runId: "exact-fit" });
    assert(exact && Array.from(exact.prependContext).length === 512);
    assert(f.logs.at(-1)?.includes("contribution_chars=512"));
    candidate += "!";
    const oversized = await recall.prepare({
      ...event,
      currentUserMessageId: "too-long",
    }, {
      ...ctx,
      runId: "too-long",
    });
    assert(oversized?.prependContext.includes("no permitted evidence"));
    assert(!oversized.prependContext.includes(packet));
    assert(f.logs.at(-1)?.includes("injected=false"));

    acknowledgeOwners = false;
    const from = f.calls.length;
    const legacy = await recall.prepare({
      ...event,
      currentUserMessageId: "older-runtime",
    }, { ...ctx, runId: "older-runtime-run" });
    assert(legacy?.prependContext.includes("clarification"));
    assert(!legacy?.prependContext.includes(packet));
    assert(
      !f.calls.slice(from).some((args) => args.includes("recall-candidates")),
    );
  } finally {
    await f.service.stop();
  }
});

Deno.test("Discord DM recall with native memory disabled follows the admitted human post and its single recipient", async () => {
  const peer = "111111111111111111", channel = "222222222222222222";
  const post = "333333333333333333", other = "444444444444444444";
  const owner = `discord:${peer}`, account = "second";
  const sessionKey = `agent:main:discord:${account}:direct:${peer}`;
  const question = "Which saved label marks the fixture index page?";
  const hash = (text: string) =>
    createHash("sha256").update(text).digest("hex");
  const admissionId = "channel-user:v1:" +
    hash(JSON.stringify(["discord", account, `user:${peer}`, post]));
  const request = {
    currentUserMessage: question,
    currentUserMessageId: admissionId,
  };
  const ctx = {
    ...context,
    sessionId: "dm-session",
    sessionKey,
    channel: "discord",
    accountId: account,
    senderId: peer,
    chatId: peer,
    hookInvocation: { assertActive() {} },
  };
  const original = {
    sender: peer,
    identity: {
      type: "observation",
      id: peer,
      senderKind: "unknown",
      pluginId: "discord",
      accountId: account,
    },
    origin: {
      chatType: "direct",
      conversationId: channel,
      messageProvider: "discord",
      accountId: account,
      senderId: peer,
      sessionKey,
    },
    transport: { channel: "discord", messageId: post },
    sourceTextHash: hash(question),
  };
  let row: typeof original | null = original;
  let recipient = peer;
  let matchingQuery = question;
  let planned: string[] = [];
  let intent: "facts" | "history" | "none" | "clarify" = "facts";
  let afterQuery = () => {};
  const f = fixture({
    judge: (input) =>
      input.candidates ? fixtureJudgement(input) : {
        intent,
        queries: intent === "facts"
          ? [{
            spans: planned.length ? planned : [input.currentText],
            includeTimeline: false,
          }]
          : [],
      },
    result: (argv, input) => {
      const privateQuery = argv.includes("recall-candidates") &&
        argv.includes(JSON.stringify([owner]));
      if (privateQuery) afterQuery();
      const data = argv.includes("bind-room")
        ? { bound: true, owners: [owner] }
        : argv.includes("recall-candidates")
        ? envelope(
          privateQuery && JSON.parse(input!)[0].text === matchingQuery
            ? packet
            : null,
          !privateQuery,
        )
        : { updated: true };
      return { code: 0, stdout: JSON.stringify(data), stderr: "" };
    },
  });
  const slots = { ...f.api.config.plugins.slots, memory: "none" };
  const session = {
    dmScope: "per-account-channel-peer",
    identityLinks: {} as Record<string, string[]>,
  };
  const pluginConfig = { ...f.api.pluginConfig, learningEnabled: true };
  const api = {
    ...f.api,
    pluginConfig,
    config: { plugins: { ...f.api.config.plugins, slots }, session },
  };
  const recall = __moonTest.createMoonPromptRecall(api, {
    client: null,
    stopping: false,
  }, {
    admission: (_api, hook, id, chatType) =>
      Promise.resolve(
        id === admissionId && chatType === "direct" &&
          hook.sessionId === ctx.sessionId
          ? row
          : null,
      ),
    access: (_api, target) => {
      assert(target.accountId === account && target.requireDirect);
      return discordRoomAccess(
        (route: string) =>
          Promise.resolve(
            route === `/channels/${channel}`
              ? { id: channel, type: 1, recipients: [{ id: recipient }] }
              : route === `/channels/${channel}/messages/${post}`
              ? {
                id: post,
                channel_id: channel,
                type: 0,
                author: { id: peer },
                content: request.currentUserMessage,
              }
              : null,
          ),
        target.channelId,
        target.senderId,
        true,
        true,
        target.learningReceipt,
      );
    },
  });
  const privateCalls = () =>
    f.calls.filter((argv) =>
      argv.includes("recall-candidates") &&
      argv.includes(JSON.stringify([owner]))
    ).length;
  try {
    for (
      const [chatId, learning] of [[peer, true], [channel, false]] as const
    ) {
      pluginConfig.learningEnabled = learning;
      const bindsBefore = f.calls.filter((argv) =>
        argv.includes("bind-room")
      ).length;
      const run = { ...ctx, chatId, runId: "dm-" + chatId };
      const allowed = await recall.prepare(request, run);
      assert(allowed?.prependContext.includes(packet));
      assert(
        allowed.prependContext.includes(
          "Current verified Discord speaker: " + owner,
        ),
      );
      equal(
        f.calls.filter((argv) => argv.includes("bind-room")).length,
        bindsBefore + Number(pluginConfig.learningEnabled),
      );
      const outbound = {
        runId: run.runId,
        sessionKey,
        channelId: "discord",
        accountId: account,
      };
      const send = {
        to: `user:${peer}`,
        content: "A private fixture answer.",
        kind: "final",
      };
      equal(await recall.guardDelivery(send, outbound), undefined);
      await recall.confirmDelivery({
        ...send,
        success: true,
        messageId: String(BigInt(Date.now() - 1420070400000) << 22n),
      }, outbound);
      assert(
        !f.calls.some((argv) =>
          argv.includes("history") || argv.includes("reply-history")
        ),
      );
      for (const to of [`user:${peer}`, channel, `channel:${channel}`]) {
        equal(
          await recall.guardDelivery({ kind: "final", to }, outbound),
          undefined,
        );
      }
      for (
        const to of [`user:${other}`, `channel:${other}`, `channel:${peer}`]
      ) {
        equal(await recall.guardDelivery({ kind: "final", to }, outbound), {
          cancel: true,
        });
      }
      recipient = other;
      equal(
        await recall.guardDelivery({ kind: "final", to: channel }, outbound),
        { cancel: true },
      );
      recipient = peer;
    }
    pluginConfig.learningEnabled = true;
    const denied = async (overrides: Record<string, unknown> = {}) => {
      const before = privateCalls();
      const result = await recall.prepare(request, { ...ctx, ...overrides });
      assert(!result?.prependContext?.includes(packet));
      equal(privateCalls(), before);
    };
    for (
      const overrides of [
        { chatId: other },
        { senderId: other },
        { accountId: "other" },
        { sessionKey: "agent:main:discord:channel:" + channel },
        { hookInvocation: undefined },
        { inputProvenance: { kind: "inter_session" } },
        { memoryAudience: { kind: "owner-private", agentId: "main" } },
        { assertMemoryAudienceCurrent() {} },
        {
          memoryAudience: { kind: "owner-private", agentId: "main" },
          assertMemoryAudienceCurrent() {
            throw Error("expired");
          },
        },
      ]
    ) await denied(overrides);
    slots.memory = "native";
    await denied();
    slots.memory = "none";
    session.dmScope = "main";
    await denied();
    session.dmScope = "per-account-channel-peer";
    session.identityLinks[peer] = [`discord:${other}`];
    await denied();
    delete session.identityLinks[peer];
    recipient = other;
    await denied();
    recipient = peer;
    for (
      const invalid of [
        null,
        { ...original, sender: other },
        { ...original, identity: { ...original.identity, accountId: "other" } },
        { ...original, origin: { ...original.origin, conversationId: other } },
        { ...original, origin: { ...original.origin, chatType: "channel" } },
        { ...original, origin: { ...original.origin, groupId: other } },
        { ...original, origin: { ...original.origin, messageThreadId: other } },
        { ...original, transport: { ...original.transport, messageId: other } },
        { ...original, sourceTextHash: hash("A second message.\n" + question) },
      ]
    ) {
      row = invalid;
      await denied();
    }
    const notebook = "What is the cover label for my stargazing notebook?";
    const chinese = "笔记本的封面标签是什么？";
    for (
      const [text, spans] of [
        [notebook + " Give just the label using Moon.", [notebook]],
        [notebook + " I mean the second edition.", [
          notebook,
          "second edition",
        ]],
        [chinese + "请只回答标签。", [chinese]],
      ] as [string, string[]][]
    ) {
      request.currentUserMessage = text;
      row = { ...original, sourceTextHash: hash(text) };
      planned = spans;
      matchingQuery = spans.join(" ");
      const before = privateCalls();
      const result = await recall.prepare(request, {
        ...ctx,
        runId: "wording-" + before,
      });
      assert(result?.prependContext.includes(packet));
      equal(privateCalls(), before + 1);
    }
    for (
      const [source, text] of [
        ["none", notebook.padEnd(1200) + " Please do not use memory."],
        ["history", "Show the original message verbatim."],
        ["clarify", "And the other one?"],
      ] as const
    ) {
      intent = source;
      request.currentUserMessage = text;
      row = { ...original, sourceTextHash: hash(text) };
      const start = f.calls.length;
      const result = await recall.prepare(request, {
        ...ctx,
        runId: "no-read-" + source,
      });
      assert(!result?.prependContext.includes(packet));
      assert(
        !f.calls.slice(start).some((argv) =>
          argv.includes("recall-candidates") || argv.includes("history")
        ),
      );
      if (source === "history") {
        assert(result?.prependContext.includes("unavailable"));
      }
      if (source === "clarify") {
        assert(result?.prependContext.includes("clarify"));
      }
    }
    planned = [];
    intent = "facts";
    request.currentUserMessage = matchingQuery = question;
    recipient = peer;
    row = original;
    afterQuery = () => {
      row = {
        ...original,
        origin: { ...original.origin, conversationId: other },
      };
    };
    const before = privateCalls();
    const replaced = await recall.prepare(request, {
      ...ctx,
      runId: "changed-receipt",
    });
    equal(privateCalls(), before + 1);
    assert(!replaced?.prependContext?.includes(packet));
  } finally {
    await f.service.stop();
  }
});

Deno.test("a human learns in a mixed Discord room while private history and private recall stay withheld", async () => {
  const senderId = "111111111111111111";
  const owner = `discord:${senderId}`;
  const readers = [owner, "bot:discord:222222222222222222"];
  const f = fixture({
    result: (argv) => ({
      code: 0,
      stderr: "",
      stdout: JSON.stringify(
        argv.includes("bind-room")
          ? { bound: true, owners: [owner] }
          : argv.includes("recall-candidates")
          ? { ...envelope(packet), public_only: false }
          : { updated: true },
      ),
    }),
  });
  const recall = __moonTest.createMoonPromptRecall(f.api, {
    client: null,
    stopping: false,
  }, {
    access: () => Promise.resolve({ readers, owners: [owner] }),
    human: () => Promise.resolve(false),
  });
  const ctx = {
    ...context,
    channel: "discord",
    accountId: "default",
    sessionId: "fixture",
    senderId,
    chatId: "333333333333333333",
    assertMemoryAudienceCurrent() {},
    memoryAudience: {
      kind: "conversation",
      agentId: context.agentId,
      sessionKey: context.sessionKey,
      sessionId: "fixture",
    },
  };
  try {
    const response = await recall.prepare({
      ...event,
      currentUserMessage: "Show our chat history",
    }, ctx);
    assert(!response?.prependContext?.includes(packet));
    const bound = f.calls.find((argv) => argv.includes("bind-room"));
    assert(bound);
    equal(JSON.parse(bound[bound.indexOf("--owners-json") + 1]), [owner]);
    equal(
      JSON.parse(bound[bound.indexOf("--audience-json") + 1]),
      [...readers].sort(),
    );
    assert(!f.calls.some((argv) => argv.includes("history")));
    assert(!f.calls.some((argv) => argv.includes("recall-candidates")));
    const facts = await recall.prepare({
      ...event,
      currentUserMessageId: "new-facts",
    }, { ...ctx, runId: "facts" });
    assert(!facts?.prependContext.includes(packet));
    const query = f.calls.find((argv) => argv.includes("recall-candidates"));
    assert(query);
    equal(JSON.parse(query[query.indexOf("--audience-json") + 1]), []);
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
          : argv.includes("recall-candidates")
          ? (privatePacket
            ? envelope(packet, false)
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

Deno.test("the Master profile binds web evidence and requests only public context and history", async () => {
  const profile = "personal-profile";
  const principal = "openclaw:" + profile;
  const master = {
    profile_id: profile,
    memory_principal: principal,
    channels: [],
  };
  const f = fixture({
    result: (argv) => {
      const data = argv.includes("bind-room")
        ? { bound: true, owners: [principal] }
        : argv.includes("recall-candidates")
        ? { ...envelope(), public_only: false }
        : { updated: true };
      return { code: 0, stdout: JSON.stringify(data), stderr: "" };
    },
  });
  const ctx = {
    ...context,
    channel: "webchat",
    sessionId: "web-one",
    sessionKey: "agent:main:web-fixture",
    accountId: "default",
    senderId: profile,
    inputProvenance: { kind: "external_user" },
    memoryAudience: { kind: "owner-private", agentId: "main" },
    assertMemoryAudienceCurrent() {},
  };
  const access = {
    ...f.api,
    config: { ...f.api.config, plugins: { slots: { contextEngine: "moon" } } },
  };
  const recall = __moonTest.createMoonPromptRecall(access, {
    client: null,
    stopping: false,
  }, {
    masterConfig: () => Promise.resolve(master),
    masterTurn: (...args: Parameters<typeof __moonTest.resolveMasterTurn>) =>
      __moonTest.resolveMasterTurn(args[0], args[1], args[2], args[3], {
        admission: () =>
          Promise.resolve({
            identity: { type: "profile", id: profile },
            sender: profile,
            origin: {
              messageProvider: "webchat",
              chatType: "direct",
              sessionKey: ctx.sessionKey,
              senderId: profile,
            },
            transport: { clients: [{ id: "openclaw-control-ui" }] },
          }),
        currentMaster: () => Promise.resolve(master),
        enroll: () => Promise.resolve(false),
        authority: () =>
          Promise.resolve({ senders: new Set(), linkedProfile: profile }),
      }),
  });
  try {
    const shared = await recall.prepare(event, ctx);
    assert(!JSON.stringify(shared ?? {}).includes("Synthetic recall"));
    assert(
      f.calls.some((argv) =>
        argv.includes("bind-room") && argv.includes(principal)
      ),
    );
    assert(
      f.calls.some((argv) =>
        argv.includes("recall-candidates") && argv.includes("[]")
      ),
    );
    const history = await recall.prepare({
      ...event,
      currentUserMessage: "What did we say in our chat history?",
      currentUserMessageId: "second-admission",
    }, { ...ctx, runId: "private-web-run" });
    assert(!JSON.stringify(history ?? {}).includes("Synthetic recall"));
    assert(!f.calls.some((args) => args.includes("history")));
    equal(f.calls.filter((args) => args.includes("bind-room")).length, 2);
  } finally {
    await f.service.stop();
  }
});

Deno.test("unlinking, disabling or rotating the Master keeps cross-channel recall fenced", async () => {
  const original = "111111111111111111";
  const master = {
    profile_id: "p-master",
    memory_principal: "discord:" + original,
    channels: [],
  };
  let configured: typeof master | null = master;
  let links = new Set([original]);
  const protectedPrincipals = new Set<string>();
  const f = fixture({
    result: (argv) => {
      if (argv.includes("--master-memory-principal")) {
        protectedPrincipals.add(
          argv[argv.indexOf("--master-memory-principal") + 1],
        );
      }
      const data = argv.includes("bind-room")
        ? {
          bound: true,
          owners: JSON.parse(argv[argv.indexOf("--owners-json") + 1]),
        }
        : argv.includes("recall-candidates")
        ? { ...envelope(), public_only: false }
        : { updated: true };
      return { code: 0, stderr: "", stdout: JSON.stringify(data) };
    },
  });
  const ctx = {
    ...context,
    channel: "discord",
    senderId: original,
    chatId: original,
    accountId: "default",
    sessionId: "fixture-session",
    inputProvenance: { kind: "external_user" },
    memoryAudience: { kind: "owner-private", agentId: "main" },
    assertMemoryAudienceCurrent() {},
  };
  const recall = __moonTest.createMoonPromptRecall(f.api, {
    client: null,
    stopping: false,
  }, {
    masterConfig: () => Promise.resolve(configured),
    masterSenders: () => Promise.resolve(links),
    protectedMasterPrincipals: () => Promise.resolve([...protectedPrincipals]),
    human: () => Promise.resolve(true),
  });
  try {
    assert(
      (await recall.prepare(event, ctx))?.prependContext.includes(
        master.memory_principal,
      ),
    );
    links = new Set();
    equal(
      await recall.guardDelivery({
        runId: ctx.runId,
        sessionKey: ctx.sessionKey,
        to: original,
      }, {
        runId: ctx.runId,
        sessionKey: ctx.sessionKey,
        channelId: "discord",
        accountId: "default",
      }),
      { cancel: true },
    );
    equal([...protectedPrincipals], [master.memory_principal]);
    for (
      const [index, config] of [
        master,
        null,
        {
          ...master,
          profile_id: "replacement",
          memory_principal: "openclaw:replacement",
        },
      ].entries()
    ) {
      configured = config;
      await recall.prepare({
        ...event,
        currentUserMessageId: "subsequent-" + index,
      }, {
        ...ctx,
        runId: "new-run-" + index,
      });
      const last = f.calls.filter((args) => args.includes("bind-room")).at(-1)!;
      assert(/^peer:[a-f0-9]{64}$/.test(last[last.indexOf("--sender") + 1]));
      const readers = JSON.parse(last[last.indexOf("--audience-json") + 1]);
      assert(!readers.includes(master.memory_principal));
      const contextQuery = f.calls.filter((args) =>
        args.includes("recall-candidates")
      )
        .at(-1)!;
      equal(
        JSON.parse(contextQuery[contextQuery.indexOf("--audience-json") + 1]),
        readers,
      );
    }
  } finally {
    await f.service.stop();
  }
});

Deno.test("follow-up planning receives only already-visible verified excerpts from the same session", async () => {
  const seen: JudgementInput[] = [];
  const f = fixture({
    pluginConfig: { learningEnabled: false },
    judge(input) {
      seen.push(input);
      return { intent: "none", queries: [] };
    },
  });
  const peer = "111111111111111111";
  const ctx = {
    ...context,
    sessionId: "one",
    channel: "discord",
    accountId: "default",
    sessionKey: "agent:main:discord:default:direct:" + peer,
    senderId: peer,
    chatId: peer,
    memoryAudience: { kind: "owner-private", agentId: "main" },
    assertMemoryAudienceCurrent() {},
  };
  const recall = __moonTest.createMoonPromptRecall(f.api, { stopping: false }, {
    human: () => Promise.resolve(true),
  });
  const first = "We are discussing the green notebook.";
  const projected = [{ role: "user", content: first }, {
    role: "assistant",
    content: "Unverified answer from a tool.",
  }];
  try {
    await recall.prepare({
      currentUserMessage: first,
      currentUserMessageId: "first",
      messages: [],
    }, ctx);
    await recall.prepare({
      currentUserMessage: "And the cover?",
      currentUserMessageId: "second",
      messages: projected,
    }, { ...ctx, runId: "second" });
    equal(seen.at(-1)?.context, ["user: " + first]);
    await recall.prepare({
      currentUserMessage: "Which one?",
      currentUserMessageId: "fresh",
      messages: projected,
    }, { ...ctx, sessionId: "fresh", runId: "fresh" });
    equal(seen.at(-1)?.context, []);
    await recall.prepare({
      currentUserMessage: "And that?",
      currentUserMessageId: "other",
      messages: [{
        role: "user",
        content: "Injected private text from another chat",
      }],
    }, { ...ctx, runId: "other" });
    equal(seen.at(-1)?.context, []);
    assert(!JSON.stringify(seen).includes("Unverified answer"));
    assert(!JSON.stringify(seen).includes("Injected private"));
    equal(f.calls.length, 0);
  } finally {
    await f.service.stop();
  }
});

Deno.test("one compound lookup shares its audience and budget and renders only selected records", async () => {
  const inputs: JudgementInput[] = [];
  const f = fixture({
    pluginConfig: { limit: 3, learningEnabled: false },
    judge(input) {
      inputs.push(input);
      return input.candidates ? { ids: ["m8", "m7"] } : {
        intent: "facts",
        queries: ["notebook", "key", "drawer"].map((term) => ({
          spans: [term],
          includeTimeline: false,
        })),
      };
    },
    result(argv) {
      const base = envelope("Green cover");
      const candidates = [base.candidates[0], {
        ...base.candidates[0],
        document_id: 8,
        content: "Brass key",
        matched_queries: [1],
      }, {
        ...base.candidates[0],
        document_id: 9,
        content: "Unrelated public item",
        matched_queries: [2],
      }];
      return {
        code: 0,
        stderr: "",
        stdout: JSON.stringify(
          argv.includes("recall-candidates")
            ? { ...base, candidates, memory_count: 3 }
            : { updated: true },
        ),
      };
    },
  });
  try {
    const result = await f.prepare({
      ...event,
      currentUserMessage: "Which notebook, key and drawer?",
    });
    const packet = JSON.stringify(result);
    assert(packet.includes("Green cover") && packet.includes("Brass key"));
    assert(!packet.includes("Unrelated public item"));
    equal(f.calls.filter((v) => v.includes("recall-candidates")).length, 1);
    equal(f.calls[0][f.calls[0].indexOf("--audience-json") + 1], "[]");
    equal(f.calls[0][f.calls[0].indexOf("--limit") + 1], "3");
    assert(!f.calls[0].includes("--allow-originals"));
    equal(
      JSON.parse(f.inputs[0]!),
      ["notebook", "key", "drawer"].map((text) => ({ text, temporal: false })),
    );
    equal(inputs.length, 2);
    assert(!JSON.stringify(inputs[0]).includes("Green cover"));
  } finally {
    await f.service.stop();
  }
});

Deno.test("an absent model route and an invalid selector cannot disclose stored content", async () => {
  const f = fixture({
    judge: (value) =>
      value.candidates ? { ids: ["m999999"] } : fixtureJudgement(value),
  });
  try {
    equal(
      await f.prepare(event, { ...context, modelId: undefined }),
      undefined,
    );
    const legacy = [{ role: "user", content: event.currentUserMessage }];
    const engine = __moonTest.createMoonContextEngine(f.api);
    assert((await engine.assemble({ messages: legacy })).messages === legacy);
    equal(f.calls.length, 0);
    equal(await f.prepare(), undefined);
    assert(!f.calls.some((args) => args.includes("--injected")));
    assert(!f.errors.join(" ").includes(packet));
  } finally {
    await f.service.stop();
  }
});

import { __moonTest } from "./index.js";

function assert(
  condition: unknown,
  message = "assertion failed",
): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function assertEquals<T>(
  actual: T,
  expected: T,
  message = "values are not equal",
) {
  if (!Object.is(actual, expected)) {
    throw new Error(
      `${message}: expected ${JSON.stringify(expected)}, got ${
        JSON.stringify(actual)
      }`,
    );
  }
}

function assertStringIncludes(
  actual: string,
  expected: string,
  message = "missing substring",
) {
  if (!actual.includes(expected)) {
    throw new Error(
      `${message}: expected ${JSON.stringify(actual)} to include ${
        JSON.stringify(expected)
      }`,
    );
  }
}

async function readPluginManifest() {
  const manifestUrl = new URL("./openclaw.plugin.json", import.meta.url);
  return JSON.parse(await Deno.readTextFile(manifestUrl));
}

function writeSessionFile(filePath: string) {
  const entries = [
    {
      type: "session",
      version: 3,
      id: "session-1",
      timestamp: "2026-03-14T00:00:00.000Z",
      cwd: "/tmp/moon",
    },
    {
      type: "message",
      id: "m1",
      parentId: null,
      timestamp: "2026-03-14T00:00:01.000Z",
      message: {
        role: "user",
        content: [{
          type: "text",
          text: "Summarize the current architecture.",
        }],
        timestamp: 1,
      },
    },
    {
      type: "message",
      id: "m2",
      parentId: "m1",
      timestamp: "2026-03-14T00:00:02.000Z",
      message: {
        role: "assistant",
        content: [{
          type: "text",
          text: "Moon should own the primary context path.",
        }],
        timestamp: 2,
      },
    },
    {
      type: "message",
      id: "m3",
      parentId: "m2",
      timestamp: "2026-03-14T00:00:03.000Z",
      message: {
        role: "user",
        content: [{ type: "text", text: "Keep the fallback path separate." }],
        timestamp: 3,
      },
    },
    {
      type: "message",
      id: "m4",
      parentId: "m3",
      timestamp: "2026-03-14T00:00:04.000Z",
      message: {
        role: "assistant",
        content: [{
          type: "text",
          text: "Understood. I will finish the Moon-owned path first.",
        }],
        timestamp: 4,
      },
    },
  ];

  const raw = `${entries.map((entry) => JSON.stringify(entry)).join("\n")}\n`;
  Deno.writeTextFileSync(filePath, raw);
}

function createApi(
  stdout: string,
  callLog: Array<{ argv: string[]; timeoutMs: number }>,
  pluginConfigOverrides: Record<string, unknown> = {},
  runtimeOverrides: {
    embeddedRunner?: (params: Record<string, unknown>) => Promise<unknown>;
    resolvePath?: (value: string) => string | null | undefined;
    runCommandWithTimeout?: (
      argv: string[],
      opts: { timeoutMs: number; env?: Record<string, string> },
    ) => Promise<{ code: number; stdout: string; stderr: string }> | {
      code: number;
      stdout: string;
      stderr: string;
    };
  } = {},
) {
  return {
    pluginConfig: {
      moonPath: "moon",
      moonHome: "/tmp/moon-home",
      ...pluginConfigOverrides,
    },
    config: {},
    resolvePath(value: string) {
      return runtimeOverrides.resolvePath?.(value) ?? value;
    },
    runtime: {
      system: {
        runCommandWithTimeout(
          argv: string[],
          opts: { timeoutMs: number; env?: Record<string, string> },
        ) {
          if (runtimeOverrides.runCommandWithTimeout) {
            return runtimeOverrides.runCommandWithTimeout(argv, opts);
          }
          callLog.push({ argv, timeoutMs: opts.timeoutMs });
          return {
            code: 0,
            stdout,
            stderr: "",
          };
        },
      },
      agent: {
        runEmbeddedPiAgent: runtimeOverrides.embeddedRunner,
      },
    },
  };
}

Deno.test("moon plugin owns compaction and appends a Moon compaction entry", async () => {
  const tempDir = await Deno.makeTempDir({ prefix: "moon-plugin-test-" });
  try {
    const sessionFile = `${tempDir}/session.jsonl`;
    const assemblyPath = `${tempDir}/assembly.md`;
    const cleansePath = `${tempDir}/cleanse.md`;
    writeSessionFile(sessionFile);

    await Deno.writeTextFile(assemblyPath, "# MOON Assembly Context\n");
    await Deno.writeTextFile(
      cleansePath,
      [
        "---",
        "moon_cleanse: 1",
        'session_id: "session-1"',
        "---",
        "",
        "# Cleanse Summary",
        "## Decisions",
        "- Keep the primary flow under Moon control.",
        "## Open Tasks",
        "- Preserve only the latest active context.",
        "",
      ].join("\n"),
    );

    const stdout = JSON.stringify({
      command: "context-engine",
      ok: true,
      details: [
        `context_engine.assembly_path=${assemblyPath}`,
        `context_engine.cleanse_summary_path=${cleansePath}`,
        "context_engine.cleanse_reason=forced",
      ],
      issues: [],
    });
    const calls: Array<{ argv: string[]; timeoutMs: number }> = [];
    const engine = __moonTest.createMoonContextEngine(createApi(stdout, calls));

    assertEquals(
      engine.info.ownsCompaction,
      true,
      "plugin should advertise compaction ownership",
    );

    const result = await engine.compact({
      sessionId: "session-1",
      sessionFile,
      tokenBudget: 20_000,
      currentTokenCount: 90_000,
      force: true,
    });

    assertEquals(result.ok, true, "compaction should succeed");
    assertEquals(result.compacted, true, "compaction should report success");
    assertEquals(
      result.result?.tokensBefore,
      90_000,
      "tokensBefore should use caller snapshot",
    );
    assert(
      typeof result.result?.tokensAfter === "number",
      "tokensAfter should be reported",
    );
    assert(
      (result.result?.tokensAfter ?? Number.POSITIVE_INFINITY) < 90_000,
      "tokensAfter should shrink",
    );
    assertEquals(calls.length, 1, "context-engine should be invoked once");
    assert(
      calls[0].argv.includes("--allow-out-of-bounds"),
      "context-engine call should bypass workspace boundary in embedded runtime",
    );
    assert(
      calls[0].argv.includes("--force-cleanse"),
      "compact should force Moon cleanse",
    );

    const entries = __moonTest.parseJsonlEntries(
      await Deno.readTextFile(sessionFile),
    );
    const lastEntry = entries[entries.length - 1];
    assertEquals(
      lastEntry.type,
      "compaction",
      "session should end with a compaction entry",
    );
    assertEquals(
      lastEntry.parentId,
      "m4",
      "compaction should attach to the current leaf",
    );
    assertStringIncludes(
      lastEntry.summary,
      "Keep the primary flow under Moon control.",
    );
    assert(
      typeof lastEntry.firstKeptEntryId === "string" &&
        lastEntry.firstKeptEntryId.length > 0,
    );
    assertEquals(
      lastEntry.tokensBefore,
      90_000,
      "persisted compaction should carry tokensBefore",
    );
    assertEquals(lastEntry.details.moon.cleanseSummaryPath, cleansePath);
    assertEquals(lastEntry.details.moon.assemblyPath, assemblyPath);
  } finally {
    await Deno.remove(tempDir, { recursive: true });
  }
});

Deno.test("moon plugin requests OpenClaw fallback when Moon does not emit a cleanse summary", async () => {
  const tempDir = await Deno.makeTempDir({ prefix: "moon-plugin-test-" });
  try {
    const sessionFile = `${tempDir}/session.jsonl`;
    const assemblyPath = `${tempDir}/assembly.md`;
    writeSessionFile(sessionFile);
    await Deno.writeTextFile(assemblyPath, "# MOON Assembly Context\n");

    const stdout = JSON.stringify({
      command: "context-engine",
      ok: true,
      details: [
        `context_engine.assembly_path=${assemblyPath}`,
        "context_engine.cleanse_summary_path=none",
        "context_engine.cleanse_reason=no-pressure-snapshot",
      ],
      issues: [],
    });
    const engine = __moonTest.createMoonContextEngine(
      createApi(stdout, [], {
        fallbackMode: "openclaw",
        compactFallbackOnSkip: true,
      }),
    );
    const before =
      __moonTest.parseJsonlEntries(await Deno.readTextFile(sessionFile)).length;
    const errors: string[] = [];
    const originalConsoleError = console.error;
    console.error = (...args: unknown[]) => {
      errors.push(args.map((value) => String(value)).join(" "));
    };

    try {
      const result = await engine.compact({
        sessionId: "session-1",
        sessionFile,
        tokenBudget: 20_000,
        currentTokenCount: 5_000,
        force: false,
      });

      assertEquals(result.ok, false, "skip should request fallback");
      assertEquals(
        result.compacted,
        false,
        "fallback request should not report compaction",
      );
      assertStringIncludes(
        result.reason ?? "",
        "moon->openclaw fallback trigger=compact-skip",
      );
      assertStringIncludes(result.reason ?? "", "moon cleanse did not trigger");
    } finally {
      console.error = originalConsoleError;
    }

    const after =
      __moonTest.parseJsonlEntries(await Deno.readTextFile(sessionFile)).length;
    assertEquals(after, before, "fallback request should not append entries");
    assert(errors.length >= 2, "fallback path should log skip + fallback");
    assertStringIncludes(
      errors.join("\n"),
      "missing cleanse summary during compaction",
    );
    assertStringIncludes(errors.join("\n"), "session_id=session-1");
    assertStringIncludes(
      errors.join("\n"),
      "reason=moon cleanse did not trigger",
    );
    assertStringIncludes(
      errors.join("\n"),
      "moon->openclaw fallback trigger=compact-skip",
    );
  } finally {
    await Deno.remove(tempDir, { recursive: true });
  }
});

Deno.test("moon plugin can keep primary-only compaction skip behavior when fallback is disabled", async () => {
  const tempDir = await Deno.makeTempDir({ prefix: "moon-plugin-test-" });
  try {
    const sessionFile = `${tempDir}/session.jsonl`;
    const assemblyPath = `${tempDir}/assembly.md`;
    writeSessionFile(sessionFile);
    await Deno.writeTextFile(assemblyPath, "# MOON Assembly Context\n");

    const stdout = JSON.stringify({
      command: "context-engine",
      ok: true,
      details: [
        `context_engine.assembly_path=${assemblyPath}`,
        "context_engine.cleanse_summary_path=none",
        "context_engine.cleanse_reason=no-pressure-snapshot",
      ],
      issues: [],
    });
    const engine = __moonTest.createMoonContextEngine(
      createApi(stdout, [], { fallbackMode: "disabled" }),
    );

    const result = await engine.compact({
      sessionId: "session-1",
      sessionFile,
      tokenBudget: 20_000,
      currentTokenCount: 5_000,
      force: false,
    });

    assertEquals(
      result.ok,
      true,
      "disabled fallback should keep skip semantics",
    );
    assertEquals(result.compacted, false, "skip should not report compaction");
    assertStringIncludes(result.reason ?? "", "moon cleanse did not trigger");
  } finally {
    await Deno.remove(tempDir, { recursive: true });
  }
});

Deno.test("moon plugin assemble injects the Moon packet into messages and keeps systemPromptAddition empty", async () => {
  const tempDir = await Deno.makeTempDir({ prefix: "moon-plugin-test-" });
  try {
    const assemblyPath = `${tempDir}/assembly.md`;
    const packetPath = `${tempDir}/packet.md`;
    await Deno.writeTextFile(
      assemblyPath,
      "# MOON Assembly Context\n\n## Control Summary\n- session_id: session-1\n",
    );
    await Deno.writeTextFile(
      packetPath,
      [
        "# Moon Active Context",
        "",
        "## Current Goal",
        "- Keep Moon retrieval in the messages lane.",
      ].join("\n"),
    );

    const stdout = JSON.stringify({
      command: "context-engine",
      ok: true,
      details: [
        `context_engine.assembly_path=${assemblyPath}`,
        `context_engine.packet_path=${packetPath}`,
        "context_engine.packet_candidate_count=2",
        "context_engine.packet_cache_hit=false",
        "context_engine.packet_query=messages lane",
        "context_engine.cleanse_summary_path=none",
        "context_engine.cleanse_reason=no-pressure-snapshot",
      ],
      issues: [],
    });
    const calls: Array<{ argv: string[]; timeoutMs: number }> = [];
    const engine = __moonTest.createMoonContextEngine(createApi(stdout, calls));

    const messages = [{
      role: "user",
      content: [{ type: "text", text: "hello" }],
    }];
    const result = await engine.assemble({
      sessionId: "session-1",
      messages,
      tokenBudget: 20_000,
    });

    assertEquals(calls.length, 1, "assemble should invoke context-engine once");
    assertEquals(Array.isArray(result.messages), true);
    assertEquals(result.messages.length, 2);
    assertEquals(result.messages[0]?.role, "assistant");
    assertStringIncludes(
      JSON.stringify(result.messages[0]),
      "Moon Active Context",
      "assemble should inject the packet as a synthetic message",
    );
    assertEquals(result.messages[1]?.role, "user");
    assertEquals(
      Object.prototype.hasOwnProperty.call(result, "systemPromptAddition"),
      false,
      "routine Moon assembly should not inject system prompt text",
    );
  } finally {
    await Deno.remove(tempDir, { recursive: true });
  }
});

Deno.test("moon plugin keeps cleanse summary in transcript compaction lane only", async () => {
  const tempDir = await Deno.makeTempDir({ prefix: "moon-plugin-test-" });
  try {
    const sessionFile = `${tempDir}/session.jsonl`;
    const assemblyPath = `${tempDir}/assembly.md`;
    const packetPath = `${tempDir}/packet.md`;
    const cleansePath = `${tempDir}/cleanse.md`;
    writeSessionFile(sessionFile);

    await Deno.writeTextFile(
      assemblyPath,
      [
        "# MOON Assembly Context",
        "",
        "## Control Summary",
        "- session_id: session-1",
        "- cleanse_summary: present",
        "",
        "## Cleanse Summary",
        "- Preserve the active Moon prompt boundary.",
        "",
      ].join("\n"),
    );
    await Deno.writeTextFile(
      packetPath,
      [
        "# Moon Active Context",
        "",
        "## Active Work",
        "- Continue the bounded Moon packet rollout.",
        "",
      ].join("\n"),
    );
    await Deno.writeTextFile(
      cleansePath,
      [
        "---",
        "moon_cleanse: 1",
        'session_id: "session-1"',
        "---",
        "",
        "# Cleanse Summary",
        "- Preserve the active Moon prompt boundary.",
        "",
      ].join("\n"),
    );

    const stdout = JSON.stringify({
      command: "context-engine",
      ok: true,
      details: [
        `context_engine.assembly_path=${assemblyPath}`,
        `context_engine.packet_path=${packetPath}`,
        "context_engine.packet_candidate_count=2",
        "context_engine.packet_cache_hit=false",
        "context_engine.packet_query=bounded moon packet",
        `context_engine.cleanse_summary_path=${cleansePath}`,
        "context_engine.cleanse_reason=forced",
      ],
      issues: [],
    });
    const calls: Array<{ argv: string[]; timeoutMs: number }> = [];
    const engine = __moonTest.createMoonContextEngine(createApi(stdout, calls));

    const compactResult = await engine.compact({
      sessionId: "session-1",
      sessionFile,
      tokenBudget: 20_000,
      currentTokenCount: 80_000,
      force: true,
    });

    assertEquals(compactResult.ok, true, "compaction should succeed");
    assertEquals(compactResult.compacted, true, "compaction should append");

    const entries = __moonTest.parseJsonlEntries(
      await Deno.readTextFile(sessionFile),
    );
    const lastEntry = entries[entries.length - 1];
    assertEquals(lastEntry.type, "compaction");
    assertStringIncludes(
      lastEntry.summary,
      "Preserve the active Moon prompt boundary.",
      "cleanse summary should be preserved in transcript compaction entry",
    );

    const messages = [{
      role: "compactionSummary",
      summary: "Preserve the active Moon prompt boundary.",
      tokensBefore: 80_000,
      timestamp: 1,
    }, {
      role: "user",
      content: [{ type: "text", text: "continue" }],
    }];
    const assembleResult = await engine.assemble({
      sessionId: "session-1",
      messages,
      tokenBudget: 20_000,
    });

    assertEquals(
      Object.prototype.hasOwnProperty.call(
        assembleResult,
        "systemPromptAddition",
      ),
      false,
      "assemble should not duplicate compaction summary in system prompt text",
    );
    assert(
      !JSON.stringify(assembleResult.messages[1] ?? {}).includes(
        "Preserve the active Moon prompt boundary.",
      ),
      "injected Moon packet should not duplicate the compaction summary text",
    );
    assert(
      calls.some((call) =>
        call.argv.includes("--replay-has-compaction-summary")
      ),
      "assemble should tell Moon when compactionSummary already exists in replay",
    );
  } finally {
    await Deno.remove(tempDir, { recursive: true });
  }
});

Deno.test("moon plugin can gate packet curation through an embedded Moon subagent", async () => {
  const tempDir = await Deno.makeTempDir({ prefix: "moon-plugin-test-" });
  try {
    const assemblyPath = `${tempDir}/assembly.md`;
    const packetPath = `${tempDir}/packet.md`;
    await Deno.writeTextFile(assemblyPath, "# MOON Assembly Context\n");
    await Deno.writeTextFile(
      packetPath,
      [
        "# Moon Active Context",
        "",
        "## Current Goal",
        "- Local packet before curation.",
        "",
        "## Evidence",
        "- Candidate A",
        "- Candidate B",
      ].join("\n"),
    );

    const stdout = JSON.stringify({
      command: "context-engine",
      ok: true,
      details: [
        `context_engine.assembly_path=${assemblyPath}`,
        `context_engine.packet_path=${packetPath}`,
        "context_engine.packet_candidate_count=12",
        "context_engine.packet_cache_hit=false",
        "context_engine.packet_query=curate the packet",
        "context_engine.cleanse_summary_path=none",
        "context_engine.cleanse_reason=no-pressure-snapshot",
      ],
      issues: [],
    });
    const calls: Array<{ argv: string[]; timeoutMs: number }> = [];
    const embeddedCalls: Array<Record<string, unknown>> = [];
    const engine = __moonTest.createMoonContextEngine(
      createApi(
        stdout,
        calls,
        {
          assemblySubagentMode: "gated",
          assemblySubagentProvider: "openai-codex",
          assemblySubagentModel: "gpt-5.4",
          contextPacketCandidateThreshold: 1,
        },
        {
          embeddedRunner: (params) => {
            embeddedCalls.push(params);
            return Promise.resolve({
              payloads: [{
                text:
                  "# Moon Active Context\n\n## Current Goal\n- Curated packet.\n",
              }],
            });
          },
        },
      ),
    );

    const params = {
      sessionId: "session-curated",
      sessionKey: "agent:main:test",
      messages: [{
        role: "user",
        content: [{ type: "text", text: "Use recall." }],
      }],
      tokenBudget: 20_000,
      prompt: "Recall the most relevant current context.",
    };
    const first = await engine.assemble(params);
    const second = await engine.assemble(params);

    assertEquals(
      calls.length,
      2,
      "context-engine should still run on both turns",
    );
    assertEquals(embeddedCalls.length, 1, "curator result should be cached");
    assertStringIncludes(JSON.stringify(first.messages[0]), "Curated packet");
    assertStringIncludes(JSON.stringify(second.messages[0]), "Curated packet");
  } finally {
    await Deno.remove(tempDir, { recursive: true });
  }
});

Deno.test("moon plugin resolves fast default curator models by provider", () => {
  const geminiSettings = __moonTest.resolveContextEngineSettings(
    createApi("", [], {
      assemblySubagentMode: "gated",
      assemblySubagentProvider: "google",
    }),
  );
  assertEquals(geminiSettings.assemblySubagentProvider, "google");
  assertEquals(
    geminiSettings.assemblySubagentModel,
    "gemini-3.1-flash-lite-preview",
  );

  const codexSettings = __moonTest.resolveContextEngineSettings(
    createApi("", [], {
      assemblySubagentMode: "gated",
      assemblySubagentProvider: "openai-codex",
    }),
  );
  assertEquals(codexSettings.assemblySubagentProvider, "openai-codex");
  assertEquals(codexSettings.assemblySubagentModel, "gpt-5.4-mini");

  const inferredOpenAiSettings = __moonTest.resolveContextEngineSettings(
    createApi("", [], {
      assemblySubagentMode: "gated",
      assemblySubagentModel: "gpt-5.4-mini",
    }),
  );
  assertEquals(inferredOpenAiSettings.assemblySubagentProvider, "openai");
  assertEquals(inferredOpenAiSettings.assemblySubagentModel, "gpt-5.4-mini");
});

Deno.test("moon plugin keeps configured absolute moonPath when host resolver returns empty", () => {
  const settings = __moonTest.resolveContextEngineSettings(
    createApi(
      "",
      [],
      {
        moonPath: "/tmp/moon-bin/moon",
      },
      {
        resolvePath: () => "",
      },
    ),
  );

  assertEquals(
    settings.moonPath,
    "/tmp/moon-bin/moon",
    "absolute configured moonPath should not silently fall back to bare moon",
  );
});

Deno.test("moon plugin still resolves relative moonPath through the host resolver", () => {
  const settings = __moonTest.resolveContextEngineSettings(
    createApi(
      "",
      [],
      {
        moonPath: "bin/moon",
      },
      {
        resolvePath: (value) => `/resolved/${value}`,
      },
    ),
  );

  assertEquals(settings.moonPath, "/resolved/bin/moon");
});

Deno.test("moon plugin falls back to the local packet when curator fails", async () => {
  const tempDir = await Deno.makeTempDir({ prefix: "moon-plugin-test-" });
  try {
    const assemblyPath = `${tempDir}/assembly.md`;
    const packetPath = `${tempDir}/packet.md`;
    await Deno.writeTextFile(assemblyPath, "# MOON Assembly Context\n");
    await Deno.writeTextFile(
      packetPath,
      "# Moon Active Context\n\n## Current Goal\n- Local packet survives.\n",
    );

    const stdout = JSON.stringify({
      command: "context-engine",
      ok: true,
      details: [
        `context_engine.assembly_path=${assemblyPath}`,
        `context_engine.packet_path=${packetPath}`,
        "context_engine.packet_candidate_count=99",
        "context_engine.packet_cache_hit=false",
        "context_engine.packet_query=why did we do this",
        "context_engine.cleanse_summary_path=none",
        "context_engine.cleanse_reason=no-pressure-snapshot",
      ],
      issues: [],
    });
    const engine = __moonTest.createMoonContextEngine(
      createApi(
        stdout,
        [],
        {
          assemblySubagentMode: "gated",
          assemblySubagentProvider: "openai-codex",
          assemblySubagentModel: "gpt-5.4",
          contextPacketCandidateThreshold: 1,
        },
        {
          embeddedRunner: () => Promise.reject(new Error("timeout")),
        },
      ),
    );

    const result = await engine.assemble({
      sessionId: "session-fallback",
      sessionKey: "agent:main:test",
      messages: [{
        role: "user",
        content: [{ type: "text", text: "What happened?" }],
      }],
      tokenBudget: 20_000,
      prompt: "Recall the most relevant current context.",
    });

    assertStringIncludes(
      JSON.stringify(result.messages[0]),
      "Local packet survives",
    );
  } finally {
    await Deno.remove(tempDir, { recursive: true });
  }
});

Deno.test("moon plugin logs executable details when launch fails before spawn", async () => {
  const errors: string[] = [];
  const originalConsoleError = console.error;
  console.error = (...args: unknown[]) => {
    errors.push(args.map((value) => String(value)).join(" "));
  };

  try {
    const engine = __moonTest.createMoonContextEngine(
      createApi(
        "",
        [],
        {
          moonPath: "/tmp/moon-bin/moon",
          fallbackMode: "openclaw",
        },
        {
          runCommandWithTimeout() {
            throw new Error("spawn moon ENOENT");
          },
        },
      ),
    );

    const result = await engine.assemble({
      sessionId: "session-launch-error",
      messages: [{
        role: "user",
        content: [{ type: "text", text: "hello" }],
      }],
      tokenBudget: 20_000,
    });

    assertEquals(result.messages.length, 1);
    assertStringIncludes(errors.join("\n"), "executable=/tmp/moon-bin/moon");
    assertStringIncludes(errors.join("\n"), "process_cwd=");
    assertStringIncludes(errors.join("\n"), "spawn moon ENOENT");
  } finally {
    console.error = originalConsoleError;
  }
});

Deno.test("moon plugin manifest keeps maxAssemblyChars as a deprecated compatibility key", async () => {
  const manifest = await readPluginManifest();
  const properties = manifest?.configSchema?.properties ?? {};

  assertEquals(
    typeof properties.maxAssemblyChars,
    "object",
    "manifest should continue to accept legacy maxAssemblyChars config",
  );
  assertEquals(
    properties.maxAssemblyChars.minimum,
    1000,
    "legacy maxAssemblyChars minimum should stay stable",
  );
  assertEquals(
    properties.maxAssemblyChars.maximum,
    200000,
    "legacy maxAssemblyChars maximum should stay stable",
  );
});

Deno.test("moon plugin falls back to base assembly output when context-engine fails", async () => {
  const calls: Array<{ argv: string[]; timeoutMs: number }> = [];
  const api = {
    pluginConfig: {
      moonPath: "moon",
      moonHome: "/tmp/moon-home",
      fallbackMode: "openclaw",
    },
    runtime: {
      system: {
        runCommandWithTimeout(
          argv: string[],
          opts: { timeoutMs: number },
        ) {
          calls.push({ argv, timeoutMs: opts.timeoutMs });
          return { code: 1, stdout: "", stderr: "context-engine failed" };
        },
      },
    },
  };
  const engine = __moonTest.createMoonContextEngine(api);

  const messages = [{
    role: "user",
    content: [{ type: "text", text: "hello" }],
  }];
  const result = await engine.assemble({
    sessionId: "session-1",
    messages,
    tokenBudget: 20_000,
  });

  assertEquals(
    calls.length,
    1,
    "assemble should still invoke context-engine once",
  );
  assertEquals(Array.isArray(result.messages), true);
  assertEquals(result.messages.length, 1);
  assertEquals(
    Object.prototype.hasOwnProperty.call(result, "systemPromptAddition"),
    false,
    "fallback assembly should not inject moon system prompt content",
  );
});

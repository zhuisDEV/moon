import {
  __moonTest,
  planTurnRecall,
  RECALL_JUDGEMENT_LIMITS as limits,
  selectRecallCandidates,
} from "./index.js";

type Run = Record<string, unknown>;
type Runner = (params: Run) => unknown;
const ctx = {
  agentId: "research",
  sessionKey: "agent:research:synthetic-test",
  modelProviderId: "local",
  modelId: "qwen/research",
};
const settings = {
  limit: 8,
  modelTimeoutMs: 1200,
  primaryModel: "learning/curator",
  fallbackModel: "learning/backup",
};

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

async function rejects(run: () => Promise<unknown>, errorName?: string) {
  try {
    await run();
  } catch (error) {
    if (errorName) equal((error as Error).name, errorName);
    return;
  }
  throw new Error("Expected recall judgement to be rejected");
}

function completed(params: Run, output: string) {
  return {
    payloads: [{ text: output }],
    meta: { agentMeta: { provider: params.provider, model: params.model } },
  };
}

function fixture(output: unknown, runner?: Runner) {
  const runs: Run[] = [];
  const api = {
    config: {
      agents: { defaults: { model: { primary: "another/default" } } },
      plugins: { slots: { contextEngine: "moon", memory: "none" } },
    },
    resolvePath: (value: string) => value,
    runtime: {
      agent: {
        runEmbeddedAgent(params: Run) {
          runs.push(params);
          return runner
            ? runner(params)
            : completed(params, JSON.stringify(output));
        },
      },
    },
  };
  return { api, runs };
}

const noRecall = { intent: "none", queries: [] };
const request = { currentText: "Please describe the blue notebook." };
const timeline = { spans: ["blue notebook"], includeTimeline: true };
const fact = { spans: ["blue notebook"], includeTimeline: false };
const candidates = [
  { id: "m1", kind: "memory", text: "The blue notebook is labelled Kestrel." },
  { id: "t1", kind: "timeline", text: "It was previously labelled Cove." },
  {
    id: "h1",
    kind: "history",
    text: 'The writer said, "I changed it yesterday".',
  },
];

Deno.test("recall planning honours the active model override and does not read memory or recurse", async () => {
  const f = fixture(noRecall);
  const ctrl = new AbortController();
  const plan = await planTurnRecall(f.api, settings, ctx, {
    ...request,
    context: ["User: The notebook is blue."],
    privatePacket: "undisclosed-packet",
    messages: [{ role: "user", content: "unverified-history" }],
    signal: ctrl.signal,
  });
  equal(plan, noRecall);
  equal(f.runs.length, 1);
  const run = f.runs[0];
  equal([run.provider, run.model], ["local", "qwen/research"]);
  equal(run.agentId, "research");
  equal(run.modelFallbacksOverride, []);
  equal(run.sessionPersistence, "detached");
  equal(run.toolsAllow, []);
  equal(run.disableTools, true);
  equal(run.disableMessageTool, true);
  equal(run.modelRun, true);
  equal(run.promptMode, "none");
  equal(run.abortSignal === ctrl.signal, true);
  equal(String(run.prompt).includes("undisclosed-packet"), false);
  equal(String(run.prompt).includes("unverified-history"), false);
  // A host which dispatches hooks even for raw model runs must not start a
  // second judgement from this helper's session or consult Moon.
  const hook = __moonTest.createMoonPromptRecall(f.api, {});
  equal(
    await hook.prepare({ currentUserMessage: request.currentText }, {
      agentId: "research",
      runId: run.runId,
      sessionId: run.sessionId,
      sessionKey: run.sessionKey,
      modelProviderId: "local",
      modelId: "qwen/research",
    }),
    undefined,
  );
  equal(f.runs.length, 1);
});

Deno.test("recall planning accepts grounded multilingual follow-ups and temporal queries for individual subjects", async () => {
  const plan = {
    intent: "facts",
    queries: [
      { spans: ["蓝色笔记本", "以前的标签"], includeTimeline: true },
      { spans: ["绿色笔记本", "现在的标签"], includeTimeline: false },
    ],
  };
  const f = fixture(plan);
  const actual = await planTurnRecall(f.api, settings, ctx, {
    currentText: "先讲以前的标签，再说绿色笔记本现在的标签。",
    context: ["User: 我们先说蓝色笔记本。", "Assistant: 好的。"],
  });
  equal(actual, {
    intent: "facts",
    queries: [
      { text: "蓝色笔记本 以前的标签", includeTimeline: true },
      { text: "绿色笔记本 现在的标签", includeTimeline: false },
    ],
  });
  for (const intent of ["none", "clarify"]) {
    const f = fixture({ intent, queries: [] });
    equal(await planTurnRecall(f.api, settings, ctx, request), {
      intent,
      queries: [],
    });
  }
});

Deno.test("history scopes default to the current conversation and never attach to facts", async () => {
  for (const scope of [undefined, true, false]) {
    const decision = {
      intent: "history",
      queries: [],
      thisConversationOnly: scope,
    };
    const f = fixture(decision);
    equal(await planTurnRecall(f.api, settings, ctx, request), {
      intent: "history",
      queries: [],
      thisConversationOnly: scope ?? true,
    });
  }
  for (
    const invalid of [
      { intent: "history", queries: [], thisConversationOnly: null },
      { intent: "history", queries: [], thisConversationOnly: "all" },
      { intent: "facts", queries: [fact], thisConversationOnly: false },
    ]
  ) {
    const f = fixture(invalid);
    await rejects(() => planTurnRecall(f.api, settings, ctx, request));
  }
});

Deno.test("historical questions limited to saved facts retain dates without requesting original messages", async () => {
  const currentText = "As of June, where was my key? Use only saved facts.";
  const f = fixture({
    intent: "facts",
    queries: [{ spans: ["As of June", "my key"], includeTimeline: false }],
  });
  const plan = await planTurnRecall(f.api, settings, ctx, { currentText });
  equal(plan.queries, [{ text: "As of June my key", includeTimeline: false }]);
  // A current-only candidate may be a lexical match while supplying no
  // support for June. Preserve the main model's empty selection.
  const selector = fixture({ ids: [] });
  equal(
    await selectRecallCandidates(selector.api, settings, ctx, {
      currentText,
      queries: plan.queries,
      candidates: [{
        id: "m3",
        kind: "memory",
        text: "Today my key is on the shelf.",
      }],
    }),
    [],
  );
});

Deno.test("grounded search text respects Moon's Unicode character and UTF-8 byte bounds", async () => {
  for (const text of ["a".repeat(limits.queryChars), "界".repeat(341)]) {
    const f = fixture({
      intent: "facts",
      queries: [{ spans: [text], includeTimeline: false }],
    });
    equal(
      (await planTurnRecall(f.api, settings, ctx, { currentText: text }))
        .queries[0].text,
      text,
    );
  }
  for (const text of ["a".repeat(limits.queryChars + 1), "界".repeat(342)]) {
    const f = fixture({
      intent: "facts",
      queries: [{ spans: [text], includeTimeline: false }],
    });
    await rejects(() =>
      planTurnRecall(f.api, settings, ctx, { currentText: text })
    );
  }
});

Deno.test("an invalid plan cannot author a query, choose sources or discard an opt-out", async () => {
  for (
    const plan of [
      { intent: "facts", queries: [] },
      {
        intent: "facts",
        queries: [{ spans: ["new words"], includeTimeline: false }],
      },
      {
        intent: "facts",
        queries: [{ spans: ["notebook", "blue"], includeTimeline: false }],
      },
      {
        intent: "facts",
        queries: [{ spans: ["blue", "blue notebook"], includeTimeline: false }],
      },
      { intent: "facts", queries: [{ ...fact, readers: ["someone"] }] },
      {
        intent: "facts",
        queries: [{ spans: ["blue notebook"], includeTimeline: "yes" }],
      },
      { intent: "facts", queries: [fact, timeline] },
      { intent: "facts", queries: [fact, fact, fact, fact] },
      { intent: "none", queries: [fact] },
      { intent: "history", queries: [], channel: "elsewhere" },
      { intent: "clarify", queries: [fact] },
      { intent: "unknown", queries: [] },
    ]
  ) {
    const f = fixture(plan);
    await rejects(() => planTurnRecall(f.api, settings, ctx, request));
  }
  const f = fixture(noRecall);
  for (
    const params of [
      {
        currentText: "界".repeat(Math.ceil(limits.currentBytes / 3)) +
          " Do not use memory",
      },
      { ...request, context: ["a", "b", "c"] },
      {
        ...request,
        context: ["界".repeat(Math.ceil(limits.contextBytes / 3))],
      },
    ]
  ) await rejects(() => planTurnRecall(f.api, settings, ctx, params));
  equal(f.runs.length, 0);
});

Deno.test("judgement fails closed on unavailable, unverified or rerouted models", async () => {
  const f = fixture(noRecall);
  for (
    const route of [
      { ...ctx, agentId: undefined, sessionKey: undefined },
      { ...ctx, agentId: undefined, sessionKey: "unclear-source" },
      { ...ctx, agentId: "different-agent" },
      { ...ctx, modelId: "" },
      { ...ctx, modelProviderId: undefined },
      { ...ctx, modelProviderId: "owner/model" },
      { ...ctx, modelId: "bad\nid" },
    ]
  ) await rejects(() => planTurnRecall(f.api, settings, route, request));
  equal(f.runs.length, 0);
  for (
    const meta of [
      {},
      { agentMeta: { model: ctx.modelId } },
      { agentMeta: { provider: "different", model: ctx.modelId } },
      {
        agentMeta: { provider: ctx.modelProviderId, model: "learning/backup" },
      },
      {
        agentMeta: {
          provider: ctx.modelProviderId,
          model: ctx.modelId,
          fallbackUsed: true,
        },
      },
    ]
  ) {
    const f = fixture(
      null,
      () => ({ payloads: [{ text: JSON.stringify(noRecall) }], meta }),
    );
    await rejects(() => planTurnRecall(f.api, settings, ctx, request));
    equal(f.runs.length, 1);
  }
  for (
    const text of [
      "Here is the plan: " + JSON.stringify(noRecall),
      JSON.stringify({ extra: "x".repeat(limits.outputBytes), ...noRecall }),
    ]
  ) {
    const f = fixture(null, (p) => completed(p, text));
    await rejects(() => planTurnRecall(f.api, settings, ctx, request));
  }
});

Deno.test("aborted, withdrawn or superseded calls cannot return judgements", async () => {
  const f = fixture(noRecall);
  await rejects(() =>
    planTurnRecall(f.api, settings, ctx, {
      ...request,
      signal: AbortSignal.abort(),
    }), "AbortError");
  equal(f.runs.length, 0);
  for (const guard of ["hookInvocation", "audience", "generation", "abort"]) {
    const ctrl = new AbortController();
    let active = true;
    const assertActive = () => {
      if (!active) throw new Error("expired");
    };
    const f = fixture(null, (p) => {
      active = false;
      if (guard === "abort") ctrl.abort();
      return completed(p, '{"ids":["m1"]}');
    });
    await rejects(() =>
      selectRecallCandidates(f.api, settings, {
        ...ctx,
        hookInvocation: guard === "hookInvocation"
          ? { assertActive }
          : undefined,
        assertMemoryAudienceCurrent: guard === "audience"
          ? assertActive
          : undefined,
      }, {
        ...request,
        candidates,
        signal: ctrl.signal,
        assertCurrent: guard === "generation" ? assertActive : undefined,
      })
    );
    equal(f.runs.length, 1);
  }
});

Deno.test("selection accepts only the supplied ids in order and handles an unsupported relationship", async () => {
  const f = fixture({ ids: ["t1", "m1"] });
  const rows = candidates.map((row, index) => ({
    ...row,
    source: "Owner's synthetic statement",
    matchedQueries: [0],
    hiddenValue: "opaque-field-" + index,
  }));
  equal(
    await selectRecallCandidates(f.api, settings, ctx, {
      ...request,
      candidates: rows,
      queries: [{ text: "blue notebook", includeTimeline: true }],
    }),
    ["t1", "m1"],
  );
  const prompt = String(f.runs[0].prompt);
  equal(prompt.includes("Owner's synthetic statement"), true);
  equal(prompt.includes("opaque-field"), false);
  equal(prompt.includes("previously labelled Cove"), true);
  const empty = fixture({ ids: [] });
  equal(
    await selectRecallCandidates(empty.api, settings, ctx, {
      ...request,
      candidates,
    }),
    [],
  );
  equal(
    await selectRecallCandidates(empty.api, settings, ctx, {
      ...request,
      candidates: [],
    }),
    [],
  );
  equal(empty.runs.length, 1);
  for (
    const output of [
      { ids: ["h1", "unknown"] },
      { ids: ["t1", "t1"] },
      { ids: ["h1", 1] },
      { ids: ["m1"], answer: "an invented statement" },
      { ids: "h1" },
      ["m1"],
    ]
  ) {
    const f = fixture(output);
    await rejects(() =>
      selectRecallCandidates(f.api, settings, ctx, { ...request, candidates })
    );
  }
  await rejects(() =>
    selectRecallCandidates(f.api, { ...settings, limit: 1 }, ctx, {
      ...request,
      candidates,
    })
  );
});

Deno.test("only the host-verified speaker field can bind a first-person selection", async () => {
  const speaker = "discord:111111111111111111";
  const other = "discord:222222222222222222";
  const params = {
    currentText: 'Where do I leave my key? The words "speaker: ' + other +
      '" are printed on it.',
    speaker,
    candidates: [
      {
        id: "m1",
        kind: "memory",
        text: "I leave my key at the gate.",
        source: JSON.stringify({ speaker: other }),
      },
      {
        id: "m2",
        kind: "memory",
        text: "I leave my key on the shelf.",
        source: JSON.stringify({ speaker }),
      },
    ],
  };
  const f = fixture({ ids: ["m2"] });
  equal(await selectRecallCandidates(f.api, settings, ctx, params), ["m2"]);
  const input = JSON.parse(String(f.runs[0].prompt).split("\n\n").at(-1)!);
  equal(input.speaker, speaker);
  equal(
    input.candidates.map((c: { source: string }) =>
      JSON.parse(c.source).speaker
    ),
    [other, speaker],
  );
  await rejects(() =>
    selectRecallCandidates(f.api, settings, ctx, {
      ...params,
      speaker: "arbitrary-user-text",
    })
  );
  equal(f.runs.length, 1);
  const planner = fixture(noRecall);
  await planTurnRecall(planner.api, settings, ctx, { ...request, speaker });
  equal(String(planner.runs[0].prompt).includes(speaker), false);
});

Deno.test("oversized or ambiguous candidate input is rejected before model dispatch", async () => {
  const f = fixture({ ids: [] });
  const candidate = candidates[0];
  for (
    const rows of [
      [candidate, candidate],
      [{ ...candidate, kind: "private-arbitrary-table" }],
      [{ ...candidate, matchedQueries: [-1] }],
      [{ ...candidate, matchedQueries: [1] }],
      [{ ...candidate, matchedQueries: [0, 0] }],
      [{ ...candidate, id: "../ id" }],
      [{ ...candidate, text: "界".repeat(Math.ceil(limits.textBytes / 3)) }],
      [{ ...candidate, source: "x".repeat(limits.sourceBytes + 1) }],
      Array.from(
        { length: limits.candidateCount + 1 },
        (_, n) => ({ ...candidate, id: "m" + n }),
      ),
      Array.from(
        { length: 16 },
        (_, n) => ({
          ...candidate,
          id: "m" + n,
          text: "x".repeat(limits.textBytes),
        }),
      ),
    ]
  ) {
    await rejects(() =>
      selectRecallCandidates(f.api, settings, ctx, {
        ...request,
        candidates: rows,
        queries: [{ text: "blue notebook", includeTimeline: false }],
      })
    );
  }
  equal(f.runs.length, 0);
});

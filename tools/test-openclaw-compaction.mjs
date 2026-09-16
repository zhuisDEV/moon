#!/usr/bin/env node
// Exercise the installed public host SDK with synthetic, in-memory transcripts.
// Usage: node tools/test-openclaw-compaction.mjs /path/to/node_modules/openclaw
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { __moonTest } from "../assets/openclaw-plugin/index.js";

const packageRoot = process.argv[2];
if (!packageRoot) {
  throw new Error("Pass the installed OpenClaw package directory");
}
const manifest = JSON.parse(
  await readFile(resolve(packageRoot, "package.json"), "utf8"),
);
const declaration = manifest.exports?.["./plugin-sdk/agent-core"];
const sdkPath = typeof declaration === "string"
  ? declaration
  : declaration?.default;
assert.equal(
  typeof sdkPath,
  "string",
  "Host must export its public agent-core SDK",
);
const { prepareCompaction, compact, buildSessionContext } = await import(
  pathToFileURL(resolve(packageRoot, sdkPath)).href
);

const oldPath = "/fixture/old-command.ts";
const recentPath = "/fixture/recent-result.ts";
const previousSummary =
  `Earlier accepted decision: preserve ${oldPath} and ticket OLD-17.`;
const messages = [];
for (let turn = 0; turn < 8; turn++) {
  messages.push(
    {
      role: "user",
      content: `Request ${turn}: ${oldPath} ${"x".repeat(500)}`,
      timestamp: turn,
    },
    {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "STORED_THINKING_SENTINEL ".repeat(50) },
        {
          type: "toolCall",
          id: `call-${turn}`,
          name: "read",
          arguments: { path: recentPath, ticket: `TASK-${turn}` },
        },
      ],
      usage: { input: 1, output: 1, totalTokens: 2 },
      providerBookkeeping: "PROVIDER_METADATA_SENTINEL",
      timestamp: turn,
    },
    {
      role: "toolResult",
      toolCallId: `call-${turn}`,
      toolName: "read",
      content: [{
        type: "text",
        text: `Result ${turn}: ${recentPath} ${"r".repeat(300)}`,
      }],
      isError: false,
      timestamp: turn,
    },
    {
      role: "assistant",
      content: [{ type: "text", text: `Answer ${turn}: TASK-${turn}` }],
      timestamp: turn,
    },
  );
}
const entries = messages.map((message, index) => ({
  type: "message",
  id: `entry-${index}`,
  parentId: index ? `entry-${index - 1}` : null,
  timestamp: new Date(index).toISOString(),
  message,
}));
// A previous native compaction boundary retains this active window.
entries.push({
  type: "compaction",
  id: "prior-boundary",
  parentId: entries.at(-1).id,
  timestamp: new Date(100).toISOString(),
  summary: previousSummary,
  firstKeptEntryId: entries[0].id,
  tokensBefore: 10000,
});
const original = JSON.stringify(entries);
const prepared = prepareCompaction(entries, {
  enabled: true,
  reserveTokens: 1024,
  keepRecentTokens: 500,
});
assert.equal(prepared.ok, true);
assert.ok(prepared.value);
assert.equal(prepared.value.previousSummary, previousSummary);
const cut = entries.findIndex((entry) =>
  entry.id === prepared.value.firstKeptEntryId
);
assert.ok(
  cut > 0 && cut < messages.length,
  "Host must compact only the oldest prefix",
);
const retained = entries.slice(cut).filter((entry) => entry.type === "message")
  .map((entry) => entry.message);
assert.notEqual(retained[0].role, "toolResult");
const retainedCalls = new Set(
  retained.flatMap((message) =>
    message.role === "assistant"
      ? message.content.filter((block) => block.type === "toolCall").map((
        block,
      ) => block.id)
      : []
  ),
);
for (const message of retained) {
  if (message.role === "toolResult") {
    assert.ok(retainedCalls.has(message.toolCallId));
  }
}

let modelCalls = 0;
const api = {
  config: {},
  resolvePath: (path) => path,
  pluginConfig: {
    compactionModel: "fixture/local",
    compactionReasoning: "off",
    compactionMaxTokens: 2048,
  },
  runtime: {
    agent: {
      runEmbeddedAgent: (request) => {
        modelCalls++;
        assert.equal(request.thinkLevel, "off");
        assert.deepEqual(request.streamParams, { maxTokens: 2048 });
        assert.equal(request.sessionPersistence, "detached");
        assert.equal(request.disableTools, true);
        assert.ok(!request.prompt.includes("STORED_THINKING_SENTINEL"));
        assert.ok(!request.prompt.includes("PROVIDER_METADATA_SENTINEL"));
        assert.ok(request.prompt.includes(previousSummary));
        assert.ok(request.prompt.includes(oldPath));
        assert.ok(request.prompt.includes(recentPath));
        assert.ok(request.prompt.includes("call-0"));
        return {
          payloads: [{
            text: `Summary: ${oldPath}; ${recentPath}; OLD-17; TASK-0.`,
          }],
        };
      },
    },
  },
};
const providerParams = {
  messages: [
    ...prepared.value.messagesToSummarize,
    ...prepared.value.turnPrefixMessages,
  ],
  previousSummary: prepared.value.previousSummary,
  customInstructions: "Preserve exact paths and task identifiers.",
};
const summary = await __moonTest.summarizeCompaction(api, providerParams);
assert.equal(modelCalls, 1);
assert.equal(JSON.stringify(entries), original);

// Run the real native compaction algorithm, substituting only model completion.
const model = {
  id: "fixture",
  provider: "fixture",
  api: "openai-completions",
  maxTokens: 4096,
  reasoning: false,
};
const nativeResult = await compact(
  prepared.value,
  model,
  undefined,
  undefined,
  undefined,
  undefined,
  "off",
  undefined,
  {
    completeSimple: () => ({
      role: "assistant",
      content: [{ type: "text", text: summary }],
      stopReason: "stop",
      usage: {},
    }),
  },
);
assert.equal(nativeResult.ok, true);
assert.equal(
  nativeResult.value.firstKeptEntryId,
  prepared.value.firstKeptEntryId,
);
const after = [...entries, {
  type: "compaction",
  id: "new-boundary",
  parentId: entries.at(-1).id,
  timestamp: new Date(200).toISOString(),
  ...nativeResult.value,
}];
const context = buildSessionContext(after).messages;
for (const sentinel of [oldPath, recentPath, "OLD-17", "TASK-7"]) {
  assert.ok(JSON.stringify(context).includes(sentinel));
}
assert.ok(context.length < messages.length);
assert.deepEqual(context.slice(-retained.length), retained);

const failingApi = {
  ...api,
  runtime: {
    agent: {
      runEmbeddedAgent: () => {
        throw new Error("PRIVATE_FAILURE_SENTINEL");
      },
    },
  },
};
await assert.rejects(
  __moonTest.summarizeCompaction(failingApi, providerParams),
  (error) => error.message === "Moon local compaction model request failed",
);
const failure = await compact(
  prepared.value,
  model,
  undefined,
  undefined,
  undefined,
  undefined,
  "off",
  undefined,
  {
    completeSimple: () => ({
      role: "assistant",
      content: [],
      stopReason: "error",
      errorMessage: "Synthetic failure",
      usage: {},
    }),
  },
);
assert.equal(failure.ok, false);
assert.equal(JSON.stringify(entries), original);
console.log(
  JSON.stringify({
    hostVersion: manifest.version,
    oldestPrefixMessages: cut,
    retainedMessages: retained.length,
    contextMessages: context.length,
    previousSummaryPreserved: true,
    retainedToolPairsIntact: true,
    candidateProviderCalls: modelCalls,
    failureLeavesSourceUnchanged: true,
    externalModelCalls: 0,
    transcriptWrites: 0,
  }),
);

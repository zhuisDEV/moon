import { __moonTest } from "./index.js";

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

Deno.test("compaction preserves native shell results and excludes display-only history", () => {
  const execution = {
    role: "bashExecution",
    command: "deno task check",
    output: "error E_017",
    exitCode: 1,
    cancelled: false,
    truncated: true,
    fullOutputPath: "/tmp/check-output.txt",
  };
  equal(
    __moonTest.compactionMessages([
      { ...execution, timestamp: 123 },
      { ...execution, excludeFromContext: true },
      { role: "custom", customType: "task-state", content: "Still unfinished" },
    ]),
    [execution, {
      role: "custom",
      content: "Still unfinished",
      customType: "task-state",
    }],
  );
});

Deno.test("compaction preserves conversation and complete tool exchange without reasoning or bookkeeping", () => {
  const messages = [
    {
      role: "user",
      content: "Keep /tmp/exact-file and error E_017",
      timestamp: 123,
    },
    {
      role: "assistant",
      usage: { input: 90000 },
      cost: 2,
      content: [
        {
          type: "thinking",
          thinking: "private reasoning",
          thinkingSignature: "signature",
        },
        { type: "text", text: "Checking now", signature: "signature" },
        {
          type: "toolCall",
          id: "call_1",
          name: "exec",
          arguments: {
            command: "echo 'exact'",
            metadata: "meaningful argument",
          },
          partialJson: "duplicate",
        },
      ],
    },
    {
      role: "toolResult",
      toolCallId: "call_1",
      toolName: "exec",
      isError: true,
      content: [{ type: "text", text: "E_017: complete error\nnext line" }],
      details: { redundant: true },
    },
  ];
  const original = JSON.stringify(messages);
  equal(__moonTest.compactionMessages(messages), [
    { role: "user", content: "Keep /tmp/exact-file and error E_017" },
    {
      role: "assistant",
      content: [
        { type: "text", text: "Checking now" },
        {
          type: "toolCall",
          id: "call_1",
          name: "exec",
          arguments: {
            command: "echo 'exact'",
            metadata: "meaningful argument",
          },
        },
      ],
    },
    {
      role: "toolResult",
      toolCallId: "call_1",
      toolName: "exec",
      isError: true,
      content: [{ type: "text", text: "E_017: complete error\nnext line" }],
    },
  ]);
  equal(JSON.stringify(messages), original);
});

Deno.test("compaction handles attachments, structured results and host summaries", () => {
  equal(
    __moonTest.compactionMessages([
      {
        role: "user",
        content: [{ type: "image", data: "AAAA", mimeType: "image/png" }],
      },
      {
        role: "toolResult",
        content: {
          data: "complete textual result",
          nested: { text: "evidence", base64: "AAAA" },
          usage: {},
        },
      },
      {
        role: "compactionSummary",
        summary: "Unfinished task: preserve identifier 123",
      },
      { role: "user", content: "data:image/png;base64,AAAA" },
    ]),
    [
      {
        role: "user",
        content: [{ type: "text", text: "[image attachment omitted]" }],
      },
      {
        role: "toolResult",
        content: {
          data: "complete textual result",
          nested: { text: "evidence", base64: "AAAA" },
          usage: {},
        },
      },
      {
        role: "compactionSummary",
        summary: "Unfinished task: preserve identifier 123",
      },
      { role: "user", content: "[Binary attachment omitted]" },
    ],
  );
});

Deno.test("compaction retains business metadata inside structured tool results", () => {
  const payload = {
    type: "thinking",
    bytes: 128,
    cost: 12,
    usage: { units: 3 },
    timestamp: "2026-09-16",
    metadata: {
      customer: "customer_1",
      reasoning: "user supplied explanation",
    },
  };
  equal(
    __moonTest.compactionMessages([{
      role: "toolResult",
      toolCallId: "call_2",
      usage: { input: 123 },
      timestamp: 123,
      content: { payload },
    }, {
      role: "user",
      content: [{
        type: "tool_result",
        tool_use_id: "call_3",
        is_error: true,
        content: [{ type: "text", text: "data: exact error; no truncation" }],
      }],
    }]),
    [{
      role: "toolResult",
      toolCallId: "call_2",
      content: { payload },
    }, {
      role: "user",
      content: [{
        type: "tool_result",
        tool_use_id: "call_3",
        is_error: true,
        content: [{ type: "text", text: "data: exact error; no truncation" }],
      }],
    }],
  );
});

Deno.test("compaction prompt retains prior summary and host instructions with semantic input", () => {
  const prompt = __moonTest.compactionPrompt({
    previousSummary: "Earlier decisions",
    customInstructions: "Keep unfinished tasks",
    summarizationInstructions: {
      identifierPolicy: "custom",
      identifierInstructions: "Keep exact IDs",
    },
    messages: [{
      role: "assistant",
      content: [{ type: "thinking", thinking: "OMIT_ME" }, {
        type: "text",
        text: "KEEP_ME",
      }],
      usage: { input: 123 },
    }],
  });
  for (
    const text of [
      "Earlier decisions",
      "Keep unfinished tasks",
      "Keep exact IDs",
      "KEEP_ME",
    ]
  ) {
    if (!prompt.includes(text)) throw new Error(`Missing ${text}`);
  }
  if (prompt.includes("OMIT_ME") || prompt.includes('"usage"')) {
    throw new Error("Leaked bookkeeping");
  }
});

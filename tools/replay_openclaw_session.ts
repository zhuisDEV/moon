import { __moonTest } from "../assets/openclaw-plugin/index.js";

type Message = {
  role?: string;
  timestamp?: number;
  content?: unknown;
};

type Turn = {
  user: Message;
  assistant: Message;
};

function option(name: string, fallback?: string): string {
  const index = Deno.args.indexOf(name);
  const value = index >= 0 ? Deno.args[index + 1] : fallback;
  if (!value) {
    throw new Error(`missing ${name}`);
  }
  return value;
}

function visibleText(content: unknown): string {
  if (typeof content === "string") {
    return content.trim();
  }
  if (!Array.isArray(content)) {
    return "";
  }
  return content
    .filter((part) =>
      part && typeof part === "object" &&
      (part as { type?: string }).type === "text" &&
      typeof (part as { text?: unknown }).text === "string"
    )
    .map((part) => (part as { text: string }).text.trim())
    .filter(Boolean)
    .join("\n");
}

function completedTurns(raw: string): Turn[] {
  const messages = raw
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line))
    .filter((item) => item?.type === "message")
    .map((item) => item.message as Message);
  const turns: Turn[] = [];
  let user: Message | null = null;
  let assistant: Message | null = null;
  for (const message of messages) {
    if (message.role === "user") {
      if (user && assistant) {
        turns.push({ user, assistant });
      }
      user = message;
      assistant = null;
      continue;
    }
    if (
      user && message.role === "assistant" && visibleText(message.content)
    ) {
      assistant = message;
    }
  }
  if (user && assistant) {
    turns.push({ user, assistant });
  }
  return turns;
}

async function runCommand(
  argv: string[],
  options: { timeoutMs: number; input?: string },
) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), options.timeoutMs);
  try {
    const command = new Deno.Command(argv[0], {
      args: argv.slice(1),
      stdin: options.input === undefined ? "null" : "piped",
      stdout: "piped",
      stderr: "piped",
      signal: controller.signal,
    }).spawn();
    if (options.input !== undefined) {
      const writer = command.stdin.getWriter();
      await writer.write(new TextEncoder().encode(options.input));
      await writer.close();
    }
    const output = await command.output();
    return {
      code: output.code,
      stdout: new TextDecoder().decode(output.stdout),
      stderr: new TextDecoder().decode(output.stderr),
    };
  } finally {
    clearTimeout(timer);
  }
}

const binary = option("--binary");
const home = option("--home");
const sessionFile = option("--session-file");
const sessionId = option("--session-id", "isolated-replay");
const fromTurn = Number(option("--from-turn", "1"));
const toTurn = Number(option("--to-turn", String(Number.MAX_SAFE_INTEGER)));
const turns = completedTurns(await Deno.readTextFile(sessionFile));
const selected = turns.slice(fromTurn - 1, toTurn);
const events: string[] = [];
const api = {
  pluginConfig: {
    moonPath: binary,
    moonHome: home,
    mode: "lexical",
    maxChars: 3_500,
    learningEnabled: true,
    learningModel: "gpt-5.6-luna",
    learningReasoning: "medium",
  },
  resolvePath(value: string) {
    return value;
  },
  runtime: {
    system: { runCommandWithTimeout: runCommand },
    agent: {},
  },
  logger: {
    info(message: string) {
      events.push(message);
    },
    error(message: string) {
      events.push(message.replace(/[\r\n].*/s, ""));
    },
  },
};
const engine = __moonTest.createMoonContextEngine(api);
for (let offset = 0; offset < selected.length; offset += 1) {
  const turn = selected[offset];
  await engine.afterTurn({
    sessionId,
    sessionKey: `isolated:replay:${sessionId}`,
    sessionFile,
    messages: [turn.user, turn.assistant],
    prePromptMessageCount: 0,
  });
}
console.log(JSON.stringify({
  availableTurns: turns.length,
  replayedTurns: selected.length,
  fromTurn,
  toTurn: Math.min(toTurn, turns.length),
  events,
}));

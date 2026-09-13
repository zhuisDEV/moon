/**
 * Native Codex account/model discovery; --smoke additionally uses subscription
 * inference for two ephemeral turns. Never reads auth files or prints RPC bodies.
 *
 * deno run --no-config --no-remote --node-modules-dir=none \
 *   --allow-env=PATH,HOME,CODEX_HOME --allow-read="/Applications,$HOME/.codex" \
 *   --allow-run=/Applications/ChatGPT.app/Contents/Resources/codex \
 *   tools/probe-codex-route.ts --codex /Applications/ChatGPT.app/Contents/Resources/codex
 * For the optional test, add /tmp,/private/tmp to --allow-read and --allow-write,
 * and pass --smoke. Use --no-prompt for unattended runs.
 * Read permission is needed only to resolve the executable/home and remove the
 * probe's own temporary directory. No provider keys or auth files are opened.
 * Protocol: https://learn.chatgpt.com/docs/app-server
 */

type JsonObject = Record<string, unknown>;
type Listener = (message: JsonObject) => void;
const ASTRA = "gpt-6-astra";
const EFFORTS = ["low", "xhigh"] as const;
const MAX_STDOUT = 8 * 1024 * 1024;
const MAX_STDERR = 1024 * 1024;
const MAX_LINE = 2 * 1024 * 1024;
const RPC_TIMEOUT = 15_000;
const REVIEWED_SMOKE_VERSION = "0.154.0-alpha.6.2";

export interface RpcProcess {
  stdin: WritableStream<Uint8Array>;
  stdout: ReadableStream<Uint8Array>;
  stderr: ReadableStream<Uint8Array>;
  status: Promise<{ success: boolean; code: number }>;
  kill(signal: Deno.Signal): void;
}

export class ProbeError extends Error {
  turnAttempts?: number;
  constructor(
    readonly code: string,
    readonly method?: string,
    readonly rpcCode?: number,
  ) {
    super(code);
  }
}

function object(value: unknown): JsonObject {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new ProbeError("invalid_protocol_shape");
  }
  return value as JsonObject;
}

function string(value: unknown): string {
  if (typeof value !== "string" || value.length === 0 || value.length > 512) {
    throw new ProbeError("invalid_protocol_shape");
  }
  return value;
}

export function safeFailure(error: unknown): JsonObject {
  // Never serialize foreign errors: native errors may contain identities or keys.
  return error instanceof ProbeError
    ? {
      error: error.code,
      ...(error.method ? { method: error.method } : {}),
      ...(error.rpcCode !== undefined ? { rpc_code: error.rpcCode } : {}),
      ...(error.turnAttempts !== undefined
        ? { turn_attempts: error.turnAttempts }
        : {}),
    }
    : { error: "probe_failed" };
}

export function parseArgs(args: string[]) {
  const options = {
    codex: "codex",
    model: ASTRA,
    smoke: false,
    timeoutMs: 60_000,
  };
  const seen = new Set<string>();
  for (let i = 0; i < args.length; i++) {
    const [name, inline] = args[i].split(/=(.*)/s, 2);
    if (seen.has(name)) throw new ProbeError("duplicate_option");
    seen.add(name);
    if (name === "--smoke" && inline === undefined) {
      options.smoke = true;
      continue;
    }
    if (!["--codex", "--model", "--timeout-ms"].includes(name)) {
      throw new ProbeError("invalid_option");
    }
    const value = inline ?? args[++i];
    if (!value || value.startsWith("--")) {
      throw new ProbeError("missing_option_value");
    }
    if (name === "--codex") options.codex = value;
    if (name === "--model") options.model = value;
    if (name === "--timeout-ms") options.timeoutMs = Number(value);
  }
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$/.test(options.model)) {
    throw new ProbeError("invalid_model");
  }
  if (
    !Number.isSafeInteger(options.timeoutMs) || options.timeoutMs < 1000 ||
    options.timeoutMs > 120_000
  ) {
    throw new ProbeError("invalid_timeout");
  }
  return options;
}

/** Byte limits apply before decoding, including malformed/unterminated lines. */
export class JsonLines {
  private chunks: Uint8Array[] = [];
  private lineBytes = 0;
  private totalBytes = 0;

  constructor(
    private readonly maxTotal = MAX_STDOUT,
    private readonly maxLine = MAX_LINE,
  ) {}

  push(chunk: Uint8Array): JsonObject[] {
    this.totalBytes += chunk.length;
    if (this.totalBytes > this.maxTotal) throw new ProbeError("stdout_limit");
    const messages: JsonObject[] = [];
    let start = 0;
    for (let i = 0; i <= chunk.length; i++) {
      if (i !== chunk.length && chunk[i] !== 10) continue;
      const part = chunk.subarray(start, i);
      this.lineBytes += part.length;
      if (this.lineBytes > this.maxLine) throw new ProbeError("line_limit");
      this.chunks.push(part);
      start = i + 1;
      if (i === chunk.length) break;
      const bytes = new Uint8Array(this.lineBytes);
      let offset = 0;
      for (const piece of this.chunks) {
        bytes.set(piece, offset);
        offset += piece.length;
      }
      this.chunks = [];
      this.lineBytes = 0;
      if (bytes.length === 0) continue;
      try {
        messages.push(
          object(
            JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)),
          ),
        );
      } catch {
        throw new ProbeError("invalid_json_rpc");
      }
    }
    return messages;
  }
}

export class RpcClient {
  turnAttempts = 0;
  private nextId = 1;
  private pending = new Map<
    number,
    {
      method: string;
      resolve: (value: JsonObject) => void;
      reject: (error: ProbeError) => void;
    }
  >();
  private listeners = new Set<Listener>();
  private failure?: ProbeError;
  private closing = false;
  private writer: WritableStreamDefaultWriter<Uint8Array>;
  private stdout: ReadableStreamDefaultReader<Uint8Array>;
  private stderr: ReadableStreamDefaultReader<Uint8Array>;
  private readTasks: Promise<void>[];

  constructor(private readonly child: RpcProcess) {
    this.writer = child.stdin.getWriter();
    this.stdout = child.stdout.getReader();
    this.stderr = child.stderr.getReader();
    this.readTasks = [this.readOutput(), this.drainErrors()];
    void child.status.then(() => {
      if (!this.closing) this.fail(new ProbeError("app_server_exited"));
    });
  }

  private fail(error: ProbeError) {
    if (this.failure || this.closing) return;
    this.failure = error;
    for (const task of this.pending.values()) task.reject(error);
    this.pending.clear();
    for (const listener of this.listeners) {
      listener({ method: "probe/failure" });
    }
    try {
      this.child.kill("SIGTERM");
    } catch { /* Already exited. */ }
  }

  private async readOutput() {
    const lines = new JsonLines();
    try {
      while (true) {
        const { value: chunk, done } = await this.stdout.read();
        if (done) break;
        for (const message of lines.push(chunk)) {
          if (typeof message.method === "string") {
            // A probe has no tools and cannot authorise any server request.
            if (message.id !== undefined) {
              throw new ProbeError("unexpected_server_request");
            }
            for (const listener of this.listeners) listener(message);
          } else if (typeof message.id === "number") {
            const task = this.pending.get(message.id);
            if (!task) continue; // Late reply to an already timed-out request.
            this.pending.delete(message.id);
            if (message.error !== undefined) {
              const remote = object(message.error);
              const code = typeof remote.code === "number" &&
                  Number.isSafeInteger(remote.code)
                ? remote.code
                : undefined;
              task.reject(new ProbeError("rpc_rejected", task.method, code));
            } else task.resolve(object(message.result));
          } else throw new ProbeError("invalid_json_rpc");
        }
      }
    } catch (error) {
      this.fail(
        error instanceof ProbeError ? error : new ProbeError("stdout_failed"),
      );
    }
  }

  private async drainErrors() {
    let bytes = 0;
    try {
      while (true) {
        const { value: chunk, done } = await this.stderr.read();
        if (done) break;
        bytes += chunk.length;
        if (bytes > MAX_STDERR) throw new ProbeError("stderr_limit");
      }
    } catch (error) {
      this.fail(
        error instanceof ProbeError ? error : new ProbeError("stderr_failed"),
      );
    }
  }

  listen(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  async notify(method: string, params: JsonObject = {}) {
    if (this.failure) throw this.failure;
    let timer: number | undefined;
    try {
      await Promise.race([
        this.writer.write(
          new TextEncoder().encode(JSON.stringify({ method, params }) + "\n"),
        ),
        new Promise<never>((_, reject) => {
          timer = setTimeout(
            () => reject(new ProbeError("stdin_timeout")),
            RPC_TIMEOUT,
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }

  async request(
    method: string,
    params: JsonObject = {},
    timeoutMs = RPC_TIMEOUT,
  ): Promise<JsonObject> {
    if (this.failure) throw this.failure;
    const id = this.nextId++;
    let timer: number | undefined;
    const reply = new Promise<JsonObject>((resolve, reject) => {
      timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new ProbeError("rpc_timeout"));
      }, timeoutMs);
      this.pending.set(id, { method, resolve, reject });
    });
    try {
      if (method === "turn/start") this.turnAttempts++;
      const write = this.writer.write(
        new TextEncoder().encode(JSON.stringify({ id, method, params }) + "\n"),
      );
      // The deadline also bounds a blocked stdin write, not just the response.
      return await Promise.race([reply, write.then(() => reply)]);
    } catch (error) {
      // Observe reply even if a broken stdin wins the race with process exit.
      void reply.catch(() => {});
      throw error;
    } finally {
      clearTimeout(timer);
      this.pending.delete(id);
    }
  }

  async close() {
    this.closing = true;
    for (const task of this.pending.values()) {
      task.reject(new ProbeError("probe_closed"));
    }
    this.pending.clear();
    try {
      this.child.kill("SIGTERM");
    } catch { /* Already exited. */ }
    const timer = setTimeout(() => {
      try {
        this.child.kill("SIGKILL");
      } catch { /* Exited. */ }
    }, 1000);
    try {
      await this.child.status;
      await Promise.all([this.stdout.cancel(), this.stderr.cancel()]);
      await Promise.all(this.readTasks);
    } finally {
      clearTimeout(timer);
      try {
        await this.writer.abort();
      } catch { /* Closed stdin. */ }
    }
  }
}

export function accountMode(result: JsonObject): string {
  if (result.account === null) return "signed_out";
  const mode = object(result.account).type;
  return ["chatgpt", "apikey", "chatgptAuthTokens"].includes(String(mode))
    ? String(mode)
    : "unknown";
}

export function advertisedModel(result: JsonObject, model: string) {
  if (!Array.isArray(result.data)) throw new ProbeError("invalid_model_list");
  for (const value of result.data) {
    const row = object(value);
    if (row.model !== model && row.id !== model) continue;
    if (!Array.isArray(row.supportedReasoningEfforts)) {
      throw new ProbeError("invalid_model_list");
    }
    const reasoning = row.supportedReasoningEfforts.map((value) =>
      string(object(value).reasoningEffort)
    );
    if (
      reasoning.some((value) =>
        !["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]
          .includes(value)
      )
    ) {
      throw new ProbeError("invalid_model_list");
    }
    return {
      id: model,
      advertised: true,
      supported_reasoning: [...new Set(reasoning)],
    };
  }
  return null;
}

// The installed native protocol has individual feature controls, not one global
// tools:none flag. Keep this deny list explicit and fail before inference when
// managed policy prevents isolation. Unknown/new native surfaces need re-review.
const DISABLED_FEATURES = [
  "apps",
  "artifact",
  "browser_use",
  "browser_use_external",
  "browser_use_full_cdp_access",
  "chronicle",
  "code_mode",
  "code_mode_only",
  "computer_use",
  "context_management",
  "current_time_reminder",
  "default_mode_request_user_input",
  "deferred_executor",
  "goals",
  "hooks",
  "image_generation",
  "memories",
  "multi_agent",
  "multi_agent_v2",
  "plugins",
  "request_permissions_tool",
  "skill_search",
  "shell_tool",
  "standalone_web_search",
  "token_budget",
  "unified_exec",
  "view_image",
  "web_search_cached",
  "web_search_request",
  "workspace_dependencies",
  "connectors",
  "imagegenext",
  "collab",
  "memory_tool",
  "telepathy",
  "codex_hooks",
];

export function smokeConfig(serverNames: string[]) {
  return {
    ...Object.fromEntries(
      DISABLED_FEATURES.map((name) => [`features.${name}`, false]),
    ),
    "agents.enabled": false,
    "orchestrator.mcp.enabled": false,
    "orchestrator.skills.enabled": false,
    "skills.bundled.enabled": false,
    "skills.include_instructions": false,
    "tools.update_plan.enabled": false,
    "tools.experimental_request_user_input.enabled": false,
    include_environment_context: false,
    project_doc_max_bytes: 0,
    developer_instructions: "",
    model_reasoning_summary: "none",
    history: { persistence: "none" },
    notify: [],
    hooks: Object.fromEntries(
      [
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "PreCompact",
        "PostCompact",
        "SessionStart",
        "UserPromptSubmit",
        "SubagentStart",
        "SubagentStop",
        "Stop",
      ].map((name) => [name, []]),
    ),
    web_search: "disabled",
    mcp_servers: Object.fromEntries(
      serverNames.map((name) => [name, { enabled: false }]),
    ),
  };
}

export function smokeThreadParams(
  cwd: string,
  model: string,
  serverNames: string[],
) {
  return {
    model,
    modelProvider: "openai",
    allowProviderModelFallback: false,
    cwd,
    runtimeWorkspaceRoots: [],
    environments: [],
    selectedCapabilityRoots: [],
    approvalPolicy: "never",
    approvalsReviewer: "user",
    sandbox: "read-only",
    baseInstructions:
      "Return only the requested JSON constant. Do not use tools.",
    developerInstructions: "",
    dynamicTools: [],
    ephemeral: true,
    config: smokeConfig(serverNames),
  };
}

export function assertEphemeralStart(
  result: JsonObject,
  model: string,
  cwd: string,
  allowedGlobalInstructionSource?: string,
): string {
  const thread = object(result.thread);
  if (thread.ephemeral !== true || thread.path != null) {
    throw new ProbeError("thread_not_ephemeral");
  }
  if (result.model !== model || result.modelProvider !== "openai") {
    throw new ProbeError("model_route_changed");
  }
  if (result.cwd !== cwd || result.approvalPolicy !== "never") {
    throw new ProbeError("thread_policy_changed");
  }
  const sandbox = object(result.sandbox);
  if (sandbox.type !== "readOnly" || sandbox.networkAccess === true) {
    throw new ProbeError("thread_policy_changed");
  }
  if (
    !Array.isArray(result.instructionSources) ||
    result.instructionSources.length > 1 ||
    result.instructionSources.some((source) =>
      typeof source !== "string" || source !== allowedGlobalInstructionSource
    )
  ) throw new ProbeError("personal_instructions_loaded", "thread/start");
  return string(thread.id);
}

export async function verifyNativeGlobalInstructions(
  nativeHome: string,
  stat: (path: string) => Promise<Pick<Deno.FileInfo, "isFile" | "isSymlink">> =
    Deno.lstat,
): Promise<string | undefined> {
  const path = `${nativeHome}/AGENTS.md`;
  try {
    const info = await stat(path);
    if (!info.isFile || info.isSymlink) {
      throw new ProbeError("native_global_instructions_not_regular");
    }
    return path;
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) return undefined;
    throw error;
  }
}

export function safeTokenUsage(value: unknown): JsonObject | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return;
  const last = (value as JsonObject).last;
  if (!last || typeof last !== "object" || Array.isArray(last)) return;
  const keys = [
    "totalTokens",
    "inputTokens",
    "cachedInputTokens",
    "outputTokens",
    "reasoningOutputTokens",
  ];
  const fields = keys.flatMap((key) => {
    const count = (last as JsonObject)[key];
    return typeof count === "number" && Number.isSafeInteger(count) &&
        count >= 0
      ? [[key, count]]
      : [];
  });
  return fields.length ? Object.fromEntries(fields) : undefined;
}

export function assertNoMcpTools(result: JsonObject) {
  if (!Array.isArray(result.data) || result.nextCursor != null) {
    throw new ProbeError("mcp_attestation_failed");
  }
  for (const value of result.data) {
    const row = object(value);
    if (
      row.serverInfo !== null || Object.keys(object(row.tools)).length !== 0
    ) throw new ProbeError("mcp_attestation_failed");
  }
}

export function assertSmokeOutput(items: unknown): void {
  if (!Array.isArray(items)) throw new ProbeError("invalid_turn_items");
  const output: string[] = [];
  for (const value of items) {
    const item = object(value);
    if (item.type === "agentMessage") output.push(string(item.text));
    else if (item.type !== "reasoning" && item.type !== "userMessage") {
      throw new ProbeError("unexpected_turn_item");
    }
  }
  if (output.length !== 1) throw new ProbeError("unexpected_smoke_output");
  try {
    const parsed = object(JSON.parse(output[0]));
    if (Object.keys(parsed).length !== 1 || parsed.ok !== "MOON_CODEX_OK") {
      throw new Error();
    }
  } catch {
    throw new ProbeError("unexpected_smoke_output");
  }
}

export async function smokeTurn(
  client: RpcClient,
  cwd: string,
  model: string,
  effort: string,
  serverNames: string[],
  timeoutMs: number,
  allowedGlobalInstructionSource?: string,
) {
  const deadline = Date.now() + timeoutMs;
  const remaining = () => {
    const left = deadline - Date.now();
    if (left <= 0) throw new ProbeError("smoke_timeout");
    return left;
  };
  let threadId: string | undefined;
  let turnId: string | undefined;
  let completed = false;
  let tokenUsage: JsonObject | undefined;
  const completedItems = new Map<string, JsonObject>();
  let unsubscribe = () => {};
  try {
    const started = await client.request(
      "thread/start",
      smokeThreadParams(cwd, model, serverNames),
      remaining(),
    );
    threadId = assertEphemeralStart(
      started,
      model,
      cwd,
      allowedGlobalInstructionSource,
    );
    const nativeGlobalInstructionsLoaded =
      (started.instructionSources as unknown[]).length === 1;
    assertNoMcpTools(
      await client.request("mcpServerStatus/list", {
        threadId,
        detail: "toolsAndAuthOnly",
      }, remaining()),
    );
    let finish: (turn: JsonObject) => void = () => {};
    let fail: (error: ProbeError) => void = () => {};
    const completion = new Promise<JsonObject>((resolve, reject) => {
      finish = resolve;
      fail = reject;
    });
    // Observe immediately: completion may arrive before the turn/start response.
    void completion.catch(() => {});
    unsubscribe = client.listen((message) => {
      if (message.method === "probe/failure") {
        fail(new ProbeError("app_server_failed"));
        return;
      }
      const params = object(message.params ?? {});
      if (params.threadId !== threadId) return;
      if (message.method === "turn/started") {
        turnId = string(object(params.turn).id);
      }
      if (message.method === "turn/completed") finish(object(params.turn));
      if (message.method === "thread/tokenUsage/updated") {
        tokenUsage = safeTokenUsage(params.tokenUsage);
      }
      if (message.method === "model/rerouted") {
        fail(new ProbeError("model_route_changed"));
      }
      if (
        message.method === "item/started" || message.method === "item/completed"
      ) {
        const item = object(params.item);
        const type = item.type;
        if (
          !["agentMessage", "reasoning", "userMessage"].includes(String(type))
        ) fail(new ProbeError("unexpected_turn_item"));
        if (message.method === "item/completed") {
          completedItems.set(string(item.id), item);
        }
      }
      if (message.method === "error") fail(new ProbeError("native_turn_error"));
    });
    const turnStartedAt = Date.now();
    const result = await client.request("turn/start", {
      threadId,
      model,
      effort,
      cwd,
      runtimeWorkspaceRoots: [],
      environments: [],
      approvalPolicy: "never",
      sandboxPolicy: { type: "readOnly", networkAccess: false },
      summary: "none",
      serviceTierForTurn: "default",
      input: [{
        type: "text",
        text: 'Return exactly {"ok":"MOON_CODEX_OK"}.',
        text_elements: [],
      }],
      outputSchema: {
        type: "object",
        properties: { ok: { type: "string", enum: ["MOON_CODEX_OK"] } },
        required: ["ok"],
        additionalProperties: false,
      },
    }, remaining());
    turnId = string(object(result.turn).id);
    const timer = setTimeout(
      () => fail(new ProbeError("smoke_timeout")),
      remaining(),
    );
    let turn: JsonObject;
    try {
      turn = await completion;
    } finally {
      clearTimeout(timer);
    }
    if (
      turn.id !== turnId || turn.status !== "completed" || turn.error != null
    ) throw new ProbeError("smoke_turn_failed");
    // Native completion notifications can leave items empty; collect the
    // independently delivered item/completed messages before checking output.
    if (!Array.isArray(turn.items)) throw new ProbeError("invalid_turn_items");
    for (const value of turn.items) {
      const item = object(value);
      completedItems.set(string(item.id), item);
    }
    assertSmokeOutput([...completedItems.values()]);
    completed = true;
    return {
      effort,
      model: started.model,
      passed: true,
      ephemeral: true,
      native_global_instructions_loaded: nativeGlobalInstructionsLoaded,
      response: { ok: "MOON_CODEX_OK" },
      latency_ms: Date.now() - turnStartedAt,
      ...(tokenUsage ? { token_usage: tokenUsage } : {}),
    };
  } finally {
    unsubscribe();
    if (threadId && turnId && !completed) {
      try {
        await client.request("turn/interrupt", { threadId, turnId }, 1000);
      } catch { /* Best effort, then stop our server. */ }
    }
    if (threadId) {
      try {
        await client.request("thread/unsubscribe", { threadId }, 1000);
      } catch { /* Ephemeral thread dies with our server. */ }
    }
  }
}

async function resolveExecutable(command: string): Promise<string> {
  const candidates = command.includes("/")
    ? [command]
    : (Deno.env.get("PATH") ?? "").split(":").filter(Boolean).map((dir) =>
      `${dir}/${command}`
    );
  for (const candidate of candidates) {
    try {
      const path = await Deno.realPath(candidate);
      if ((await Deno.stat(path)).isFile) return path;
    } catch { /* Try the next explicitly allowed PATH entry. */ }
  }
  throw new ProbeError("codex_executable_unavailable");
}

async function binaryVersion(binary: string): Promise<string> {
  const child = new Deno.Command(binary, {
    args: ["--version"],
    stdout: "piped",
    stderr: "null",
  }).spawn();
  const timer = setTimeout(() => {
    try {
      child.kill("SIGKILL");
    } catch { /* Exited. */ }
  }, 5000);
  try {
    let version = "";
    for await (const chunk of child.stdout) {
      if (version.length + chunk.length > 4096) {
        throw new ProbeError("invalid_codex_version");
      }
      version += new TextDecoder().decode(chunk);
    }
    const match = version.trim().match(
      /^codex-cli ([0-9][a-zA-Z0-9.+-]{0,80})$/,
    );
    if (!(await child.status).success || !match) {
      throw new ProbeError("invalid_codex_version");
    }
    return match[1];
  } finally {
    clearTimeout(timer);
    try {
      child.kill("SIGKILL");
    } catch { /* Exited. */ }
    await child.status;
  }
}

export async function runProbe(args: string[]) {
  const options = parseArgs(args);
  const binary = await resolveExecutable(options.codex);
  const version = await binaryVersion(binary);
  if (options.smoke && version !== REVIEWED_SMOKE_VERSION) {
    throw new ProbeError("smoke_native_version_requires_review");
  }
  const userHome = Deno.env.get("HOME");
  const home = Deno.env.get("CODEX_HOME")?.trim() ||
    (userHome ? `${userHome}/.codex` : null);
  if (!home) throw new ProbeError("native_home_unavailable");
  const nativeHome = await Deno.realPath(home);
  let cwd = "/tmp";
  let temporaryDirectory: string | undefined;
  if (options.smoke) {
    temporaryDirectory = await Deno.makeTempDir({
      dir: "/tmp",
      prefix: "moon-codex-probe-",
    });
    try {
      cwd = await Deno.realPath(temporaryDirectory);
    } catch (error) {
      await Deno.remove(temporaryDirectory);
      throw error;
    }
  }
  let client: RpcClient | undefined;
  let operationError: ProbeError | undefined;
  let reportOutput: JsonObject | undefined;
  try {
    // Inherit native authentication unchanged. Never set HOME/CODEX_HOME or log
    // child environment, account identity, token fields, or stderr.
    client = new RpcClient(
      new Deno.Command(binary, {
        args: ["app-server"],
        cwd,
        stdin: "piped",
        stdout: "piped",
        stderr: "piped",
      }).spawn(),
    );
    await client.request("initialize", {
      clientInfo: {
        name: "moon_codex_route_probe",
        title: "Moon route probe",
        version: "1.0.0",
      },
      capabilities: { experimentalApi: true },
    });
    await client.notify("initialized");
    const authMode = accountMode(
      await client.request("account/read", { refreshToken: false }),
    );
    let cursor: string | undefined;
    let selected = null;
    const cursors = new Set<string>();
    for (let page = 0; page < 10; page++) {
      const result = await client.request("model/list", {
        limit: 100,
        includeHidden: true,
        ...(cursor ? { cursor } : {}),
      });
      selected ??= advertisedModel(result, options.model);
      if (result.nextCursor == null) {
        cursor = undefined;
        break;
      }
      cursor = string(result.nextCursor);
      if (cursors.has(cursor)) throw new ProbeError("model_pagination_loop");
      cursors.add(cursor);
    }
    if (cursor) throw new ProbeError("model_pagination_limit");
    const report: JsonObject = {
      binary,
      version,
      native_home: nativeHome,
      auth_mode: authMode,
      model: selected ??
        { id: options.model, advertised: false, supported_reasoning: [] },
    };
    if (options.smoke) {
      if (authMode !== "chatgpt") {
        throw new ProbeError("smoke_requires_native_chatgpt_auth");
      }
      if (
        !selected ||
        EFFORTS.some((effort) => !selected.supported_reasoning.includes(effort))
      ) throw new ProbeError("smoke_reasoning_unavailable");
      // Unknown managed policy cannot be overridden by this diagnostic.
      const requirements = await client.request("configRequirements/read");
      if (requirements.requirements !== null) {
        throw new ProbeError("smoke_managed_policy_requires_review");
      }
      const config = await client.request("config/read", {
        cwd,
        includeLayers: true,
      });
      if (!Array.isArray(config.layers)) {
        throw new ProbeError("smoke_config_layers_unavailable");
      }
      const allowedLayers = [
        "packagedDefaults",
        "user",
        "system",
        "project",
        "sessionFlags",
      ];
      for (const layer of config.layers) {
        if (!allowedLayers.includes(string(object(object(layer).name).type))) {
          throw new ProbeError("smoke_config_layer_requires_review");
        }
      }
      const effectiveConfig = object(config.config);
      // Null is not a TOML override value. Do not inherit a personal instruction
      // file: reject that setting rather than trying to clear it with JSON null.
      if (effectiveConfig.model_instructions_file != null) {
        throw new ProbeError("smoke_model_instructions_require_review");
      }
      const inherited = effectiveConfig.mcp_servers;
      const serverNames = inherited == null
        ? []
        : Object.keys(object(inherited));
      const outcomes = [];
      for (const effort of EFFORTS) {
        outcomes.push(
          await smokeTurn(
            client,
            cwd,
            options.model,
            effort,
            serverNames,
            options.timeoutMs,
            await verifyNativeGlobalInstructions(nativeHome),
          ),
        );
      }
      report.smoke = outcomes;
      report.turn_attempts = client.turnAttempts;
    }
    reportOutput = report;
  } catch (error) {
    const safe = error instanceof ProbeError
      ? error
      : new ProbeError("probe_failed");
    if (options.smoke) safe.turnAttempts = client?.turnAttempts ?? 0;
    operationError = safe;
  } finally {
    let cleanupFailed = false;
    try {
      await client?.close();
    } catch {
      cleanupFailed = true;
    }
    // Keep the caller-authorised /tmp spelling for cleanup on macOS; the
    // canonical /private/tmp spelling may be outside Deno's granted paths.
    if (temporaryDirectory) {
      try {
        await Deno.remove(temporaryDirectory, { recursive: true });
      } catch {
        cleanupFailed = true;
      }
    }
    // Preserve the first failure and attempted-turn count if cleanup also fails;
    // otherwise a caller might retry an already dispatched model turn.
    if (cleanupFailed && !operationError) {
      const failure = new ProbeError("cleanup_failed");
      if (options.smoke) failure.turnAttempts = client?.turnAttempts ?? 0;
      operationError = failure;
    }
  }
  if (operationError) throw operationError;
  if (!reportOutput) throw new ProbeError("probe_failed");
  return reportOutput;
}

if (import.meta.main) {
  try {
    console.log(JSON.stringify(await runProbe(Deno.args)));
  } catch (error) {
    console.log(JSON.stringify(safeFailure(error)));
    Deno.exitCode = 1;
  }
}

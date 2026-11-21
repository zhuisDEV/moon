/** Operator-only synthetic source-learning validation. See --help. */
// deno-lint-ignore-file no-explicit-any
import {
  adapterDiagnostic,
  canaryChat,
  canaryChatResponse,
  CanaryError,
  check,
  cleanEnvironment,
  ENV_ALLOWLIST,
  guardCommand,
  hookHostConfig,
  inspectPacket,
  MAX_CHARS,
  MODEL,
  QUERY,
  safeFailure,
  SCOPE,
  SOURCES,
  stopOwnedChild,
  TOPIC,
} from "./source-learning-canary-contract.ts";

const HELP = `Moon source-learning canary (default: zero-inference preflight)

Initial command:
  --home ABS/target/moon-canary-NAME --binary ABS --plugin ABS
  --host ABS --host-state ABS --host-config ABS --agent-dir ABS
  --codex-plugin ABS --node ABS [--stage preflight]

Resume with --home and one stage:
  --stage prepare                  create synthetic SQLite sources
  --stage l1 --smoke                at most two real L1 calls; stop for review
  --stage review                   save source/topic proposals for manual review
  --stage accept-l1 --approve HASH,HASH
  --stage l2 --smoke                at most one real L2 call; stop for review
  --stage accept-l2 --approve HASH
  --stage verify --model-cache ABS  lexical/hybrid, exact hook, withdrawal checks

Never automatically retries a reserved inference. Maximum three reservations
per home, including uncertain/failed calls. Review means manually checking
all statements, negations, qualifications and citation mappings before passing
the exact result hashes to accept. It is not supplied by an automated check.
Artifacts remain in the private canary home; archive before removing it.
One supervisor holds operator.lock throughout each stage. A crash leaves that
lock in place: inspect its owner, owned-host.json and all reservations before
manually removing it. The tool never steals a lock or retries uncertain calls.
--timeout-ms N bounds each host stage (default 360000, maximum 600000).

Deno supervisor permissions (replace paths with canonical absolute paths):
  deno run --no-config --no-remote --node-modules-dir=none --no-prompt \\
    --allow-env=${ENV_ALLOWLIST.join(",")} \\
    --allow-read=REPO/target,REPO/tools,BINARY,PLUGIN,HOST,HOST_STATE,NODE,MODEL_CACHE \\
    --allow-write=REPO/target/moon-canary-NAME \\
    --allow-run=NODE,BINARY tools/source-learning-canary.ts ...

For verify only, the installed adapter's Node-compatible stdio spawn enumerates
its environment. Launch Deno via env -i with only PATH, a canary HOME and LANG,
then use --allow-env; retain the same scoped filesystem/run permissions. This
stage needs no host authentication and performs no model calls.

The existing OpenClaw Node host runs with its normal host permissions: Deno's
permissions do not sandbox this child. It uses the existing host auth owner;
no credentials are copied, printed or saved in reports. No services/channels
start. Preflight performs no inference: it reads the account catalogue and
warms the exact execution owner. The existing host may refresh its catalogue.
OpenClaw 2026.11.3 and 2026.11.5 are currently reviewed; other versions fail
closed until their contracts are reviewed. Account discovery uses the public
SDK. Exact-owner preparation requires one version-gated private named export;
missing/ambiguous capabilities fail closed. No hardcoded hashed imports.
`;

export function parseArgs(args: string[]) {
  const options: Record<string, string | boolean> = {
    stage: "preflight",
    timeoutMs: "360000",
  };
  const names: Record<string, string> = {
    "--home": "home",
    "--binary": "binary",
    "--plugin": "plugin",
    "--host": "host",
    "--host-state": "hostState",
    "--host-config": "hostConfig",
    "--agent-dir": "agentDir",
    "--codex-plugin": "codexPlugin",
    "--node": "node",
    "--stage": "stage",
    "--approve": "approve",
    "--model-cache": "modelCache",
    "--timeout-ms": "timeoutMs",
  };
  const seen = new Set<string>();
  for (let i = 0; i < args.length; i++) {
    const key = args[i];
    check(!seen.has(key), "duplicate_option");
    seen.add(key);
    if (key === "--smoke" || key === "--help") {
      options[key.slice(2)] = true;
      continue;
    }
    check(
      names[key] && args[i + 1] && !args[i + 1].startsWith("--"),
      "invalid_option",
    );
    options[names[key]] = args[++i];
  }
  check(
    [
      "preflight",
      "prepare",
      "l1",
      "review",
      "accept-l1",
      "l2",
      "accept-l2",
      "verify",
    ].includes(String(options.stage)),
    "invalid_stage",
  );
  check(
    !["l1", "l2"].includes(String(options.stage)) || options.smoke,
    "smoke_flag_required",
  );
  check(
    !options.smoke || ["l1", "l2"].includes(String(options.stage)),
    "smoke_flag_requires_model_stage",
  );
  const timeout = Number(options.timeoutMs);
  check(
    Number.isSafeInteger(timeout) && timeout >= 1000 && timeout <= 600000,
    "invalid_timeout",
  );
  return options;
}

async function writeJson(file: string, value: unknown) {
  const temporary = await Deno.open(file + ".next", {
    create: true,
    truncate: true,
    write: true,
    mode: 0o600,
  });
  try {
    const bytes = new TextEncoder().encode(
      JSON.stringify(value, null, 2) + "\n",
    );
    let offset = 0;
    while (offset < bytes.length) {
      offset += await temporary.write(bytes.subarray(offset));
    }
    await temporary.sync();
  } finally {
    temporary.close();
  }
  await Deno.rename(file + ".next", file);
  const directory = await Deno.open(file.slice(0, file.lastIndexOf("/")), {
    read: true,
  });
  try {
    await directory.sync();
  } finally {
    directory.close();
  }
}
async function sha256(file: string) {
  return Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", await Deno.readFile(file)),
    ),
  )
    .map((b) => b.toString(16).padStart(2, "0")).join("");
}
async function exists(file: string) {
  try {
    await Deno.lstat(file);
    return true;
  } catch (error) {
    if (error instanceof Deno.errors.NotFound) return false;
    throw error;
  }
}
async function validateHome(options: Record<string, string | boolean>) {
  const home = String(options.home ?? "");
  check(
    home.startsWith("/") && /\/moon-canary-[A-Za-z0-9_-]+$/.test(home),
    "explicit_temporary_home_required",
  );
  const parent = home.slice(0, home.lastIndexOf("/"));
  check(
    await Deno.realPath(parent) === parent,
    "home_parent_must_be_canonical",
  );
  check(
    parent === await Deno.realPath(Deno.cwd() + "/target"),
    "home_must_be_in_workspace_target",
  );
  if (await exists(home)) {
    check(
      !(await Deno.lstat(home)).isSymlink && await Deno.realPath(home) === home,
      "home_symlink_refused",
    );
  }
  return home;
}

async function acquireHome(options: Record<string, string | boolean>) {
  const home = await validateHome(options);
  const fresh = !await exists(home);
  if (fresh) {
    check(options.stage === "preflight", "new_home_requires_preflight");
    await Deno.mkdir(home, { mode: 0o700 });
  } else {check(
      await exists(home + "/canary-state.json"),
      "foreign_canary_home",
    );}
  const file = home + "/operator.lock";
  let lock: Deno.FsFile;
  try {
    lock = await Deno.open(file, { createNew: true, write: true, mode: 0o600 });
  } catch (error) {
    if (error instanceof Deno.errors.AlreadyExists) {
      throw new CanaryError(
        "canary_locked_verify_previous_owner_before_removing_lock",
      );
    }
    throw error;
  }
  const identity = await lock.stat();
  const token = crypto.randomUUID();
  try {
    const bytes = new TextEncoder().encode(
      JSON.stringify({ supervisorPid: Deno.pid, token }) + "\n",
    );
    let offset = 0;
    while (offset < bytes.length) {
      offset += await lock.write(bytes.subarray(offset));
    }
    await lock.sync();
  } catch (error) {
    lock.close();
    throw error;
  }
  return {
    home,
    fresh,
    async release() {
      lock.close();
      const current = await Deno.lstat(file);
      check(
        !current.isSymlink && current.ino === identity.ino &&
          current.dev === identity.dev,
        "operator_lock_ownership_changed",
      );
      check(
        JSON.parse(await Deno.readTextFile(file)).token === token,
        "operator_lock_ownership_changed",
      );
      await Deno.remove(file);
    },
  };
}

async function loadState(
  options: Record<string, string | boolean>,
  home: string,
  fresh: boolean,
) {
  const file = home + "/canary-state.json";
  if (!fresh) {
    const state = JSON.parse(await Deno.readTextFile(file));
    check(
      state.schema_version === 1 && state.home === home &&
        state.scope === SCOPE,
      "foreign_canary_home",
    );
    check(
      state.binaryHash === await sha256(state.paths.binary) &&
        state.pluginHash === await sha256(state.paths.plugin + "/index.js"),
      "candidate_changed_start_new_home",
    );
    for (const key of Object.keys(state.paths)) {
      if (options[key]) {
        check(
          await Deno.realPath(String(options[key])) === state.paths[key],
          "resume_path_mismatch",
        );
      }
    }
    return state;
  }
  check(options.stage === "preflight", "new_home_requires_preflight");
  const paths: Record<string, string> = {};
  for (
    const key of [
      "binary",
      "plugin",
      "host",
      "hostState",
      "hostConfig",
      "agentDir",
      "codexPlugin",
      "node",
    ]
  ) {
    const value = String(options[key] ?? "");
    check(value.startsWith("/"), "initial_paths_required");
    try {
      paths[key] = await Deno.realPath(value);
    } catch {
      throw new CanaryError(`initial_path_unavailable_${key}`);
    }
  }
  check(
    !home.startsWith(paths.hostState + "/") &&
      !paths.hostState.startsWith(home + "/"),
    "host_state_isolation_failed",
  );
  await Deno.mkdir(home + "/workspace", { mode: 0o700 });
  const state = {
    schema_version: 1,
    home,
    scope: SCOPE,
    paths,
    stage: "new",
    calls: [],
    reviews: [],
    binaryHash: await sha256(paths.binary),
    pluginHash: await sha256(paths.plugin + "/index.js"),
    servicesStarted: false,
    gatewayChatDeliveryVerified: false,
  };
  await writeJson(file, state);
  return state;
}

async function readBounded(
  stream: ReadableStream<Uint8Array>,
  limit: number,
  onLimit: () => void,
  signal?: AbortSignal,
) {
  let size = 0;
  const chunks: Uint8Array[] = [];
  const reader = stream.getReader();
  const cancel = () => {
    void reader.cancel().catch(() => {});
  };
  signal?.addEventListener("abort", cancel, { once: true });
  try {
    while (true) {
      const { done, value: chunk } = await reader.read();
      if (done) break;
      size += chunk.length;
      if (size > limit) {
        onLimit();
        await reader.cancel();
        throw new CanaryError("host_output_limit");
      }
      chunks.push(chunk);
    }
  } finally {
    signal?.removeEventListener("abort", cancel);
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return new TextDecoder().decode(bytes);
}

/** Test seam is a process interface, never a user-selectable fake host flag. */
export async function supervise(
  child: {
    stdout: ReadableStream<Uint8Array>;
    stderr: ReadableStream<Uint8Array>;
    status: Promise<{ success: boolean; code: number }>;
    kill(signal: "SIGTERM" | "SIGKILL"): void;
  },
  timeoutMs: number,
) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const controller = new AbortController();
  const abort = () => {
    try {
      child.kill("SIGTERM");
    } catch { /* exited */ }
  };
  const outputs = Promise.allSettled([
    readBounded(child.stdout, 1024 * 1024, abort, controller.signal),
    readBounded(child.stderr, 1024 * 1024, abort, controller.signal),
  ]);
  const completed = await Promise.race([
    Promise.all([child.status, outputs]),
    new Promise<null>((resolve) => {
      timer = setTimeout(() => resolve(null), timeoutMs);
    }),
  ]).finally(() => clearTimeout(timer));
  if (!completed) {
    controller.abort();
    await stopOwnedChild(child);
    throw new CanaryError("host_stage_timeout");
  }
  const [status, output] = completed;
  check(
    output[0].status === "fulfilled" && output[1].status === "fulfilled",
    "host_output_limit",
  );
  const stdout = output[0].value;
  const records = stdout.split("\n").flatMap((line) => {
    try {
      const value = JSON.parse(line);
      return value.protocol === "moon-source-canary-v1" ? [value] : [];
    } catch {
      return [];
    }
  });
  check(records.length === 1, "host_protocol_missing_or_duplicated");
  const value = records[0];
  if (value.status === "failed") {
    if (value.error === "adapter_failure") {
      throw new CanaryError(
        "adapter_failure",
        adapterDiagnostic(value.adapter_failure),
      );
    }
    const known = [
      "unsupported_host_version_review_required",
      "host_sdk_export_missing",
      "host_snapshot_capability_missing",
      "host_loader_capability_missing",
      "host_catalogue_capability_missing",
      "configured_model_missing_from_catalogue",
      "unsupported_host_auth_owner_route",
      "authored_provider_requires_review",
      "credential_material_in_selected_config",
      "host_plugin_registration_failed",
      "registered_model_runner_missing",
      "adapter_tool_capability_missing",
      "observed_model_identity_missing",
      "reported_request_mismatch",
      "observed_model_mismatch",
      "observed_backend_unknown",
      "observed_fallback_attempt",
      "host_inference_failed",
      "inference_replay_refused",
      "inference_budget_exhausted",
      "wrong_review_checkpoint",
      "model_did_not_leave_review",
      "operation_failed",
      "execution_owner_capability_missing",
      "execution_owner_capability_ambiguous",
      "execution_owner_contract_changed",
      "execution_owner_identity_mismatch",
      "execution_owner_model_missing",
    ];
    throw new CanaryError(
      known.includes(value.error) ? value.error : "host_bridge_failed",
    );
  }
  check(
    status.success && value.status === "passed" &&
      value.nativeAuthUnchanged === true &&
      value.cleanup?.remainingOwnedChildren === 0,
    "host_or_auth_preservation_failed",
  );
  // Pick fixed fields instead of forwarding potentially compromised host output.
  return {
    status: "passed",
    modelCalls: Number.isSafeInteger(value.modelCalls)
      ? value.modelCalls
      : null,
    nativeAuthUnchanged: true,
    runtime: "captured_registered_api",
    catalogue: value.catalogue === "exact_owner_full_paired_auth_publication"
      ? "exact_owner_full_paired_auth_publication"
      : "read_only",
    cleanup: {
      servicesSettled: value.cleanup.servicesSettled === true,
      remainingOwnedChildren: 0,
    },
  };
}

async function hostStage(state: any, stage: string, timeout: number) {
  const env = cleanEnvironment((key) => Deno.env.get(key));
  env.OPENCLAW_STATE_DIR = state.paths.hostState;
  env.OPENCLAW_AGENT_DIR = state.paths.agentDir;
  const script =
    new URL("./source-learning-canary-host.mjs", import.meta.url).pathname;
  const child = new Deno.Command(state.paths.node, {
    args: [script, state.home, stage],
    clearEnv: true,
    env,
    stdin: "null",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  await writeJson(state.home + "/owned-host.json", {
    pid: child.pid,
    stage,
    status: "running",
  });
  try {
    const result = await supervise(child, timeout);
    await writeJson(state.home + `/host-${stage}.json`, result);
    return JSON.parse(
      await Deno.readTextFile(state.home + "/canary-state.json"),
    );
  } finally {
    await stopOwnedChild(child);
    await writeJson(state.home + "/owned-host.json", {
      pid: child.pid,
      stage,
      status: "exited",
    });
  }
}

function moonRunner(state: any) {
  const run = async (argv: string[], options: any = {}) => {
    guardCommand(argv, state.paths.binary, state.home);
    const child = new Deno.Command(argv[0], {
      args: argv.slice(1),
      clearEnv: true,
      env: {
        HOME: state.home,
        PATH: "/opt/homebrew/bin:/usr/bin:/bin",
        LANG: "C.UTF-8",
      },
      stdin: options.input === undefined ? "null" : "piped",
      stdout: "piped",
      stderr: "null",
    }).spawn();
    const timer = setTimeout(() => {
      try {
        child.kill("SIGKILL");
      } catch { /* exited */ }
    }, 180000);
    try {
      if (options.input !== undefined) {
        const writer = child.stdin.getWriter();
        await writer.write(new TextEncoder().encode(options.input));
        await writer.close();
      }
      const stdout = await readBounded(
        child.stdout,
        4 * 1024 * 1024,
        () => child.kill("SIGKILL"),
      );
      const result = await child.status;
      return { code: result.code, stdout, stderr: "" };
    } finally {
      clearTimeout(timer);
      await stopOwnedChild(child);
    }
  };
  const moon = async (args: string[], input?: unknown) => {
    const result = await run([
      state.paths.binary,
      "--home",
      state.home,
      "--database",
      state.home + "/state/moon.sqlite",
      "--dimensions",
      "384",
      "--json",
      ...args,
    ], { input: typeof input === "string" ? input : JSON.stringify(input) });
    check(result.code === 0, "isolated_moon_command_failed");
    return JSON.parse(result.stdout);
  };
  return { moon, run };
}

async function copyPublicCache(from: string, to: string) {
  check(
    from.startsWith("/") && await Deno.realPath(from) === from,
    "canonical_model_cache_required",
  );
  const model = "models--intfloat--multilingual-e5-small";
  const revision = (await Deno.readTextFile(from + `/${model}/refs/main`))
    .trim();
  check(/^[a-f0-9]{40}$/.test(revision), "model_cache_revision_invalid");
  const relativeFiles = [
    `${model}/refs/main`,
    ...[
      "config.json",
      "tokenizer.json",
      "tokenizer_config.json",
      "special_tokens_map.json",
      "onnx/model.onnx",
    ]
      .map((name) => `${model}/snapshots/${revision}/${name}`),
  ];
  for (const relative of relativeFiles) {
    // Hugging Face cache snapshots use symlinks into their own blob directory.
    // Dereference only files proven to remain inside this public cache root.
    const source = await Deno.realPath(from + "/" + relative);
    check(
      source.startsWith(from + "/") && (await Deno.stat(source)).isFile,
      "model_cache_file_escapes_root",
    );
    const destination = to + "/" + relative;
    await Deno.mkdir(destination.slice(0, destination.lastIndexOf("/")), {
      recursive: true,
      mode: 0o700,
    });
    await Deno.copyFile(source, destination);
  }
}

async function verify(state: any, moon: any, run: any, options: any) {
  check(
    state.calls.length === 3 &&
      state.calls.every((call: any) => call.status === "completed"),
    "three_completed_inferences_required",
  );
  check(
    state.stage === "topic_accepted" && state.acceptedSnapshot?.ok,
    "accepted_snapshot_required",
  );
  check(
    typeof options.modelCache === "string",
    "existing_model_cache_required_no_download",
  );
  await copyPublicCache(options.modelCache, state.home + "/models/fastembed");
  // Keep reviewed operator sources private. Exercise the chat hook with one
  // independently published synthetic fact in this isolated database.
  const publicFact =
    "The Aster restart rehearsal uses the public status label Silver Finch.";
  await moon([
    "privacy",
    "publish",
    "--actor",
    "canary:operator",
    "--kind",
    "fact",
  ], publicFact);
  await moon(["embed", "--provider", "local", "--limit", "100"]);
  const contextArgs = (
    mode: string,
    collection: string[] = ["--scope", SCOPE],
    query: string = QUERY,
  ) => [
    "context",
    "--query",
    query,
    ...collection,
    "--mode",
    mode,
    "--provider",
    "local",
    "--max-chars",
    String(MAX_CHARS),
    "--limit",
    "8",
  ];
  state.recalls = [];
  for (const mode of ["lexical", "hybrid"]) {
    const packet = await moon(contextArgs(mode));
    state.recalls.push({ mode, ...inspectPacket(packet, true) });
    await writeJson(state.home + `/packet-${mode}.json`, packet);
  }
  const hooks = new Map<string, any>();
  const services: any[] = [];
  const settings = {
    moonPath: state.paths.binary,
    moonHome: state.home,
    dimensions: 384,
    mode: "hybrid",
    scope: SCOPE,
    learningScope: SCOPE,
    maxChars: MAX_CHARS,
    limit: 8,
    embeddingEnabled: false,
    learningEnabled: false,
    failOpen: false,
  };
  const plugin = await import(
    new URL("file://" + state.paths.plugin + "/index.js").href
  );
  const logger = { info() {}, warn() {}, error() {}, debug() {} };
  plugin.default.register({
    pluginConfig: settings,
    config: hookHostConfig(settings),
    logger,
    resolvePath: (p: string) => p === "." ? state.home + "/workspace" : p,
    runtime: {
      system: { runCommandWithTimeout: run },
      agent: { runEmbeddedAgent: canaryChatResponse },
    },
    on: (name: string, fn: any) => hooks.set(name, fn),
    registerService: (service: any) => services.push(service),
    registerTool() {},
    registerContextEngine() {},
    registerCompactionProvider() {},
  });
  try {
    check(hooks.has("before_prompt_build"), "prompt_hook_missing");
    const recall = (query: string) =>
      hooks.get("before_prompt_build")({
        prompt: query,
        currentUserMessage: query,
        currentUserMessageId: crypto.randomUUID(),
      }, {
        ...canaryChat,
        agentId: "main",
        senderIsOwner: true,
        sessionKey: "agent:main:direct:aster-canary",
        workspaceDir: state.home + "/workspace",
        runId: "canary-recall",
        hookInvocation: { assertActive() {} },
      });
    // The operator query has highly relevant approved sources. The chat has
    // no right to those sources even when its sender is marked as owner.
    const operatorResult = await recall(QUERY);
    const operatorPacket = operatorResult?.prependContext ?? "";
    const excludesSources = (packet: string) =>
      !packet.includes(TOPIC.key) &&
      SOURCES.every((s) =>
        !packet.includes(s.key) &&
        s.content.split("\n").every((line) => !packet.includes(line))
      );
    check(excludesSources(operatorPacket), "operator_source_leaked_to_chat");
    const publicQuery =
      "What is the public status label for the Aster restart rehearsal?";
    const result = await recall(publicQuery);
    const actual = result?.prependContext;
    check(
      typeof actual === "string" && Array.from(actual).length <= MAX_CHARS &&
        actual.includes(publicFact) && excludesSources(actual),
      "hook_packet_mismatch",
    );
    const modelPlanned = typeof plugin.planTurnRecall === "function";
    if (modelPlanned) {
      const expected = await moon([
        "recall-candidates",
        "--audience-json",
        "[]",
        "--mode",
        "hybrid",
        "--provider",
        "local",
        "--limit",
        "8",
        "--max-chars",
        String(MAX_CHARS),
      ], [{ text: publicQuery, temporal: false }]);
      const selected = actual.split("\n").filter((line: string) =>
        line.startsWith("{")
      )
        .map((line: string) => JSON.parse(line));
      check(
        expected.public_only === true && selected.length > 0 &&
          selected.length === expected.candidates.length &&
          selected.every((record: { id: string; text: string }, i: number) =>
            record.id === "m" + expected.candidates[i].document_id &&
            record.text === expected.candidates[i].content
          ),
        "hook_candidate_mismatch",
      );
      await writeJson(state.home + "/hook-comparison.json", {
        actual,
        expected: expected.candidates,
      });
    } else {
      const expected = await moon([
        ...contextArgs("hybrid", ["--audience-json", "[]"], publicQuery),
        "--adapter",
      ]);
      check(actual === expected.packet, "hook_packet_mismatch");
      await writeJson(state.home + "/hook-comparison.json", {
        actual,
        expected: expected.packet,
      });
    }
    state.promptHook = {
      exactIndependentPacketMatch: !modelPlanned,
      exactIndependentRecordMatch: modelPlanned,
      modelDecision: modelPlanned ? "scripted-canary" : "legacy-hook",
      publicFactRecalled: true,
      operatorSourcesExcluded: true,
      gatewayChatDeliveryVerified: false,
      packetChars: Array.from(result.prependContext).length,
    };
    await Deno.writeTextFile(
      state.home + "/prompt-hook-packet.txt",
      result.prependContext,
      { mode: 0o600 },
    );
  } finally {
    await Promise.all(services.map((service) => service.stop?.()));
  }
  const before = await moon([
    "source",
    "state",
    "--scope",
    SCOPE,
    "--key",
    SOURCES[0].key,
  ]);
  await moon([
    "source",
    "withdraw",
    "--scope",
    SCOPE,
    "--key",
    SOURCES[0].key,
    "--expected-revision",
    before.revision_id,
    "--expected-generation",
    String(before.generation),
    "--reason",
    "Synthetic canary lifecycle verification",
  ]);
  const topic = await moon([
    "topic",
    "get",
    "--scope",
    SCOPE,
    "--key",
    TOPIC.key,
  ]);
  check(topic.freshness !== "fresh", "withdrawal_did_not_invalidate_topic");
  const packet = await moon(contextArgs("hybrid"));
  const citations = inspectPacket(packet, false);
  check(
    !packet.references.some((r: any) => r.topic) &&
      !citations.source_keys.includes(SOURCES[0].key) &&
      citations.source_keys.includes(SOURCES[1].key),
    "withdrawal_evidence_retention_failed",
  );
  state.lifecycle = {
    topicInvalidated: true,
    withdrawnSourceExcluded: true,
    independentEvidenceRetained: true,
  };
  state.health = await moon(["health"]);
  check(
    state.health.ok && Number.isInteger(state.expectedSchema) &&
      state.expectedSchema > 0 &&
      state.health.schema_version === state.expectedSchema,
    "canary_health_failed",
  );
  await moon([
    "backup",
    "--destination",
    state.home + "/after-withdrawal.sqlite",
  ]);
  state.stage = "passed";
}

export async function main(args: string[]) {
  const options = parseArgs(args);
  if (options.help) {
    console.log(HELP);
    return;
  }
  const ownership = await acquireHome(options);
  try {
    let state = await loadState(options, ownership.home, ownership.fresh);
    const { moon, run } = moonRunner(state);
    const stage = String(options.stage);
    if (["preflight", "l1", "l2"].includes(stage)) {
      state = await hostStage(state, stage, Number(options.timeoutMs));
      if (stage === "l1") {
        check(
          (await moon(["source", "claims", "list", "--scope", SCOPE])).claims
            .length === 0,
          "unreviewed_claims_visible",
        );
      }
    } else if (stage === "prepare") {
      check(
        state.preflightPassed && state.stage === "new",
        "successful_preflight_required",
      );
      await Deno.writeTextFile(
        state.home + "/moon.toml",
        `[learning.l1]\nenabled = true\nmodel = "openai/${MODEL}"\nreasoning = "medium"\nfallback_enabled = false\n\n[learning.l2]\nenabled = true\nmodel = "openai/${MODEL}"\nreasoning = "max"\nfallback_enabled = false\n`,
        { mode: 0o600 },
      );
      await moon(["init"]);
      const initialHealth = await moon(["health"]);
      check(
        initialHealth.ok && Number.isInteger(initialHealth.schema_version) &&
          initialHealth.schema_version > 0,
        "initial_candidate_health_failed",
      );
      state.expectedSchema = initialHealth.schema_version;
      const cfg = await moon(["config", "show"]);
      for (const level of ["l1", "l2"]) {
        check(
          cfg.learning[level].enabled &&
            cfg.learning[level].model === `openai/${MODEL}` &&
            cfg.learning[level].reasoning ===
              (level === "l1" ? "medium" : "max") &&
            cfg.learning[level].fallback_enabled === false,
          "moon_model_config_mismatch",
        );
      }
      for (const source of SOURCES) {
        await moon(["source", "import", "--scope", SCOPE, "--allow-learning"], {
          schema_version: 1,
          source_key: source.key,
          title: source.title,
          kind: "external_document",
          content: source.content,
        });
      }
      state.stage = "prepared";
    } else if (stage === "review") {
      check(
        ["l1_review", "l2_review"].includes(state.stage),
        "review_checkpoint_required",
      );
      const reviews = state.stage === "l1_review"
        ? state.reviews
        : [state.topicReview];
      const proposals = [];
      for (const review of reviews) {
        proposals.push(
          await moon([
            ...(state.stage === "l1_review"
              ? ["source", "learning", "status"]
              : ["topic", "status"]),
            "--scope",
            SCOPE,
            "--job-id",
            review.job_id,
          ]),
        );
      }
      await writeJson(state.home + "/review-proposals.json", {
        sources: SOURCES,
        proposals,
      });
    } else if (stage === "accept-l1" || stage === "accept-l2") {
      const l1 = stage === "accept-l1";
      check(
        state.stage === (l1 ? "l1_review" : "l2_review"),
        "review_checkpoint_required",
      );
      const reviews = l1 ? state.reviews : [state.topicReview];
      const approved = String(options.approve ?? "").split(",").sort();
      check(
        JSON.stringify(approved) === JSON.stringify(
          reviews.map((r: any) => r.result_hash).sort(),
        ),
        "exact_review_hashes_required",
      );
      state.reviewApprovals ??= {};
      state.reviewApprovals[l1 ? "l1" : "l2"] = {
        hashes: approved,
        manuallyAcknowledged: true,
      };
      if (l1) {
        state.acceptedClaims ??= [];
        for (const review of reviews) {
          const result = await moon([
            "source",
            "learning",
            "accept",
            "--scope",
            SCOPE,
            "--job-id",
            review.job_id,
            "--expected-result-hash",
            review.result_hash,
            "--yes",
          ]);
          state.acceptedClaims.push(...result.claim_ids);
        }
        await moon(["topic", "put", "--scope", SCOPE], {
          schema_version: 1,
          ...TOPIC,
          claim_ids: state.acceptedClaims,
        });
        state.stage = "claims_accepted";
      } else {
        const packet = await moon([
          "context",
          "--scope",
          SCOPE,
          "--query",
          QUERY,
          "--mode",
          "lexical",
          "--max-chars",
          String(MAX_CHARS),
        ]);
        check(
          !packet.references.some((r: any) => r.topic),
          "unreviewed_topic_visible",
        );
        state.unreviewedTopicExcluded = true;
        await moon([
          "topic",
          "accept",
          "--scope",
          SCOPE,
          "--job-id",
          state.topicReview.job_id,
          "--expected-result-hash",
          state.topicReview.result_hash,
          "--yes",
        ]);
        state.acceptedSnapshot = await moon([
          "backup",
          "--destination",
          state.home + "/accepted-before-withdrawal.sqlite",
        ]);
        state.stage = "topic_accepted";
      }
    } else await verify(state, moon, run, options);
    await writeJson(state.home + "/canary-state.json", state);
    if (state.stage === "passed") {
      const hostChecks = await Promise.all(
        ["l1", "l2"].map(async (level) =>
          JSON.parse(
            await Deno.readTextFile(state.home + `/host-${level}.json`),
          )
        ),
      );
      check(
        hostChecks.every((host) =>
          host.nativeAuthUnchanged && host.cleanup.remainingOwnedChildren === 0
        ),
        "successful_stage_cleanup_or_auth_evidence_missing",
      );
      await writeJson(state.home + "/validation-summary.json", {
        status: "passed",
        completedInferences: state.calls,
        reviews: state.reviewApprovals,
        recalls: state.recalls,
        promptHook: state.promptHook,
        lifecycle: state.lifecycle,
        health: state.health,
        acceptedSnapshot: state.acceptedSnapshot,
        unreviewedTopicExcluded: state.unreviewedTopicExcluded,
        nativeAuthUnchanged: true,
        remainingOwnedChildren: 0,
        runtime: "adapter_factories_using_captured_registered_api",
        limitations: {
          gatewayChatDeliveryVerified: false,
          productionScheduleAwaited: false,
          transportReasoningEffortVerified: false,
        },
      });
    }
    console.log(
      JSON.stringify({
        status: "passed",
        stage: state.stage,
        reservedCalls: state.calls.length,
        completedCalls: state.calls.filter((c: any) =>
          c.status === "completed"
        ).length,
        report: state.home + "/canary-state.json",
      }),
    );
  } finally {
    await ownership.release();
  }
}
if (import.meta.main) {
  try {
    await main(Deno.args);
  } catch (error) {
    const failure = error instanceof Deno.errors.NotCapable
      ? {
        error: "permission_denied_check_help",
        required_permission: error.message.match(
          /Requires (env|read|write|run|ffi|sys|net) access/,
        )?.[1] ?? "unknown",
        ...(error.message.match(/^Requires env access to "([A-Z_a-z0-9]+)"/)
            ?.[1]
          ? {
            required_env: error.message.match(
              /^Requires env access to "([A-Z_a-z0-9]+)"/,
            )![1],
          }
          : {}),
      }
      : safeFailure(error);
    console.error(JSON.stringify({ status: "failed", ...failure }));
    Deno.exitCode = 1;
  }
}

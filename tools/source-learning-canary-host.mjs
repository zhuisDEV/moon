/** Existing OpenClaw Node host bridge; not a Moon runtime or dependency.
 * Deno owns the CLI, permission boundary, deadline and reports. OpenClaw needs
 * its supported Node SQLite/fchmod behaviour, unavailable in Deno compatibility.
 */
import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { createHash } from "node:crypto";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import process from "node:process";
import {
  adapterDiagnostic,
  assertExecution,
  assertModelRequest,
  CanaryError,
  check,
  checkAuthoredProvider,
  credentialGuard,
  executionOwnerInput,
  guardCommand,
  MAX_CHARS,
  MODEL,
  PROVIDER,
  reserveCall,
  safeFailure,
  SCOPE,
  selectExecutionOwnerModule,
  SOURCES,
  SUPPORTED_HOSTS,
  TOPIC,
} from "./source-learning-canary-contract.ts";

const PROTOCOL = "moon-source-canary-v1";
for (const key of ["log", "info", "warn", "error", "debug"]) {
  console[key] = () => {};
}

// Track handles created in this standalone process, never search/kill by name.
// This also catches native harness subprocesses spawned by imported host code.
const owned = new Set();
for (const key of ["spawn", "fork", "exec", "execFile"]) {
  const original = childProcess[key];
  childProcess[key] = (...args) => {
    const child = original(...args);
    owned.add(child);
    child.once("close", () => owned.delete(child));
    return child;
  };
}
syncBuiltinESMExports();
let stopping = false;
let active = true;
const services = [];
let phase = "startup";
async function cleanup() {
  active = false;
  const deadline = new Promise((resolve) =>
    setTimeout(() => resolve(false), 2000)
  );
  const disposed = Promise.allSettled(services.map((s) => s.stop?.())).then(
    () => true,
  );
  const servicesSettled = await Promise.race([disposed, deadline]);
  for (const child of owned) child.kill("SIGTERM");
  const until = Date.now() + 1500;
  while (owned.size && Date.now() < until) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  for (const child of owned) child.kill("SIGKILL");
  const killUntil = Date.now() + 500;
  while (owned.size && Date.now() < killUntil) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  return { servicesSettled, remainingOwnedChildren: owned.size };
}
process.once("SIGTERM", async () => {
  if (stopping) return;
  stopping = true;
  await cleanup();
  process.exit(124);
});

async function writeJson(file, value) {
  const temporary = file + ".next";
  const handle = await fs.open(temporary, "w", 0o600);
  try {
    await handle.writeFile(JSON.stringify(value, null, 2) + "\n");
    await handle.sync();
  } finally {
    await handle.close();
  }
  await fs.rename(temporary, file);
  // A reservation must survive a crash after dispatch, including its rename.
  const directory = await fs.open(path.dirname(file), "r");
  try {
    await directory.sync();
  } finally {
    await directory.close();
  }
}
async function authFingerprint() {
  const home = process.env.CODEX_HOME || path.join(process.env.HOME, ".codex");
  try {
    return createHash("sha256").update(
      await fs.readFile(path.join(home, "auth.json")),
    ).digest("hex");
  } catch (error) {
    if (error?.code === "ENOENT") return "absent";
    throw new CanaryError("native_auth_preservation_check_unavailable");
  }
}
async function executeMoon(state, argv, options = {}) {
  guardCommand(argv, state.paths.binary, state.home);
  check(active, "invocation_closed");
  const child = childProcess.spawn(argv[0], argv.slice(1), {
    env: {
      HOME: state.home,
      PATH: "/opt/homebrew/bin:/usr/bin:/bin",
      LANG: "C.UTF-8",
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  const chunks = [];
  let bytes = 0;
  let exceeded = false;
  child.stdout.on("data", (chunk) => {
    bytes += chunk.length;
    if (bytes > 4 * 1024 * 1024) {
      exceeded = true;
      child.kill("SIGKILL");
    } else chunks.push(chunk);
  });
  // Consume, but never print or persist arbitrary Moon/host diagnostics.
  child.stderr.resume();
  const timer = setTimeout(
    () => child.kill("SIGKILL"),
    Math.min(options.timeoutMs ?? 180000, 180000),
  );
  child.stdin.end(options.input);
  try {
    const code = await new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("close", resolve);
    });
    check(!exceeded, "moon_output_limit");
    return { code, stdout: Buffer.concat(chunks).toString("utf8"), stderr: "" };
  } finally {
    clearTimeout(timer);
  }
}

async function registeredRuntime(state, stage, save) {
  phase = "host_setup";
  const { paths, home } = state;
  const pkg = JSON.parse(
    await fs.readFile(path.join(paths.host, "package.json"), "utf8"),
  );
  check(
    pkg.name === "openclaw" && SUPPORTED_HOSTS.includes(pkg.version),
    "unsupported_host_version_review_required",
  );
  const sdk = (name) => {
    const exported = pkg.exports?.[`./plugin-sdk/${name}`]?.default;
    check(
      typeof exported === "string" && exported.startsWith("./dist/plugin-sdk/"),
      "host_sdk_export_missing",
    );
    return pathToFileURL(path.join(paths.host, exported)).href;
  };
  const source = JSON.parse(await fs.readFile(paths.hostConfig, "utf8"));
  const codexConfig = source.plugins?.entries?.codex?.config ?? {};
  const server = codexConfig.appServer ?? {};
  check(
    (server.homeScope ?? "agent") === "agent" &&
      (server.transport ?? "stdio") === "stdio" && !server.env &&
      !server.codexHome,
    "unsupported_host_auth_owner_route",
  );
  checkAuthoredProvider(source.models?.providers?.openai);
  const workspace = path.join(home, "workspace");
  const wrapper = path.join(home, "registered-plugin");
  const modelRef = `${PROVIDER}/${MODEL}`;
  const main = source.agents?.list?.find((a) => a.id === "main") ??
    source.agents?.entries?.main ?? {};
  const settings = {
    moonPath: paths.binary,
    moonHome: home,
    dimensions: 384,
    maxChars: MAX_CHARS,
    limit: 8,
    scope: SCOPE,
    learningScope: SCOPE,
    learningEnabled: true,
    embeddingEnabled: false,
    mode: "hybrid",
    failOpen: false,
    timeoutMs: 30000,
    embeddingTimeoutMs: 180000,
  };
  const config = {
    agents: {
      defaults: {
        ...source.agents?.defaults,
        workspace,
        heartbeat: undefined,
        model: { primary: modelRef, fallbacks: [] },
        models: {
          [modelRef]: source.agents?.defaults?.models?.[modelRef] ?? {},
        },
      },
      list: [{
        id: "main",
        default: true,
        agentDir: paths.agentDir,
        workspace,
        ...(main.models?.[modelRef]
          ? { models: { [modelRef]: main.models[modelRef] } }
          : {}),
      }],
    },
    ...(source.auth ? { auth: source.auth } : {}),
    models: { providers: {} },
    tools: { allow: ["moon_source_learn", "moon_topic_compile"] },
    session: { store: home + "/host-sessions/{agentId}/sessions.json" },
    logging: {
      level: "silent",
      consoleLevel: "silent",
      file: home + "/host-runtime.log",
    },
    plugins: {
      enabled: true,
      allow: ["moon", "codex", "openai"],
      load: { paths: [wrapper, paths.codexPlugin] },
      slots: { contextEngine: "moon", memory: "none" },
      entries: {
        codex: { enabled: true, config: codexConfig },
        openai: { ...source.plugins?.entries?.openai, enabled: true },
        moon: {
          enabled: true,
          config: settings,
          hooks: { allowConversationAccess: true, allowPromptInjection: true },
        },
      },
    },
  };
  credentialGuard(config);
  await fs.mkdir(wrapper, { mode: 0o700, recursive: true });
  await writeJson(home + "/host-probe.json", config);
  process.env.OPENCLAW_CONFIG_PATH = home + "/host-probe.json";
  await writeJson(wrapper + "/package.json", {
    type: "module",
    openclaw: { extensions: ["./index.js"] },
  });
  await fs.copyFile(
    paths.plugin + "/openclaw.plugin.json",
    wrapper + "/openclaw.plugin.json",
  );
  await fs.chmod(wrapper + "/openclaw.plugin.json", 0o600);
  const symbol = "moon.source.canary.registered-api";
  const capture = { api: null, services };
  globalThis[Symbol.for(symbol)] = capture;
  await fs.writeFile(
    wrapper + "/index.js",
    `import moon from ${
      JSON.stringify(pathToFileURL(paths.plugin + "/index.js").href)
    };export default {...moon,register(api){const c=globalThis[Symbol.for(${
      JSON.stringify(symbol)
    })];c.api=api;moon.register({...api,registerService(s){c.services.push(s);api.registerService(s);}});}};\n`,
    { mode: 0o600 },
  );
  const logger = {
    info() {},
    warn() {},
    error() {},
    debug() {},
    child() {
      return this;
    },
  };
  const snapshot = await import(sdk("runtime-config-snapshot"));
  check(
    typeof snapshot.setRuntimeConfigSnapshot === "function",
    "host_snapshot_capability_missing",
  );
  snapshot.setRuntimeConfigSnapshot(config);
  const loader = await import(
    pathToFileURL(paths.host + "/dist/plugins/loader.js").href
  );
  check(
    typeof loader.loadOpenClawPlugins === "function",
    "host_loader_capability_missing",
  );
  const registry = loader.loadOpenClawPlugins({
    config,
    workspaceDir: workspace,
    onlyPluginIds: ["moon", "codex", "openai"],
    activate: true,
    cache: false,
    allowProcessHomeSessionCatalogs: false,
    logger,
  });
  check(
    ["moon", "codex", "openai"].every((id) =>
      registry.plugins.some((p) => p.id === id && p.status === "loaded")
    ) && capture.api,
    "host_plugin_registration_failed",
  );
  const agent = await import(sdk("agent-runtime"));
  check(
    typeof agent.loadPreparedModelCatalog === "function",
    "host_catalogue_capability_missing",
  );
  // Public SDK owns auth preparation, standalone publication, inherited owner
  // resolution and full catalogue/auth pairing. Never import hashed chunks.
  phase = "account_catalogue";
  const rows = await agent.loadPreparedModelCatalog({
    config,
    agentId: "main",
    agentDir: paths.agentDir,
    workspaceDir: workspace,
    readOnly: true,
    providerDiscoveryProviderIds: [PROVIDER],
    scopedLiveProviderDiscovery: true,
  });
  check(
    rows.some((r) => r.provider === PROVIDER && r.id === MODEL),
    "configured_model_missing_from_catalogue",
  );
  // The public catalogue SDK omits runtimePluginSelections from its owner.
  // A model turn includes those selections in its owner key. Warm and retain
  // that exact owner, otherwise catalogue success can mask missing paired auth.
  // This private compatibility surface is admitted only for reviewed host versions.
  phase = "execution_owner";
  const candidates = [];
  for (
    const entry of await fs.readdir(paths.host + "/dist", {
      withFileTypes: true,
    })
  ) {
    if (
      entry.isFile() && /^prepared-model-runtime-[\w-]+\.mjs$/.test(entry.name)
    ) {
      candidates.push({
        name: entry.name,
        content: await fs.readFile(paths.host + "/dist/" + entry.name, "utf8"),
      });
    }
  }
  const ownerModule = selectExecutionOwnerModule(candidates);
  const ownerApi = await import(
    pathToFileURL(paths.host + "/dist/" + ownerModule).href
  );
  check(
    typeof ownerApi.acquireAgentRunPreparedModelRuntime === "function",
    "execution_owner_capability_missing",
  );
  const lease = await ownerApi.acquireAgentRunPreparedModelRuntime(
    executionOwnerInput(config, paths.agentDir, workspace),
    { catalogMode: "static", retainIdleRunOwner: false },
  );
  check(
    typeof lease?.[Symbol.asyncDispose] === "function" &&
      typeof lease.snapshot?.loadFullModelCatalog === "function",
    "execution_owner_contract_changed",
  );
  services.push({ stop: () => lease[Symbol.asyncDispose]() });
  check(
    lease.snapshot.agentDir === paths.agentDir &&
      lease.snapshot.workspaceDir === workspace,
    "execution_owner_identity_mismatch",
  );
  const full = await lease.snapshot.loadFullModelCatalog({
    providerIds: [PROVIDER],
    refresh: true,
    wait: true,
  });
  check(
    [...full.entries, ...full.routeVariants ?? []].some((row) =>
      row.provider === PROVIDER && row.id === MODEL
    ),
    "execution_owner_model_missing",
  );
  state.executionOwnerPreflight = {
    capability: "version_gated_private_named_export",
    selections: [{ provider: PROVIDER, modelId: MODEL, agentId: "main" }],
    fullCatalogueAuthPaired: true,
  };
  const api = capture.api;
  check(
    typeof api.runtime?.agent?.runEmbeddedAgent === "function",
    "registered_model_runner_missing",
  );
  const original = api.runtime.agent.runEmbeddedAgent;
  let pendingKey;
  const instrumented = {
    ...api,
    logger,
    resolvePath: (p) => p === "." ? workspace : api.resolvePath(p),
    runtime: {
      ...api.runtime,
      system: {
        ...api.runtime.system,
        runCommandWithTimeout: (argv, options) =>
          executeMoon(state, argv, options),
      },
      agent: {
        ...api.runtime.agent,
        async runEmbeddedAgent(params) {
          check(
            stage !== "preflight" && pendingKey,
            "inference_not_authorised",
          );
          assertModelRequest(params, stage, workspace);
          const call = reserveCall(state.calls, stage, pendingKey);
          // Crash/timeout after reservation is uncertain, and cannot be replayed.
          await save();
          try {
            const result = await original(params);
            call.status = result?.meta?.error || result?.meta?.aborted ||
                result?.meta?.timeoutPhase || result?.payloads?.some((p) =>
                  p.isError
                )
              ? "failed_or_uncertain"
              : "returned";
            await save();
            return result;
          } catch (error) {
            call.status = "failed_or_uncertain";
            await save();
            // Preserve the original error *in memory* for the adapter's fixed
            // category classifier. Never print or persist its message/body.
            throw error;
          }
        },
      },
    },
  };
  const plugin = await import(pathToFileURL(paths.plugin + "/index.js").href);
  check(
    typeof plugin.__moonTest?.failureDiagnostic === "function" &&
      typeof plugin.__moonTest?.createSourceLearningTool === "function" &&
      typeof plugin.__moonTest?.createTopicCompileTool === "function",
    "adapter_tool_capability_missing",
  );
  const context = {
    senderIsOwner: true,
    agentId: "main",
    sessionKey: "agent:main:direct:aster-canary",
    workspaceDir: workspace,
    assertInvocationCurrent() {
      check(active, "invocation_closed");
    },
    assertInputCommitAllowed() {
      check(active, "invocation_closed");
    },
  };
  return {
    hostVersion: pkg.version,
    async call(key) {
      pendingKey = key;
      const tool = stage === "l1"
        ? plugin.__moonTest.createSourceLearningTool(instrumented, context)
        : plugin.__moonTest.createTopicCompileTool(instrumented, context);
      phase = stage === "l1" ? "l1_tool" : "l2_tool";
      let result;
      try {
        result = await tool.execute(crypto.randomUUID(), {
          scope: SCOPE,
          ...(stage === "l1" ? { source_key: key } : { topic_key: key }),
        });
      } catch (error) {
        const diagnostic = adapterDiagnostic(
          plugin.__moonTest.failureDiagnostic(error),
        );
        const call = state.calls.find((c) => c.key === key);
        if (call && diagnostic) {
          call.failure = diagnostic;
          await save();
        }
        if (diagnostic) throw new CanaryError("adapter_failure", diagnostic);
        throw error;
      }
      check(
        result.details?.status === "review" &&
          result.details.automatic_acceptance === false,
        "model_did_not_leave_review",
      );
      const execution = assertExecution(result.details.model_execution, stage);
      const call = state.calls.find((c) => c.key === key);
      check(call?.status === "returned", "model_call_checkpoint_missing");
      call.status = "completed";
      call.execution = execution;
      await save();
      // Only a reviewed adapter result shape, never arbitrary model payloads.
      return {
        key,
        job_id: result.details.job_id,
        result_hash: result.details.result_hash,
        model_execution: execution,
      };
    },
  };
}

let result;
let before;
let state;
try {
  const [home, stage] = process.argv.slice(2);
  check(["preflight", "l1", "l2"].includes(stage), "invalid_host_stage");
  state = JSON.parse(await fs.readFile(home + "/canary-state.json", "utf8"));
  check(
    state.home === await fs.realpath(home) && state.schema_version === 1,
    "invalid_canary_home",
  );
  const save = () => writeJson(home + "/canary-state.json", state);
  before = await authFingerprint();
  const runtime = await registeredRuntime(state, stage, save);
  if (stage === "l1") {
    check(state.stage === "prepared", "wrong_review_checkpoint");
    for (const source of SOURCES) {
      if (state.reviews.some((r) => r.key === source.key)) continue;
      state.reviews.push(await runtime.call(source.key));
      await save();
    }
    state.stage = "l1_review";
  } else if (stage === "l2") {
    check(state.stage === "claims_accepted", "wrong_review_checkpoint");
    state.topicReview = await runtime.call(TOPIC.key);
    state.stage = "l2_review";
  } else state.preflightPassed = true;
  await save();
  result = {
    status: "passed",
    stage,
    modelCalls: state.calls.length,
    hostVersion: runtime.hostVersion,
    runtime: "captured_registered_api",
    catalogue: "exact_owner_full_paired_auth_publication",
  };
} catch (error) {
  result = { status: "failed", phase, ...safeFailure(error) };
} finally {
  stopping = true;
  const cleanupResult = await cleanup();
  try {
    result.nativeAuthUnchanged = before !== undefined &&
      before === await authFingerprint();
  } catch {
    result.nativeAuthUnchanged = false;
  }
  result.cleanup = cleanupResult;
  if (
    !result.nativeAuthUnchanged || cleanupResult.remainingOwnedChildren !== 0
  ) result.status = "failed";
  if (state?.home) {
    // Discard unexpected host logs rather than retaining foreign diagnostics.
    await fs.rm(state.home + "/host-runtime.log", { force: true }).catch(
      () => {},
    );
    await writeJson(state.home + "/host-last-result.json", result).catch(
      () => {},
    );
  }
  // This is the only stdout record admitted by the Deno supervisor. Host logs
  // and foreign exception bodies are neither emitted nor saved as diagnostics.
  await new Promise((resolve) =>
    process.stdout.write(
      JSON.stringify({ protocol: PROTOCOL, ...result }) + "\n",
      resolve,
    )
  );
  process.exit(result.status === "passed" ? 0 : 1);
}

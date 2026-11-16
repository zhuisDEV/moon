/** Shared, dependency-free contracts for the Deno operator tool and host bridge. */
// deno-lint-ignore-file no-explicit-any
export const SCOPE = "project:aster-source-learning-canary";
export const MODEL = "gpt-6.1-terra";
export const PROVIDER = "openai";
export const MAX_CALLS = 3;
export const MAX_CHARS = 8000;
export const SUPPORTED_HOST = "2026.11.3";
export const TOPIC = {
  key: "aster-readiness",
  title: "Aster restart readiness: operator approval and integrity checks",
};
export const QUERY =
  "Does Aster restart readiness permit restarting after plan review without operator approval or a successful integrity check?";
export const SOURCES = [
  {
    key: "aster-operator",
    title: "Aster restart operator approval",
    content:
      "The Aster rehearsal service may restart only after an operator approves the restart.\nA reviewed plan alone does not authorise an Aster restart.",
  },
  {
    key: "aster-integrity",
    title: "Aster restart integrity readiness",
    content:
      "The Aster rehearsal service is ready only after a local integrity check succeeds.\nA successful integrity check does not replace operator approval.",
  },
];

export class CanaryError extends Error {
  readonly code: string;
  readonly diagnostic?: AdapterDiagnostic;
  constructor(code: string, diagnostic?: AdapterDiagnostic) {
    super(code);
    this.code = code;
    this.diagnostic = diagnostic;
  }
}
export function check(value: unknown, code: string): asserts value {
  if (!value) throw new CanaryError(code);
}
export function safeFailure(error: unknown) {
  // Only our closed, authored codes are safe. Never serialize foreign errors.
  return {
    error: error instanceof CanaryError ? error.code : "operation_failed",
    ...(error instanceof CanaryError && error.diagnostic
      ? { adapter_failure: error.diagnostic }
      : {}),
  };
}

export interface AdapterDiagnostic {
  phase: string;
  code: string;
  dispatch: "not_started" | "unknown" | "completed";
}
const ADAPTER_FAILURES: Record<string, string[]> = {
  internal: ["unknown"],
  config: [
    "invalid_response",
    "command_failed",
    "command_timeout",
    "command_cancelled",
  ],
  prompt: ["unreadable"],
  prepare: [
    "invalid_response",
    "command_failed",
    "command_timeout",
    "command_cancelled",
  ],
  model: [
    "runtime_unavailable",
    "runtime_setup_failed",
    "authentication_failed",
    "model_unavailable",
    "model_route_mismatch",
    "backend_error",
    "timeout",
    "cancelled",
    "empty_response",
    "error_payload",
  ],
  normalise: ["invalid_json", "invalid_batch", "invalid_citation"],
  apply: [
    "invalid_response",
    "not_committed",
    "command_failed",
    "command_timeout",
    "command_cancelled",
  ],
};
/** Input must come from the installed adapter's WeakMap-backed diagnostic API,
 * never directly from foreign error properties or text. */
export function adapterDiagnostic(value: any): AdapterDiagnostic | undefined {
  if (
    typeof value?.phase !== "string" || typeof value?.code !== "string" ||
    !Object.hasOwn(ADAPTER_FAILURES, value.phase) ||
    !ADAPTER_FAILURES[value.phase].includes(value.code)
  ) return undefined;
  return {
    phase: value.phase,
    code: value.code,
    dispatch: ["not_started", "completed"].includes(value.dispatch)
      ? value.dispatch
      : "unknown",
  };
}

/** A reviewed private host capability, selected by its explicit exported name.
 * Reject absent/ambiguous build wrappers; never guess a minifier alias/hash. */
export function selectExecutionOwnerModule(
  files: { name: string; content: string }[],
) {
  const matches = files.filter(({ content }) =>
    [...content.matchAll(/export\s*\{([^}]*)\}/g)].some((match) =>
      match[1].split(",").some((entry) =>
        entry.trim() === "acquireAgentRunPreparedModelRuntime"
      )
    )
  );
  check(
    matches.length === 1,
    matches.length
      ? "execution_owner_capability_ambiguous"
      : "execution_owner_capability_missing",
  );
  return matches[0].name;
}
export function executionOwnerInput(
  config: unknown,
  agentDir: string,
  workspaceDir: string,
) {
  return {
    config,
    agentId: "main",
    agentDir,
    workspaceDir,
    preserveWorkspaceDirOnRefresh: false,
    runtimePluginSelections: [{
      provider: PROVIDER,
      modelId: MODEL,
      agentId: "main",
    }],
  };
}

export function hookHostConfig(settings: Record<string, unknown>) {
  return {
    plugins: {
      slots: { contextEngine: "moon", memory: "none" },
      entries: {
        moon: {
          enabled: true,
          config: settings,
          hooks: { allowConversationAccess: true, allowPromptInjection: true },
        },
      },
    },
  };
}
export function credentialGuard(value: unknown): void {
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    check(
      !/(api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|password|credentials|(^|[_-])(access|refresh|token|secret)($|[_-]))/i
        .test(key),
      "credential_material_in_selected_config",
    );
    credentialGuard(child);
  }
}
export const ENV_ALLOWLIST = [
  "PATH",
  "HOME",
  "TMPDIR",
  "CODEX_HOME",
  "HTTP_PROXY",
  "HTTPS_PROXY",
  "ALL_PROXY",
  "NO_PROXY",
  "http_proxy",
  "https_proxy",
  "all_proxy",
  "no_proxy",
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
  "NODE_EXTRA_CA_CERTS",
];
export function cleanEnvironment(read: (key: string) => string | undefined) {
  return Object.fromEntries(ENV_ALLOWLIST.flatMap((key) => {
    const value = read(key);
    return value === undefined ? [] : [[key, value]];
  }));
}
export function assertModelRequest(p: any, stage: string, workspace: string) {
  check(
    p.provider === PROVIDER && p.model === MODEL &&
      p.thinkLevel === (stage === "l2" ? "max" : "medium"),
    "requested_model_mismatch",
  );
  check(
    p.agentId === "main" && p.sessionPersistence === "detached" &&
      /^agent:main:internal-session-effects:incognito-/.test(p.sessionKey) &&
      p.workspaceDir === workspace,
    "model_session_isolation_failed",
  );
  check(
    p.disableTools === true && p.disableMessageTool === true &&
      Array.isArray(p.toolsAllow) && p.toolsAllow.length === 0 &&
      Array.isArray(p.modelFallbacksOverride) &&
      p.modelFallbacksOverride.length === 0 && p.streamParams === undefined,
    "model_tool_or_transport_policy_failed",
  );
}
export function assertExecution(value: any, stage: string) {
  check(value?.identity === "verified", "observed_model_identity_missing");
  check(
    value.requested?.provider === PROVIDER &&
      value.requested?.model === MODEL &&
      value.requested?.reasoning === (stage === "l2" ? "max" : "medium"),
    "reported_request_mismatch",
  );
  check(
    value.observed?.provider === PROVIDER &&
      [MODEL, `${PROVIDER}/${MODEL}`].includes(value.observed?.model),
    "observed_model_mismatch",
  );
  check(
    value.observed?.fallback_attempts === null ||
      value.observed?.fallback_attempts === 0,
    "observed_fallback_attempt",
  );
  // The supported Responses/token-sharing route reports "openclaw". Native
  // Codex is also valid; neither backend proves a transport reasoning effort.
  check(
    ["openclaw", "codex"].includes(value.observed?.backend),
    "observed_backend_unknown",
  );
  return value;
}
export function reserveCall(calls: any[], stage: string, key: string) {
  check(calls.length < MAX_CALLS, "inference_budget_exhausted");
  check(!calls.some((call) => call.key === key), "inference_replay_refused");
  const call = { stage, key, status: "reserved" };
  calls.push(call);
  return call;
}
export function guardCommand(argv: string[], binary: string, home: string) {
  check(argv[0] === binary, "unexpected_executable");
  for (
    const [flag, value] of [
      ["--home", home],
      ["--database", `${home}/state/moon.sqlite`],
    ]
  ) {
    check(
      !argv.some((arg) => arg.startsWith(flag + "=")) &&
        argv.filter((a) => a === flag).length === 1 &&
        argv[argv.indexOf(flag) + 1] === value,
      "moon_storage_not_isolated",
    );
  }
  const scopeIndex = argv.indexOf("--scope");
  check(!argv.some((arg) => arg.startsWith("--scope=")), "unexpected_scope");
  if (scopeIndex >= 0) {
    check(
      argv.filter((arg) => arg === "--scope").length === 1 &&
        argv[scopeIndex + 1] === SCOPE,
      "unexpected_scope",
    );
  }
  for (const flag of ["--key", "--source-key", "--topic-key"]) {
    const index = argv.indexOf(flag);
    if (index >= 0) {
      check(
        [...SOURCES.map((s) => s.key), TOPIC.key].includes(argv[index + 1]),
        "unexpected_source_or_topic",
      );
    }
  }
}
export function inspectPacket(packet: any, requireTopic: boolean) {
  check(
    Number.isInteger(packet.used_chars) && packet.used_chars <= MAX_CHARS &&
      packet.memories?.length === 0,
    "packet_budget_or_personal_memory_failed",
  );
  let count = 0;
  const keys = new Set<string>();
  for (const ref of packet.references) {
    check(ref.scope === SCOPE, "packet_scope_mismatch");
    const citations = ref.source_claim?.citations ??
      ref.topic?.statements.flatMap((s: any) => s.citations) ?? [];
    for (const citation of citations) {
      const source = SOURCES.find((s) => s.key === citation.source_key);
      check(source, "unknown_citation_source");
      check(
        Number.isInteger(citation.start_byte) &&
          Number.isInteger(citation.end_byte) && citation.start_byte >= 0 &&
          citation.end_byte > citation.start_byte &&
          new TextDecoder("utf-8", { fatal: true }).decode(
              new TextEncoder().encode(source.content).slice(
                citation.start_byte,
                citation.end_byte,
              ),
            ) === citation.quote,
        "citation_bytes_mismatch",
      );
      keys.add(citation.source_key);
      count++;
    }
  }
  check(count > 0, "no_grounded_citations");
  if (requireTopic) {
    const topics = packet.references.filter((r: any) => r.topic);
    check(topics.length === 1, "expected_one_topic");
    const topic = topics[0].topic;
    check(
      topic.key === TOPIC.key && topic.statements.length > 0 &&
        topic.statements.every((s: any) => s.citations.length > 0),
      "topic_statements_ungrounded",
    );
    const topicKeys = new Set(
      topic.statements.flatMap((s: any) =>
        s.citations.map((c: any) => c.source_key)
      ),
    );
    check(SOURCES.every((s) => topicKeys.has(s.key)), "topic_missing_premise");
  }
  return {
    citations: count,
    used_chars: packet.used_chars,
    source_keys: [...keys],
  };
}

/** Bounded shutdown: exclusively the child object created by this invocation. */
export async function stopOwnedChild(
  child: {
    status: Promise<unknown>;
    kill(signal: "SIGTERM" | "SIGKILL"): void;
  },
  graceMs = 2000,
) {
  // Do not signal a numeric PID after its child status is already reaped.
  const alreadyExited = await Promise.race([
    child.status.then(() => true, () => true),
    new Promise<boolean>((resolve) => queueMicrotask(() => resolve(false))),
  ]);
  if (alreadyExited) return "already_exited";
  const exited = async () => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        child.status.then(() => true, () => true),
        new Promise<boolean>((resolve) => {
          timer = setTimeout(() => resolve(false), graceMs);
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  };
  try {
    child.kill("SIGTERM");
  } catch { /* already exited */ }
  if (await exited()) return "terminated";
  try {
    child.kill("SIGKILL");
  } catch { /* already exited */ }
  check(await exited(), "owned_child_cleanup_failed");
  return "killed";
}

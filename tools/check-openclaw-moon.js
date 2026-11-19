// Read-only probe for a Moon 2.8 OpenClaw installation. No model call or delivery.
// deno run --no-prompt --allow-run tools/check-openclaw-moon.js
// Optional: --since 2026-11-19T01:10:00Z limits L1 observations to that period.
// If using a custom L2 prompt, also allow read access to that file.
// Exit 0: checks passed (idle stages can lack proof). 1: failed check.
// Exit 2: inspection incomplete. Exit 3: eligible scheduled work still outstanding.
// Empty occurrences do not leave a durable scheduler receipt.
import { __moonTest as adapter } from "../assets/openclaw-plugin/index.js";
import manifest from "../assets/openclaw-plugin/package.json" with {
  type: "json",
};
import { isAbsolute, join } from "node:path";

class ProbeError extends Error {}

async function json(command, args, label) {
  try {
    const result = await new Deno.Command(command, {
      args,
      stdin: "null",
      stderr: "null",
      signal: AbortSignal.timeout(30_000),
    }).output();
    if (!result.success) throw new Error();
    const output = new TextDecoder().decode(result.stdout).trim();
    // sqlite3 -json emits zero bytes for an empty set.
    return command === "sqlite3" && !output ? [] : JSON.parse(output);
  } catch {
    throw new ProbeError(label);
  }
}

// Match the scheduler's batch order. A committed retry supersedes earlier
// failures for that key. Manual runs use different keys, even on the same date.
export function scheduledState(batches, limit, hasEligible) {
  for (let i = 0; i < limit; i++) {
    const b = batches.find((b) => b.batch === i);
    if (b?.committed) continue;
    if (b?.leased) return "in_progress";
    // prepare releases an expired lease as a failed attempt on its next tick.
    if ((b?.failed ?? 0) + (b?.expired ?? 0) >= 3) return "exhausted";
    if (hasEligible && (b?.failed || b?.expired)) return "retry_pending";
    if (hasEligible) return "eligible_pending";
    return i === 0 ? "empty_no_receipt" : "committed";
  }
  return "daily_limit_reached";
}

async function inspect() {
  const now = Date.now();
  let since = now - 86_400_000;
  if (Deno.args.length) {
    since = Date.parse(Deno.args[1]);
    if (
      Deno.args.length !== 2 || Deno.args[0] !== "--since" ||
      !/(Z|[+-]\d\d:\d\d)$/.test(Deno.args[1]) ||
      !Number.isSafeInteger(since) || since > now
    ) {
      throw new ProbeError("expected --since ISO_TIMESTAMP_WITH_TIMEZONE");
    }
  }
  const oc = (...args) => json("openclaw", [...args, "--json"], args.join(" "));
  const [gateway, entry, slot, modelStatus, catalogue] = await Promise.all([
    oc("health"),
    oc("config", "get", "plugins.entries.moon"),
    oc("config", "get", "plugins.slots.contextEngine"),
    oc("models", "status"),
    oc("models", "list"),
  ]);
  const settings = adapter.resolveSettings({
    pluginConfig: entry.config,
    config: {
      agents: {
        defaults: {
          model: {
            primary: modelStatus.resolvedDefault,
            fallbacks: modelStatus.fallbacks,
          },
        },
      },
    },
  });
  if (!settings.moonHome || !isAbsolute(settings.moonHome)) {
    throw new ProbeError("explicit absolute Moon home required");
  }
  const db = join(settings.moonHome, "state", "moon.sqlite");
  const moon = (...args) =>
    json(settings.moonPath, [
      "--home",
      settings.moonHome,
      "--database",
      db,
      "--dimensions",
      String(settings.dimensions),
      "--json",
      ...args,
    ], "moon " + args.slice(0, 2).join(" "));
  const sql = (query) =>
    json(
      "sqlite3",
      ["-readonly", "-json", db, query],
      "read-only learning ledger",
    );
  const [identity, health, config, status] = await Promise.all([
    moon("--version"),
    moon("health"),
    moon("config", "validate"),
    moon("learning", "status"),
  ]);
  if (identity.version !== manifest.version) {
    throw new ProbeError("use the checkout for the installed Moon version");
  }
  const l1 = adapter.stageSettings(settings, config, "l1");
  const l2 = adapter.stageSettings(settings, config, "l2");
  const schedule = config.learning.l2;
  const window = adapter.dailySynthesisWindow(
    now,
    schedule.daily_at,
    schedule.timezone,
  );
  const key = window.key;
  if (!/^\d{4}-\d{2}-\d{2}$/.test(key)) {
    throw new ProbeError("invalid occurrence");
  }
  l2.promptText = await adapter.readLearningPrompt(l2.promptFile);
  const [preview, batches, lastCommit, [l1Activity], [turns]] = await Promise
    .all([
      moon(
        "learning",
        "prepare",
        "--preview",
        "--run-key",
        `l2:${key}:0`,
        "--occurrence",
        key,
        "--before-ms",
        String(window.cutoffMs),
        "--rule-version",
        adapter.synthesisRuleVersion(l2),
        "--limit",
        String(schedule.batch_size),
        "--max-chars",
        String(schedule.max_input_chars),
      ),
      sql(`SELECT CAST(substr(run_key,15) AS INTEGER) AS batch,
      sum(status='committed') AS committed, sum(status='failed') AS failed,
      count(*) FILTER (WHERE status='prepared' AND lease_until_ms>${now}) AS leased,
      count(*) FILTER (WHERE status='prepared' AND lease_until_ms<=${now}) AS expired
      FROM learning_runs WHERE run_key GLOB 'l2:${key}:[0-9]*'
      AND run_key=printf('l2:${key}:%d',cast(substr(run_key,15) AS INTEGER)) GROUP BY run_key ORDER BY batch`),
      sql(`SELECT completed_at_ms, CASE WHEN run_key GLOB 'l2:????-??-??:[0-9]*'
      THEN 'scheduled_key' ELSE 'other_key' END AS kind,
      json_extract(outcome_json,'$.metadata.execution.requested.model') AS requested_model,
      json_extract(outcome_json,'$.metadata.execution.requested.reasoning') AS requested_effort,
      json_extract(outcome_json,'$.metadata.execution.identity') AS identity,
      json_extract(outcome_json,'$.metadata.execution.observed.model') AS observed_model,
      json_extract(outcome_json,'$.metadata.execution.observed.backend') AS observed_runtime,
      json_extract(outcome_json,'$.metadata.execution.observed.reasoning') AS observed_effort
      FROM learning_runs WHERE status='committed' ORDER BY completed_at_ms DESC LIMIT 1`),
      sql(
        `SELECT count(*) AS recorded_turns, count(*) FILTER (WHERE learning_eligible) AS eligible,
      count(*) FILTER (WHERE status='error') AS errors, coalesce(sum(accepted_memories),0) AS accepted,
      max(occurred_at_ms) FILTER (WHERE learning_eligible OR status='error') AS latest_attempt_ms,
      max(occurred_at_ms) FILTER (WHERE status='error') AS latest_error_ms
      FROM runtime_metrics WHERE event_kind='learning' AND occurred_at_ms>=${since}`,
      ),
      sql(
        `SELECT count(*) AS retrievals, count(*) FILTER (WHERE status='error') AS retrieval_errors,
      count(*) FILTER (WHERE adapter_injected=1) AS packets_injected,
      (SELECT count(*) FROM privacy_admissions WHERE sender IS NOT NULL AND room_access_at_ms>=${since}) AS verified_admissions,
      (SELECT count(*) FROM evidence_admissions a JOIN evidence_sessions e ON a.evidence_id=e.id
       WHERE e.recorded_at_ms>=${since}) AS evidence_records_with_admission
      FROM context_metrics WHERE occurred_at_ms>=${since}`,
      ),
    ]);
  if (!["empty", "preview"].includes(preview.status)) {
    throw new ProbeError("unexpected learning preview");
  }
  const selected = preview.status === "empty"
    ? 0
    : preview.evidence.filter((e) => e.selected).length;
  const current = scheduledState(
    batches,
    schedule.max_batches_per_day,
    selected > 0,
  );
  const failures = [];
  const check = (ok, name) => {
    if (!ok) failures.push(name);
    return Boolean(ok);
  };
  const services = {
    gateway: check(gateway.ok && gateway.coreReady, "gateway"),
    model_runtime: check(
      gateway.modelRuntime?.degraded === false,
      "model_runtime",
    ),
    moon_plugin: check(
      gateway.plugins?.loaded?.includes("moon") && slot === "moon" &&
        entry.enabled !== false,
      "moon_plugin",
    ),
    learning_hooks: check(
      entry.hooks?.allowConversationAccess === true &&
        entry.hooks?.allowPromptInjection !== false,
      "learning_hooks",
    ),
  };
  const discord = gateway.channels?.discord;
  if (discord?.enabled !== false && discord?.configured) {
    const accounts = Object.values(discord.accounts ?? { default: discord })
      .filter((a) => a.enabled !== false && a.configured);
    services.discord = check(
      accounts.length > 0 && accounts.every((a) => a.running && a.connected),
      "discord",
    );
  }
  const route = modelStatus.auth.runtimeAuthRoutes?.some((r) =>
    r.provider === "openai" && r.runtime === "codex" && r.status === "usable"
  );
  const auth = modelStatus.auth.providers.find((p) => p.provider === "openai")
    ?.profiles;
  services.codex_oauth = check(
    route && gateway.plugins?.loaded?.includes("codex") && auth?.oauth > 0 &&
      auth.apiKey === 0 && auth.token === 0 &&
      modelStatus.auth.oauth.profiles.some((p) =>
        p.provider === "openai" && p.type === "oauth" && p.status === "ok"
      ),
    "codex_oauth",
  );
  check(
    health.ok && health.dead_embeddings === 0 && health.failed_embeddings === 0,
    "moon_storage",
  );
  check(
    settings.learningEnabled && l1.stageEnabled && l2.stageEnabled,
    "learning_enabled",
  );
  for (
    const model of [
      l1.primaryModel,
      l1.fallbackModel,
      l2.primaryModel,
      l2.fallbackModel,
    ].filter(Boolean)
  ) {
    check(
      catalogue.models.some((m) => m.key === model && m.available),
      "unavailable_model:" + model,
    );
  }
  check(current !== "exhausted", "scheduled_l2_attempts_exhausted");
  check(
    l1Activity.errors === 0 ||
      l1Activity.latest_error_ms < l1Activity.latest_attempt_ms,
    "latest_l1_attempt_in_window_failed",
  );
  const stage = (s) => ({
    model: s.primaryModel,
    requested_effort: s.primaryReasoning,
    fallback: s.fallbackModel,
  });
  return {
    checked_at: new Date(now).toISOString(),
    failures,
    services,
    storage: {
      version: identity.version,
      schema: health.schema_version,
      integrity: health.integrity,
      active_memories: health.active_memories,
      evidence_sessions: health.evidence_sessions,
      pending_embeddings: health.pending_embeddings,
      failed_embeddings: health.failed_embeddings,
      dead_embeddings: health.dead_embeddings,
    },
    l1: {
      ...stage(l1),
      since: new Date(since).toISOString(),
      turn_path: turns,
      metrics: l1Activity,
    },
    l2: {
      ...stage(l2),
      daily_at: schedule.daily_at,
      timezone: schedule.timezone,
      occurrence: key,
      state: current,
      scheduled_batches: batches,
      eligible_in_next_batch: selected,
      pending_evidence: status.pending_evidence,
      retry_pending: status.retry_pending_evidence,
      parked: status.parked_evidence,
      latest_committed_receipt: lastCommit[0] ?? null,
    },
  };
}

if (import.meta.main) {
  try {
    const report = await inspect();
    console.log(JSON.stringify(report, null, 2));
    Deno.exitCode = report.failures.length
      ? 1
      : ["eligible_pending", "retry_pending", "in_progress"].includes(
          report.l2.state,
        )
      ? 3
      : 0;
  } catch (error) {
    console.error(
      JSON.stringify({
        incomplete: error instanceof ProbeError
          ? error.message
          : "unable to inspect Moon configuration or status",
      }),
    );
    Deno.exitCode = 2;
  }
}

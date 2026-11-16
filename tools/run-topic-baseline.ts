// Frozen schema-10 baseline and explicit schema-11/12 candidate comparison.
// Optional packet sweep uses an explicitly supplied cached local embedding model.
// No downloads, live database writes, provider models or credentials.
// Run with the scoped permissions documented in docs/topic-baseline.md.
type Claim = {
  id: string;
  quote: string;
  original_start_byte?: number;
  original_end_byte?: number;
};
type Source = {
  key: string;
  scope: string;
  kind: string;
  title: string;
  author: string;
  content: string;
  claims: Claim[];
  expires?: boolean;
  withdraw?: boolean;
  revision?: { claims: Claim[] };
  provenance: { path?: string; sha256?: string; treatment: string };
};
type Scenario = {
  id: string;
  labels: string[];
  scope: string;
  question: string;
  diagnostic_query: string;
  expected_claims: string[];
  forbidden_claims?: string[];
  reference_answer: { text: string; supports: string[]; authorship: string };
  unavailable_feature?: string;
};
type Fixture = {
  schema_version: number;
  id: string;
  baseline_database_schema: number;
  binary_sha256: string;
  review_status: string;
  measurement: {
    mode: string;
    normal_max_chars: number;
    diagnostic_max_chars: number;
    limit: number;
    search_limit: number;
  };
  sources: Source[];
  scenarios: Scenario[];
};
type Citation = {
  source_key: string;
  revision_id: string;
  quote: string;
  start_byte: number;
  end_byte: number;
  start_line: number;
  end_line: number;
};
type Reference = {
  source_uri: string;
  scope: string;
  content: string;
  source_claim?: { claim_id: string; citations: Citation[] };
  topic?: {
    topic_id: string;
    statements: {
      kind: string;
      text: string;
      input_ids: string[];
      citations: Citation[];
    }[];
  };
};
type Packet = {
  used_chars: number;
  truncated: boolean;
  references: Reference[];
  memories: unknown[];
};
type Captured = { revision_id: string; content?: string };
type Prepared = { status: string; job_id: string; lease_token: string };
type Review = { status: string; result_hash: string };
type Accepted = { claim_ids: string[] };
const enc = new TextEncoder();
const dec = new TextDecoder("utf-8", { fatal: true });
function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
async function sha(bytes: Uint8Array) {
  const digest = await crypto.subtle.digest("SHA-256", new Uint8Array(bytes));
  return Array.from(
    new Uint8Array(digest),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
}
const root = Deno.cwd();
const sweep = Deno.args[0] === "--packet-sweep";
const candidate = sweep || Deno.args[0] === "--candidate";
const args = candidate ? Deno.args.slice(1) : Deno.args;
assert(
  args.length <= 2 ||
    (sweep && args.length === 4 && args[2] === "--hybrid-cache"),
  "Expected binary, output and optional --hybrid-cache PATH for a packet sweep",
);
const hybridCache = args[2] === "--hybrid-cache" ? args[3] : undefined;
assert(!hybridCache || hybridCache.startsWith("/"), "Cache must be absolute");
const retrievalMode = hybridCache ? "hybrid" : "lexical";
const budgets = [3_500, 6_000, 8_000, 14_000];
const binary = args[0] ??
  root +
    (sweep
      ? "/target/packet-after-moon"
      : candidate
      ? "/target/topic-candidate-moon"
      : "/target/topic-baseline-moon");
const output = args[1] ??
  root +
    (sweep
      ? "/target/packet-after-lexical.json"
      : candidate
      ? "/target/topic-candidate-report.json"
      : "/target/topic-baseline-report.json");
assert(
  !candidate || output !== root + "/target/topic-baseline-report.json",
  "Candidate mode must not overwrite the frozen baseline report",
);
assert(
  !sweep || ![
    "/target/topic-baseline-report.json",
    "/target/topic-candidate-report.json",
  ].some((path) => output === root + path),
  "Packet sweep must not overwrite earlier milestone reports",
);
assert(
  binary.startsWith("/") && output.startsWith(root + "/target/"),
  "Use an absolute binary and a report inside this checkout's target directory",
);
const fixtureBytes = await Deno.readFile(
  root + "/tests/fixtures/topic_baseline.json",
);
const fixture = JSON.parse(dec.decode(fixtureBytes)) as Fixture;
assert(
  fixture.schema_version === 1 && fixture.baseline_database_schema === 10,
  "Unsupported baseline fixture",
);
const binaryHash = await sha(await Deno.readFile(binary));
assert(
  candidate || binaryHash === fixture.binary_sha256,
  "The selected binary is not the frozen reviewed schema10 baseline",
);
const provenance = [];
for (const source of fixture.sources) {
  if (source.provenance.path) {
    const bytes = await Deno.readFile(root + "/" + source.provenance.path);
    const matches = await sha(bytes) === source.provenance.sha256;
    for (const claim of source.claims) {
      // Provenance drift is reported; frozen evidence remains reproducible after doc edits.
      if (matches) {
        assert(
          dec.decode(
            bytes.slice(claim.original_start_byte, claim.original_end_byte),
          ) === claim.quote,
          "Invalid original source range: " + claim.id,
        );
      }
    }
    provenance.push({
      path: source.provenance.path,
      current_file_matches_snapshot: matches,
    });
  }
}
const temporary = await Deno.makeTempDir({
  dir: root + "/target",
  prefix: "topic-baseline-work-",
});
const database = temporary + "/state/moon.sqlite";
const bodies = new Map<string, string>();
const actualToFixture = new Map<string, string>();
const fixtureToActual = new Map<string, string>();
const topicToScenario = new Map<string, string>();
const forbiddenTopics = new Set<string>();
const withdrawnTopicChecks: {
  topic_id: string;
  freshness: string;
  indexed_results: number;
}[] = [];
const candidateTopics: {
  scenario: string;
  topic_id: string;
  revision_id: string;
  title: string;
}[] = [];
const claims = new Map<string, Claim>();
const sourceForClaim = new Map<string, string>();
for (const source of fixture.sources) {
  for (const claim of [...source.claims, ...(source.revision?.claims ?? [])]) {
    assert(!claims.has(claim.id), "Duplicate fixture claim ID");
    claims.set(claim.id, claim);
    sourceForClaim.set(claim.id, source.key);
  }
}
let commandCount = 0;
async function moon<T>(args: string[], input?: unknown): Promise<T> {
  commandCount++;
  // macOS's process sandbox enforces no networking for the opt-in local-model
  // experiment, including inside the Rust subprocess. Cached public weights are
  // copied into the temporary home; the installed model cache is read-only input.
  const child = new Deno.Command(
    hybridCache ? "/usr/bin/sandbox-exec" : binary,
    {
      args: [
        ...(hybridCache
          ? ["-p", "(version 1) (allow default) (deny network*)", binary]
          : []),
        "--home",
        temporary,
        "--database",
        database,
        "--dimensions",
        hybridCache ? "384" : "64",
        "--json",
        ...args,
      ],
      cwd: temporary,
      clearEnv: true,
      env: {
        HOME: temporary,
        TMPDIR: temporary,
        HF_HUB_OFFLINE: "1",
        HF_HUB_DISABLE_TELEMETRY: "1",
      },
      stdin: "piped",
      stdout: "piped",
      stderr: "piped",
    },
  ).spawn();
  const writer = child.stdin.getWriter();
  if (input !== undefined) {
    await writer.write(enc.encode(JSON.stringify(input)));
  }
  await writer.close();
  const result = await child.output();
  assert(
    result.success,
    "Baseline CLI step failed: " + args.slice(0, 3).join(" ") + " (exit " +
      result.code + ")",
  );
  return JSON.parse(dec.decode(result.stdout)) as T;
}
async function captureAndAccept(
  source: Source,
  selected: Claim[],
  previous?: string,
) {
  const content = selected.map((c) => c.quote).join("\n\n");
  const capture = await moon<Captured>([
    "source",
    "import",
    "--scope",
    source.scope,
    "--allow-learning",
    ...(previous ? ["--expected-revision", previous] : []),
  ], {
    schema_version: 1,
    source_key: source.key,
    kind: source.kind,
    format: "text",
    title: source.title,
    author: source.author,
    content,
  });
  const retained = await moon<Captured>([
    "source",
    "get",
    "--scope",
    source.scope,
    "--key",
    source.key,
  ]);
  assert(
    retained.content === content,
    "Fixture unexpectedly changed during sanitisation",
  );
  bodies.set(capture.revision_id, content);
  const prepared = await moon<Prepared>([
    "source",
    "learning",
    "prepare",
    "--scope",
    source.scope,
    "--key",
    source.key,
  ]);
  assert(prepared.status === "prepared", "Expected a bounded source job");
  const review = await moon<Review>([
    "source",
    "learning",
    "submit",
    "--scope",
    source.scope,
    "--job-id",
    prepared.job_id,
    "--lease-token",
    prepared.lease_token,
  ], {
    schema_version: 1,
    // Fixed historical expiry avoids clock sleeps and exercises read-time exclusion.
    claims: selected.map((claim) => ({
      quote: claim.quote,
      valid_until_ms: source.expires ? 946684800000 : null,
    })),
  });
  const accepted = await moon<Accepted>([
    "source",
    "learning",
    "accept",
    "--scope",
    source.scope,
    "--job-id",
    prepared.job_id,
    "--expected-result-hash",
    review.result_hash,
    "--yes",
  ]);
  assert(
    accepted.claim_ids.length === selected.length,
    "Not every fixture claim was accepted",
  );
  for (const id of accepted.claim_ids) {
    const stored = await moon<{ content: string }>([
      "source",
      "claims",
      "get",
      "--scope",
      source.scope,
      "--claim-id",
      id,
    ]);
    const match = selected.find((c) => c.quote === stored.content);
    assert(match, "Accepted claim differs from fixture evidence");
    actualToFixture.set(id, match.id);
    fixtureToActual.set(match.id, id);
  }
  return capture;
}
async function compileTopic(
  scope: string,
  key: string,
  title: string,
  ids: string[],
  text: string,
  kind: string,
) {
  const definition = await moon<{ topic_id: string }>([
    "topic",
    "put",
    "--scope",
    scope,
  ], { schema_version: 1, key, title, claim_ids: ids });
  const prepared = await moon<Prepared>([
    "topic",
    "prepare",
    "--scope",
    scope,
    "--key",
    key,
  ]);
  assert(
    prepared.status === "prepared",
    "Candidate topic inputs are not available",
  );
  const review = await moon<Review>([
    "topic",
    "submit",
    "--scope",
    scope,
    "--job-id",
    prepared.job_id,
    "--lease-token",
    prepared.lease_token,
  ], {
    schema_version: 1,
    review_complete: true,
    statements: [{ kind, text, input_ids: ids }],
  });
  const accepted = await moon<{ revision_id: string }>([
    "topic",
    "accept",
    "--scope",
    scope,
    "--job-id",
    prepared.job_id,
    "--expected-result-hash",
    review.result_hash,
    "--yes",
  ]);
  return { topic_id: definition.topic_id, revision_id: accepted.revision_id };
}
function inspectCitation(
  citation: Citation,
  violations: string[],
  displayed?: string,
) {
  const body = bodies.get(citation.revision_id);
  if (body === undefined) {
    violations.push("missing_original_source");
    return;
  }
  const bytes = enc.encode(body);
  const original = dec.decode(
    bytes.slice(citation.start_byte, citation.end_byte),
  );
  const startLine = 1 +
    bytes.slice(0, citation.start_byte).filter((b) => b === 10).length;
  const endLine = 1 +
    bytes.slice(0, citation.end_byte - 1).filter((b) => b === 10).length;
  if (
    original !== citation.quote ||
    (displayed !== undefined && citation.quote !== displayed) ||
    startLine !== citation.start_line || endLine !== citation.end_line
  ) violations.push("citation_mismatch");
}
function inspectPacket(packet: Packet, scenario: Scenario, budget: number) {
  const returned: string[] = [];
  const complete: string[] = [];
  const partial: string[] = [];
  let citationCount = 0;
  let topicCitationCount = 0;
  const returnedTopics: string[] = [];
  const completeExplanations: string[] = [];
  const violations: string[] = [];
  if (packet.used_chars > budget) violations.push("packet_budget_exceeded");
  if (packet.memories.length) violations.push("unexpected_personal_memory");
  for (const reference of packet.references) {
    if (reference.scope !== scenario.scope) {
      violations.push("cross_scope_reference");
    }
    if (reference.topic) {
      if (!candidate) violations.push("topic_in_frozen_baseline");
      const topic = reference.topic;
      const labelled = topicToScenario.get(topic.topic_id);
      if (!labelled) violations.push("unmapped_topic_reference");
      returnedTopics.push(labelled ?? topic.topic_id);
      if (forbiddenTopics.has(topic.topic_id)) {
        violations.push("stale_topic_returned");
      }
      for (const block of topic.statements) {
        if (
          !["fact", "interpretation", "disagreement", "question"].includes(
            block.kind,
          )
        ) violations.push("unlabelled_topic_block");
        if (
          labelled === scenario.id &&
          block.text === scenario.reference_answer.text
        ) completeExplanations.push(scenario.id);
        for (const citation of block.citations) {
          topicCitationCount++;
          inspectCitation(citation, violations);
        }
      }
      continue;
    }
    const claimId = reference.source_claim &&
      actualToFixture.get(reference.source_claim.claim_id);
    if (!claimId) {
      violations.push("unmapped_source_reference");
      continue;
    }
    returned.push(claimId);
    const expected = claims.get(claimId)!;
    if (scenario.expected_claims.includes(claimId)) {
      (reference.content.includes(expected.quote) ? complete : partial).push(
        claimId,
      );
    }
    if (!reference.source_claim!.citations.length) {
      violations.push("uncited_reference");
    }
    for (const citation of reference.source_claim!.citations) {
      citationCount++;
      inspectCitation(citation, violations, reference.content);
    }
  }
  const forbidden = returned.filter((id) =>
    scenario.forbidden_claims?.includes(id)
  );
  if (new Set(returned).size !== returned.length) {
    violations.push("duplicate_source_reference");
  }
  if (new Set(returnedTopics).size !== returnedTopics.length) {
    violations.push("duplicate_topic_reference");
  }
  if (forbidden.length) {
    violations.push("forbidden_inactive_or_unrelated_claim");
  }
  if (
    returnedTopics.length >
      Math.min(2, Math.floor(fixture.measurement.limit / 3))
  ) violations.push("topic_slot_quota_exceeded");
  return {
    returned_claims: returned,
    complete_expected_units: complete,
    partial_expected_units: partial,
    missing_expected_units: scenario.expected_claims.filter((id) =>
      !complete.includes(id)
    ),
    unlabelled_references: returned.filter((id) =>
      !scenario.expected_claims.includes(id)
    ),
    citation_count: citationCount,
    returned_topics: returnedTopics,
    complete_prepared_explanations: completeExplanations,
    topic_citation_count: topicCitationCount,
    violations,
    used_chars: packet.used_chars,
    truncated: packet.truncated,
    // Unlabelled does not mean false-positive: that judgement requires semantic review.
    packet,
  };
}
try {
  if (hybridCache) {
    await Deno.mkdir(temporary + "/models");
    const copied = await new Deno.Command("/bin/cp", {
      args: ["-cR", hybridCache, temporary + "/models/fastembed"],
      clearEnv: true,
      stdout: "null",
      stderr: "null",
    }).output();
    assert(copied.success, "Could not clone the supplied local model cache");
  }
  await moon(["init"]);
  for (const source of fixture.sources) {
    const first = await captureAndAccept(source, source.claims);
    if (source.revision) {
      await captureAndAccept(source, source.revision.claims, first.revision_id);
    }
    if (source.withdraw) {
      let withdrawnTopic: string | undefined;
      if (candidate) {
        const ids = source.claims.map((c) => fixtureToActual.get(c.id)!);
        const old = await compileTopic(
          source.scope,
          "obsolete-cached-policy",
          "Cached source policy",
          ids,
          source.claims[0].quote,
          "fact",
        );
        topicToScenario.set(old.topic_id, "withdrawn-cached-support");
        forbiddenTopics.add(old.topic_id);
        withdrawnTopic = old.topic_id;
      }
      const state = await moon<{ generation: number; revision_id: string }>([
        "source",
        "state",
        "--scope",
        source.scope,
        "--key",
        source.key,
      ]);
      await moon([
        "source",
        "withdraw",
        "--scope",
        source.scope,
        "--key",
        source.key,
        "--expected-revision",
        state.revision_id,
        "--expected-generation",
        String(state.generation),
        "--reason",
        "Labelled synthetic withdrawal challenge",
      ]);
      if (withdrawnTopic) {
        const state = await moon<{ freshness: string }>([
          "topic",
          "get",
          "--scope",
          source.scope,
          "--key",
          "obsolete-cached-policy",
        ]);
        const hits = await moon<unknown[]>([
          "search",
          "--scope",
          source.scope,
          "--query",
          "Moon",
          "--mode",
          "lexical",
          "--kind",
          "topic_summary",
          "--limit",
          "50",
        ]);
        assert(
          state.freshness !== "fresh" && hits.length === 0,
          "Withdrawn cached topic remains current or indexed for recall",
        );
        withdrawnTopicChecks.push({
          topic_id: withdrawnTopic,
          freshness: state.freshness,
          indexed_results: hits.length,
        });
      }
    }
  }
  if (candidate) {
    for (const scenario of fixture.scenarios) {
      // The fallback reference explicitly describes schema10's lack of topics;
      // retain that question unchanged and measure the newly stale cached topic instead.
      if (!scenario.expected_claims.length || scenario.unavailable_feature) {
        continue;
      }
      const title = scenario.id.split("-").join(" ");
      const ids = scenario.expected_claims.map((id) =>
        fixtureToActual.get(id)!
      );
      const compiled = await compileTopic(
        scenario.scope,
        "reviewed-" + scenario.id,
        title,
        ids,
        scenario.reference_answer.text,
        scenario.labels.includes("disagreement")
          ? "disagreement"
          : "interpretation",
      );
      topicToScenario.set(compiled.topic_id, scenario.id);
      candidateTopics.push({ scenario: scenario.id, ...compiled, title });
    }
  }
  if (hybridCache) await moon(["embed", "--provider", "local", "--drain"]);
  type Measurement = ReturnType<typeof inspectPacket> & {
    cli_elapsed_ms: number;
  };
  const cases: {
    id: string;
    labels: string[];
    question: string;
    scope: string;
    reference_answer: Scenario["reference_answer"];
    expected_units: number;
    required_distinct_sources: number;
    expected_search_ranks: { claim: string; rank: number | null }[];
    topic_search_ranks?: {
      topic: string;
      rank: number;
      title: string | null;
    }[];
    normal: Measurement;
    larger_budget_diagnostic: Measurement;
    keyword_diagnostic: Measurement;
    budget_sweep?: Record<string, Measurement>;
    unavailable_feature: string | null;
  }[] = [];
  for (const scenario of fixture.scenarios) {
    assert(
      scenario.expected_claims.every((id) => claims.has(id)),
      "Unresolved reference answer support",
    );
    const context = async (query: string, budget: number) => {
      const started = performance.now();
      const packet = await moon<Packet>([
        "context",
        "--scope",
        scenario.scope,
        "--query",
        query,
        "--mode",
        retrievalMode,
        ...(hybridCache ? ["--provider", "local"] : []),
        "--max-chars",
        String(budget),
        "--limit",
        String(fixture.measurement.limit),
      ]);
      return {
        ...inspectPacket(packet, scenario, budget),
        cli_elapsed_ms: Math.round((performance.now() - started) * 100) / 100,
      };
    };
    const search = await moon<{ source_uri: string }[]>([
      "search",
      "--scope",
      scenario.scope,
      "--query",
      scenario.question,
      "--mode",
      retrievalMode,
      ...(hybridCache ? ["--provider", "local"] : []),
      "--limit",
      String(fixture.measurement.search_limit),
    ]);
    const ranks = search.map((hit, index) => ({
      claim: actualToFixture.get(
        hit.source_uri.replace("moon-source-claim://", ""),
      ),
      rank: index + 1,
    }));
    const topicRanks = sweep
      ? (await moon<{ source_uri: string; title?: string }[]>([
        "search",
        "--scope",
        scenario.scope,
        "--query",
        scenario.question,
        "--kind",
        "topic_summary",
        "--mode",
        retrievalMode,
        ...(hybridCache ? ["--provider", "local"] : []),
        "--limit",
        "16",
      ])).map((hit, index) => ({
        topic: topicToScenario.get(hit.source_uri.split("/")[2]) ?? "unmapped",
        rank: index + 1,
        title: hit.title ?? null,
      }))
      : undefined;
    const packets: Record<string, Awaited<ReturnType<typeof context>>> = {};
    for (
      const budget of sweep ? budgets : [
        fixture.measurement.normal_max_chars,
        fixture.measurement.diagnostic_max_chars,
      ]
    ) {
      packets[String(budget)] = await context(scenario.question, budget);
    }
    const normal = packets[String(fixture.measurement.normal_max_chars)];
    const larger = packets[String(fixture.measurement.diagnostic_max_chars)];
    const keyword = await context(
      scenario.diagnostic_query,
      fixture.measurement.normal_max_chars,
    );
    cases.push({
      id: scenario.id,
      labels: scenario.labels,
      question: scenario.question,
      scope: scenario.scope,
      reference_answer: scenario.reference_answer,
      expected_units: scenario.expected_claims.length,
      required_distinct_sources: new Set(scenario.expected_claims.map((id) =>
        sourceForClaim.get(id)
      )).size,
      expected_search_ranks: scenario.expected_claims.map((id) => ({
        claim: id,
        rank: ranks.find((r) => r.claim === id)?.rank ?? null,
      })),
      ...(sweep ? { topic_search_ranks: topicRanks } : {}),
      normal,
      larger_budget_diagnostic: larger,
      keyword_diagnostic: keyword,
      ...(sweep ? { budget_sweep: packets } : {}),
      unavailable_feature: scenario.unavailable_feature ?? null,
    });
  }
  const health = await moon<{ ok: boolean; schema_version: number }>([
    "health",
  ]);
  assert(
    health.ok &&
      (candidate
        ? [11, 12].includes(health.schema_version)
        : health.schema_version === fixture.baseline_database_schema),
    "Baseline database health or schema mismatch",
  );
  assert(
    await sha(await Deno.readFile(binary)) === binaryHash,
    "Selected binary changed during the run; discard this mixed comparison",
  );
  const aggregate = (subset: {
    expected_units: number;
    normal: ReturnType<typeof inspectPacket>;
    larger_budget_diagnostic: ReturnType<typeof inspectPacket>;
    keyword_diagnostic: ReturnType<typeof inspectPacket>;
    expected_search_ranks: { rank: number | null }[];
  }[]) => ({
    scenarios: subset.length,
    expected_units: subset.reduce((n, c) => n + c.expected_units, 0),
    complete_normal_units: subset.reduce(
      (n, c) => n + c.normal.complete_expected_units.length,
      0,
    ),
    partially_visible_normal_units: subset.reduce(
      (n, c) => n + c.normal.partial_expected_units.length,
      0,
    ),
    complete_larger_budget_units: subset.reduce(
      (n, c) => n + c.larger_budget_diagnostic.complete_expected_units.length,
      0,
    ),
    expected_units_found_in_top50: subset.reduce(
      (n, c) =>
        n + c.expected_search_ranks.filter((r) => r.rank !== null).length,
      0,
    ),
    complete_normal_scenarios: subset.filter((c) =>
      c.normal.complete_expected_units.length === c.expected_units
    ).length,
    normal_citations: subset.reduce((n, c) =>
      n + c.normal.citation_count, 0),
    normal_topic_references: subset.reduce(
      (n, c) => n + c.normal.returned_topics.length,
      0,
    ),
    complete_normal_prepared_explanations: subset.reduce(
      (n, c) => n + c.normal.complete_prepared_explanations.length,
      0,
    ),
    complete_larger_budget_prepared_explanations: subset.reduce(
      (n, c) =>
        n + c.larger_budget_diagnostic.complete_prepared_explanations.length,
      0,
    ),
    normal_topic_citations: subset.reduce(
      (n, c) => n + c.normal.topic_citation_count,
      0,
    ),
    all_packet_violations: subset.flatMap((
      c,
    ) => [c.normal, c.larger_budget_diagnostic, c.keyword_diagnostic])
      .flatMap((p) => p.violations).length,
  });
  const report = {
    schema_version: 1,
    fixture: fixture.id,
    fixture_sha256: await sha(fixtureBytes),
    binary_sha256: binaryHash,
    evaluation_mode: sweep
      ? "four_budget_packet_sweep_with_reviewed_topics"
      : candidate
      ? "candidate_with_reviewed_topics"
      : "frozen_source_claim_baseline",
    frozen_baseline_binary_sha256: fixture.binary_sha256,
    generated_at: new Date().toISOString(),
    review_status: fixture.review_status,
    conditions: {
      database_schema: health.schema_version,
      ...fixture.measurement,
      mode: retrievalMode,
      sources: fixture.sources.length,
      retained_claim_revisions: claims.size,
      no_models: !hybridCache,
      no_provider_models: true,
      local_embedding_model: hybridCache
        ? "intfloat/multilingual-e5-small@fastembed-5.17.3:query-passage-v1:max512"
        : null,
      no_network: true,
      network_enforcement: hybridCache
        ? "sandbox-exec deny network*"
        : "no network-capable calls",
      temporary_home_and_database: true,
      historical_expiry_fixture_ms: 946684800000,
    },
    interpretation:
      "Exact-unit retrieval and citation checks only. Reference explanations are agent-prepared. No human usefulness, answer-model quality, repeated-research time, production route or representative performance claim. CLI timings include process startup and any local model load. Larger/keyword packets are diagnostics, not a changed baseline. Candidate topics store reviewed reference interpretations; complete explanation presence is reported separately from exact source units. Topic dependency cycles and transitive purge are verified by integration tests, not this corpus.",
    candidate_topics: candidateTopics,
    withdrawn_topic_checks: withdrawnTopicChecks,
    provenance,
    summary: aggregate(cases),
    repository_summary: aggregate(
      cases.filter((c) => c.labels.includes("repository")),
    ),
    challenge_summary: aggregate(
      cases.filter((c) => c.labels.includes("challenge")),
    ),
    ...(sweep
      ? {
        budget_summary: budgets.map((budget) => {
          const packets = cases.map((c) => c.budget_sweep![String(budget)]);
          const chars = packets.map((p) => p.used_chars).sort((a, b) => a - b);
          const timings = packets.map((p) => p.cli_elapsed_ms).sort((a, b) =>
            a - b
          );
          return {
            max_chars: budget,
            cases: packets.length,
            complete_source_units: packets.reduce(
              (n, p) => n + p.complete_expected_units.length,
              0,
            ),
            partial_source_units: packets.reduce(
              (n, p) => n + p.partial_expected_units.length,
              0,
            ),
            complete_prepared_explanations: packets.reduce(
              (n, p) => n + p.complete_prepared_explanations.length,
              0,
            ),
            source_citations: packets.reduce((n, p) => n + p.citation_count, 0),
            topic_citations: packets.reduce(
              (n, p) => n + p.topic_citation_count,
              0,
            ),
            unlabelled_source_references: packets.reduce(
              (n, p) => n + p.unlabelled_references.length,
              0,
            ),
            used_chars_total: chars.reduce((n, c) => n + c, 0),
            used_chars_median: chars[Math.floor(chars.length / 2)],
            used_chars_max: chars.at(-1),
            cli_elapsed_ms_median: timings[Math.floor(timings.length / 2)],
            packet_violations: packets.flatMap((p) => p.violations).length,
          };
        }),
      }
      : {}),
    cases,
    health: { ok: health.ok, schema_version: health.schema_version },
    cli_commands: commandCount,
  };
  await Deno.writeTextFile(output, JSON.stringify(report, null, 2) + "\n", {
    mode: 0o600,
  });
  console.log(
    JSON.stringify(
      {
        report: output,
        ...report.summary,
        repository: report.repository_summary,
        challenge: report.challenge_summary,
        ...(sweep ? { budgets: report.budget_summary } : {}),
      },
      null,
      2,
    ),
  );
  assert(
    report.summary.all_packet_violations === 0,
    "Baseline safety checks failed; inspect the saved report",
  );
  assert(
    !sweep ||
      report.budget_summary!.every((row) => row.packet_violations === 0),
    "Packet sweep checks failed; inspect the saved report",
  );
} finally {
  await Deno.remove(temporary, { recursive: true });
}

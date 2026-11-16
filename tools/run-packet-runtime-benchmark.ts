// Warm stdio packet assembly on bounded, synthetic reference corpora.
// Usage: deno run --allow-read=target --allow-write=target \
//   --allow-run=/absolute/before,/absolute/after tools/run-packet-runtime-benchmark.ts \
//   /absolute/before /absolute/after /absolute/target/report.json [1000,10000] [25]
// Use - for the second binary to collect baseline-only timings.
// Optional: --seeds /absolute/target/packet-runtime-seeds --budgets 8000
//           --require-parity (assert identical complete structured packets).
//           --observe (include durable local context-metric recording).
// No network, provider model, credentials or live Moon home is used. The hash
// provider only satisfies the worker interface; every timed request is lexical.
type Citation = {
  source_key: string;
  quote: string;
  start_byte: number;
  end_byte: number;
};
type Reference = {
  scope: string;
  source_uri: string;
  content: string;
  start_byte: number;
  end_byte: number;
  source_claim?: { citations: Citation[] };
  topic?: { statements: { citations: Citation[] }[] };
};
type Packet = {
  used_chars: number;
  references: Reference[];
  memories: unknown[];
};
const root = Deno.cwd();
const positional = Deno.args.filter((_, i, args) =>
  !args[i].startsWith("--") &&
  (i === 0 || !["--seeds", "--budgets"].includes(args[i - 1]))
);
const [before, after, output, sizesArg = "1000,10000", repeatsArg = "25"] =
  positional;
const option = (name: string) => {
  const index = Deno.args.indexOf(name);
  return index < 0 ? undefined : Deno.args[index + 1];
};
const sizes = sizesArg.split(",").map(Number);
const repeats = Number(repeatsArg);
const budgets = (option("--budgets") ?? "3500,6000,8000,14000").split(",").map(
  Number,
);
const requireParity = Deno.args.includes("--require-parity");
const observe = Deno.args.includes("--observe");
function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}
check(
  before?.startsWith("/") && (after === "-" || after?.startsWith("/")) &&
    output?.startsWith(root + "/target/") &&
    /^[a-zA-Z0-9_.-]+\.json$/.test(output.slice((root + "/target/").length)),
  "Use two absolute binaries and a JSON report directly under this checkout's target/",
);
check(
  positional.length <= 5 &&
    Deno.args.filter((arg) => arg.startsWith("--")).every((arg) =>
      ["--seeds", "--budgets", "--require-parity", "--observe"].includes(arg)
    ),
  "Unknown arguments",
);
check(!requireParity || after !== "-", "Parity requires two binaries");
for (const name of ["--seeds", "--budgets"]) {
  check(
    Deno.args.filter((arg) => arg === name).length <= 1 &&
      (!Deno.args.includes(name) ||
        (option(name) !== undefined && !option(name)!.startsWith("--"))),
    "Missing option value",
  );
}
check(
  sizes.length >= 1 && sizes.length <= 4 &&
    sizes.every((n) => Number.isSafeInteger(n) && n >= 20 && n <= 20_000) &&
    new Set(sizes).size === sizes.length &&
    Number.isSafeInteger(repeats) && repeats >= 2 && repeats <= 200 &&
    budgets.length >= 1 && budgets.length <= 4 &&
    budgets.every((n) => Number.isSafeInteger(n) && n >= 1000 && n <= 14000) &&
    new Set(budgets).size === budgets.length,
  "Invalid corpus sizes, packet budgets or repeat count",
);
try {
  await Deno.stat(output);
  throw new Error("Report already exists; choose a new output path");
} catch (error) {
  if (!(error instanceof Deno.errors.NotFound)) throw error;
}
const seedDirectory = option("--seeds") === undefined
  ? undefined
  : await Deno.realPath(option("--seeds")!);
check(
  seedDirectory === undefined ||
    seedDirectory.startsWith(await Deno.realPath(root + "/target") + "/"),
  "Retained synthetic seeds must be inside target/",
);
const encode = new TextEncoder();
const decode = new TextDecoder();
async function sha(path: string) {
  return Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", await Deno.readFile(path)),
    ),
    (v) => v.toString(16).padStart(2, "0"),
  ).join("");
}
const scope = "synthetic-packet-runtime";
const binaries = after === "-" ? [before] : [before, after];
const hashes = await Promise.all(binaries.map(sha));
const work = await Deno.makeTempDir({
  dir: root + "/target",
  prefix: "packet-runtime-",
});
const queries = [
  "How does the Archive review workflow preserve withdrawal history and restore source support?",
  "Which Archive backup safeguards apply before a storage migration?",
  "What does the Indigo sensor calibration record require?",
  "What is the Quartz spacecraft launch manifest?",
];
const queryLabels = [
  "cross_source",
  "migration",
  "raw_reference",
  "out_of_corpus",
];
function argumentsFor(home: string) {
  return [
    "--home",
    home,
    "--database",
    home + "/state/moon.sqlite",
    "--dimensions",
    "64",
    "--json",
  ];
}
function command(binary: string, home: string, args: string[], stdin = false) {
  return new Deno.Command(binary, {
    args: [...argumentsFor(home), ...args],
    clearEnv: true,
    env: { HOME: home, PATH: "/usr/bin:/bin", LANG: "C.UTF-8" },
    stdin: stdin ? "piped" : "null",
    stdout: "piped",
    stderr: "null",
  });
}
async function cli(
  binary: string,
  home: string,
  args: string[],
  input?: unknown,
) {
  const process = command(binary, home, args, input !== undefined).spawn();
  if (input !== undefined) {
    const writer = process.stdin.getWriter();
    await writer.write(encode.encode(JSON.stringify(input)));
    await writer.close();
  }
  const result = await process.output();
  check(result.success, "Isolated CLI failed at " + args.slice(0, 2).join(" "));
  return JSON.parse(decode.decode(result.stdout));
}
function percentile(values: number[], percentile: number) {
  const ordered = [...values].sort((a, b) => a - b);
  return Math.round(
    ordered[Math.ceil(ordered.length * percentile) - 1] * 1_000,
  ) / 1_000;
}
async function worker(binary: string, home: string) {
  const process = command(binary, home, ["serve", "--provider", "hash"], true)
    .spawn();
  const writer = process.stdin.getWriter();
  const reader = process.stdout.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  let sequence = 0;
  async function request(payload: Record<string, unknown>) {
    const id = ++sequence;
    await writer.write(
      encode.encode(JSON.stringify({ id, ...payload }) + "\n"),
    );
    while (!pending.includes("\n")) {
      const result = await reader.read();
      check(!result.done, "Isolated stdio worker ended before reply");
      pending += decoder.decode(result.value, { stream: true });
      check(
        pending.length < 2_000_000,
        "Worker reply exceeded benchmark bound",
      );
    }
    const boundary = pending.indexOf("\n");
    const reply = JSON.parse(pending.slice(0, boundary));
    pending = pending.slice(boundary + 1);
    check(
      reply.id === id && reply.ok === true,
      "Isolated stdio request failed",
    );
    return reply.result;
  }
  await request({ op: "ping" });
  return {
    request,
    async close() {
      await writer.close();
      await reader.cancel();
      const status = await process.status;
      check(status.success, "Isolated stdio worker did not shut down cleanly");
    },
  };
}
const cases = [];
try {
  for (const size of sizes) {
    const seed = work + "/seed-" + size;
    const corpus = seed + "/corpus";
    if (seedDirectory === undefined) {
      await Deno.mkdir(corpus, { recursive: true, mode: 0o700 });
    }
    const originals = new Map<string, string>();
    const quoted = new Map<string, string>();
    const corpusClaims = Math.floor(size / 5);
    const rawDocuments = size - corpusClaims;
    const categories = [
      "Archive review workflow preserves withdrawal history until explicit deletion is approved.",
      "Archive storage migration requires a verified backup and a reviewed restoration procedure.",
      "Indigo sensor calibration records require a laboratory supervisor to approve changed thresholds.",
      "Cobalt inventory procurement assigns each delivery a warehouse shelf and an audit timestamp.",
      "Amber watering observations record rainfall and irrigation changes without changing the forecast.",
      "Violet typography research compares reading comfort across printed signs and display panels.",
    ];
    for (let index = 0; index < rawDocuments; index++) {
      const path = corpus + "/reference-" + String(index).padStart(5, "0") +
        ".txt";
      const text = categories[index % categories.length] +
        ` The synthetic record number is ${index}. ` +
        "The record describes a local rehearsal and does not authorise a production action. " +
        "Preserve its conditions, original attribution and review date whenever the record is consulted. "
          .repeat(index % 4 + 1);
      originals.set(
        "reference-" + String(index).padStart(5, "0") + ".txt",
        text,
      );
      if (seedDirectory === undefined) {
        await Deno.writeTextFile(path, text, { mode: 0o600 });
      }
    }
    if (seedDirectory === undefined) {
      await cli(before, seed, [
        "ingest",
        "--path",
        corpus,
        "--recursive",
        "--scope",
        scope,
      ]);
    }
    const learnBatch = async (
      key: string,
      statements: string[],
    ): Promise<string[]> => {
      const content = statements.join("\n");
      quoted.set(key, content);
      if (seedDirectory !== undefined) return [];
      await cli(before, seed, [
        "source",
        "import",
        "--scope",
        scope,
        "--allow-learning",
      ], {
        schema_version: 1,
        source_key: key,
        kind: "external_document",
        content,
      });
      const prepared = await cli(before, seed, [
        "source",
        "learning",
        "prepare",
        "--scope",
        scope,
        "--key",
        key,
      ]);
      const proposed = await cli(before, seed, [
        "source",
        "learning",
        "submit",
        "--scope",
        scope,
        "--job-id",
        prepared.job_id,
        "--lease-token",
        prepared.lease_token,
      ], {
        schema_version: 1,
        claims: statements.map((quote) => ({ quote })),
      });
      const accepted = await cli(before, seed, [
        "source",
        "learning",
        "accept",
        "--scope",
        scope,
        "--job-id",
        prepared.job_id,
        "--expected-result-hash",
        proposed.result_hash,
        "--yes",
      ]);
      check(
        accepted.claim_ids.length === statements.length,
        "Fixture claims missing after review",
      );
      return accepted.claim_ids;
    };
    for (let offset = 0; offset < corpusClaims; offset += 16) {
      const statements = Array.from({
        length: Math.min(16, corpusClaims - offset),
      }, (_, i) => {
        const index = offset + i;
        return categories[index % categories.length] +
          ` Reviewed synthetic source statement ${index} concerns a local rehearsal only; preserve its qualifications.`;
      });
      await learnBatch("runtime-source-group-" + offset, statements);
    }
    const evidence = [
      "Archive withdrawal hides unsupported recall while retaining the source history; restoration does not approve new proposals.",
      "Archive backup snapshots must pass a restoration rehearsal before an authorised storage migration starts.",
      "Archive review is separate from truth verification and must preserve uncertainty in every attributed statement.",
    ];
    const claims: string[] = [];
    for (let i = 0; i < evidence.length; i++) {
      claims.push(...await learnBatch("runtime-canary-" + i, [evidence[i]]));
    }
    if (seedDirectory === undefined) {
      await cli(before, seed, ["topic", "put", "--scope", scope], {
        schema_version: 1,
        key: "archive-runtime",
        title: "Archive lifecycle and migration review",
        claim_ids: claims,
      });
      const prepared = await cli(before, seed, [
        "topic",
        "prepare",
        "--scope",
        scope,
        "--key",
        "archive-runtime",
      ]);
      const proposed = await cli(before, seed, [
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
        statements: [{
          kind: "interpretation",
          text:
            "Archive review preserves withdrawal history and requires backup restoration checks before a storage migration; restoration alone never approves a proposal.",
          input_ids: claims,
        }],
      });
      await cli(before, seed, [
        "topic",
        "accept",
        "--scope",
        scope,
        "--job-id",
        prepared.job_id,
        "--expected-result-hash",
        proposed.result_hash,
        "--yes",
      ]);
    }
    const snapshot = seedDirectory === undefined
      ? seed + "/snapshot.sqlite"
      : seedDirectory + "/" + size + ".sqlite";
    if (seedDirectory === undefined) {
      await cli(before, seed, ["backup", "--destination", snapshot]);
    }
    check(
      (await Deno.lstat(snapshot)).isFile,
      "Seed must be a regular snapshot, not a symlink",
    );
    // A copied standalone snapshot must not depend on an uncheckpointed WAL.
    for (const suffix of ["-wal", "-journal"]) {
      try {
        check(
          (await Deno.stat(snapshot + suffix)).size === 0,
          "Seed has pending journal data; create a consistent backup first",
        );
      } catch (error) {
        if (!(error instanceof Deno.errors.NotFound)) throw error;
      }
    }
    const snapshotHash = await sha(snapshot);
    const homes = binaries.map((_, variant) =>
      work + "/case-" + size + "-" + variant
    );
    const clients: Awaited<ReturnType<typeof worker>>[] = [];
    const observedIds = binaries.map(() => new Set<string>());
    const previousMetricCounts = binaries.map(() => 0);
    const samples = binaries.map(() =>
      budgets.map(() => ({
        elapsed: [] as number[],
        used: [] as number[],
        counts: [] as number[],
        queryElapsed: queries.map(() => [] as number[]),
        citationChecks: 0,
        signatures: queries.map(() => ""),
      }))
    );
    const parityDifferences = new Set<string>();
    let parityChecks = 0;
    try {
      for (const [variant, binary] of binaries.entries()) {
        const home = homes[variant];
        await Deno.mkdir(home + "/state", { recursive: true, mode: 0o700 });
        await Deno.copyFile(snapshot, home + "/state/moon.sqlite");
        await Deno.chmod(home + "/state/moon.sqlite", 0o600);
        // Migrate only the isolated copy, outside the timed interval.
        await cli(binary, home, ["init"]);
        if (observe) {
          const metrics = await cli(binary, home, [
            "metrics",
            "summary",
            "--since",
            "100000d",
          ]);
          previousMetricCounts[variant] = metrics.successful_requests;
        }
        clients.push(await worker(binary, home));
      }
      for (const [budgetIndex, budget] of budgets.entries()) {
        for (let round = -3; round < repeats; round++) {
          for (const [queryIndex, query] of queries.entries()) {
            // Pair each request and reverse order, avoiding a whole-before then
            // whole-after run that conflates changing machine load with variants.
            const order = binaries.map((_, index) => index);
            if ((round + queryIndex) % 2 !== 0) order.reverse();
            const signatures: string[] = [];
            for (const variant of order) {
              const sample = samples[variant][budgetIndex];
              const started = performance.now();
              const result = await clients[variant].request({
                op: "context",
                query,
                mode: "lexical",
                limit: 8,
                scope,
                max_chars: budget,
                evidence_per_memory: 2,
                structured: true,
                observe,
              });
              const duration = performance.now() - started;
              const packet = (observe ? result.packet : result) as Packet;
              if (observe) {
                check(
                  typeof result.request_id === "string" &&
                    /^[a-f0-9]{32}$/.test(result.request_id) &&
                    !observedIds[variant].has(result.request_id),
                  "Context observation was not recorded with a unique request ID",
                );
                observedIds[variant].add(result.request_id);
              }
              check(packet.used_chars <= budget, "Packet budget exceeded");
              check(
                packet.memories.length + packet.references.length <= 8,
                "Packet slot limit exceeded",
              );
              if (queryIndex === 3) {
                check(
                  packet.references.length === 0 &&
                    packet.memories.length === 0,
                  "Out-of-corpus request returned unrelated context",
                );
              } else {
                check(
                  packet.references.length > 0,
                  "Known synthetic query lost all evidence",
                );
              }
              for (const ref of packet.references) {
                check(ref.scope === scope, "Reference scope mismatch");
                const citations = ref.source_claim?.citations ??
                  ref.topic?.statements.flatMap((block) => block.citations) ??
                  [];
                if (citations.length === 0) {
                  // Retained snapshots contain the original temporary corpus
                  // paths. Validate a strict synthetic suffix without opening it.
                  const suffix = new RegExp(
                    "/seed-" + size + "/corpus/(reference-[0-9]{5}\\.txt)$",
                  ).exec(ref.source_uri);
                  const original = suffix
                    ? originals.get(suffix[1])
                    : undefined;
                  check(original !== undefined, "Unknown synthetic reference");
                  check(
                    decode.decode(
                      encode.encode(original).slice(
                        ref.start_byte,
                        ref.end_byte,
                      ),
                    ) === ref.content,
                    "Raw reference citation mismatch",
                  );
                }
                for (const citation of citations) {
                  const original = quoted.get(citation.source_key);
                  check(
                    original !== undefined,
                    "Unknown synthetic source citation",
                  );
                  check(
                    decode.decode(
                      encode.encode(original).slice(
                        citation.start_byte,
                        citation.end_byte,
                      ),
                    ) === citation.quote,
                    "Source citation mismatch",
                  );
                  if (round >= 0) sample.citationChecks++;
                }
              }
              // The packet has no per-request timestamp or metric ID. Exclude
              // only the observation wrapper's request_id; include scores,
              // ordering, evidence IDs, quotes and metadata in equality.
              const signature = JSON.stringify(packet);
              signatures[variant] = signature;
              if (sample.signatures[queryIndex]) {
                check(
                  sample.signatures[queryIndex] === signature,
                  "Repeated synthetic query changed its packet",
                );
              } else {
                sample.signatures[queryIndex] = signature;
              }
              if (round >= 0) {
                sample.elapsed.push(duration);
                sample.queryElapsed[queryIndex].push(duration);
                sample.used.push(packet.used_chars);
                sample.counts.push(packet.references.length);
              }
            }
            if (binaries.length === 2) {
              const matches = signatures[0] === signatures[1];
              if (!matches) {
                parityDifferences.add(budget + ":" + queryLabels[queryIndex]);
              }
              if (requireParity) {
                check(
                  matches,
                  "Structured packet parity failed: " + budget + ":" +
                    queryLabels[queryIndex],
                );
              }
              if (round >= 0) parityChecks++;
            }
          }
        }
      }
    } finally {
      const closed = await Promise.allSettled(
        clients.map((client) => client.close()),
      );
      check(
        closed.every((item) => item.status === "fulfilled"),
        "One or more benchmark workers failed to close",
      );
    }
    const variants = [];
    for (const [variant, binary] of binaries.entries()) {
      const health = await cli(binary, homes[variant], ["health"]);
      check(
        health.ok === true && [11, 12, 13].includes(health.schema_version) &&
          health.documents === size + 4,
        "Benchmark database unhealthy or wrong corpus size",
      );
      if (observe) {
        const metrics = await cli(binary, homes[variant], [
          "metrics",
          "summary",
          "--since",
          "100000d",
        ]);
        check(
          metrics.successful_requests - previousMetricCounts[variant] ===
              (repeats + 3) * queries.length * budgets.length &&
            observedIds[variant].size ===
              (repeats + 3) * queries.length * budgets.length,
          "Observed context requests were not retained after worker shutdown",
        );
      }
      variants.push({
        variant: variant === 0 ? "before" : "after",
        binary_sha256: hashes[variant],
        health_ok: health.ok,
        schema_version: health.schema_version,
        documents: health.documents,
        context_observations_recorded: observedIds[variant].size,
        measurements: budgets.map((budget, index) => {
          const sample = samples[variant][index];
          return {
            max_chars: budget,
            requests: sample.elapsed.length,
            warm_stdio_roundtrip_ms: {
              p50: percentile(sample.elapsed, 0.5),
              p95: percentile(sample.elapsed, 0.95),
              p99: percentile(sample.elapsed, 0.99),
            },
            packet_chars: {
              median: percentile(sample.used, 0.5),
              maximum: Math.max(...sample.used),
            },
            references: {
              median: percentile(sample.counts, 0.5),
              maximum: Math.max(...sample.counts),
            },
            exact_source_citations_checked: sample.citationChecks,
            per_query: sample.queryElapsed.map((values, queryIndex) => ({
              label: queryLabels[queryIndex],
              requests: values.length,
              p50_ms: percentile(values, 0.5),
              p95_ms: percentile(values, 0.95),
            })),
          };
        }),
      });
    }
    check(
      await sha(snapshot) === snapshotHash,
      "Seed snapshot changed during benchmark",
    );
    cases.push({
      corpus_reference_target: size,
      raw_reference_documents: rawDocuments,
      reviewed_claims: corpusClaims + 3,
      reviewed_topics: 1,
      fixture_snapshot_sha256: snapshotHash,
      packet_parity: {
        required: requireParity,
        paired_measured_checks: parityChecks,
        differences: [...parityDifferences],
        equal: binaries.length === 2 ? parityDifferences.size === 0 : null,
      },
      variants,
    });
    console.log(
      JSON.stringify({
        finished_corpus: size,
        requests_per_variant: repeats * queries.length * budgets.length,
      }),
    );
  }
  for (const [index, binary] of binaries.entries()) {
    check(
      await sha(binary) === hashes[index],
      "Binary changed during benchmark",
    );
  }
  await Deno.writeTextFile(
    output,
    JSON.stringify(
      {
        schema_version: 1,
        generated_at: new Date().toISOString(),
        status: "passed",
        retrieval_mode: "lexical",
        transport:
          "warm private stdio; includes JSON serialization and local IPC",
        iterations_per_query: repeats,
        queries_per_budget: queries.length,
        budgets,
        retained_seed_directory: seedDirectory ?? null,
        comparison_order:
          "Paired queries; before/after order alternates for each query and round",
        packet_parity_required: requireParity,
        context_observation_enabled: observe,
        packet_parity_exclusions: observe ? ["observation.request_id"] : [],
        before_sha256: hashes[0],
        after_sha256: hashes[1],
        provider_model_calls: 0,
        source_model:
          "No embedding query is executed; hash provider satisfies worker startup only",
        limitations: [
          "Deterministic synthetic corpora, not a production workload or answer-quality benchmark.",
          "Binary build profiles must be interpreted by the caller; debug timings do not prove release latency or speed improvements.",
          "Startup, seeding, database migration, backups and model loading are outside measured intervals.",
          observe
            ? "Timings include context observation and its durable local metric write; only observation.request_id is excluded from packet equality."
            : "Context observation is disabled; timings exclude its durable local metric write.",
          "Before/after calls alternate on the same machine; shared-machine variation still prevents treating small timing differences as established gains.",
          "Pending unembedded fixture documents are expected because all retrieval is lexical.",
        ],
        cases,
      },
      null,
      2,
    ) + "\n",
    { mode: 0o600, createNew: true },
  );
  console.log(
    JSON.stringify({
      status: "passed",
      output,
      before_sha256: hashes[0],
      after_sha256: hashes[1] ?? null,
      comparison: binaries.length === 2,
    }),
  );
} finally {
  await Deno.remove(work, { recursive: true });
}

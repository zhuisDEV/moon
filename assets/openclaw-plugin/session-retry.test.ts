import assert from "node:assert/strict";
import { __moonTest } from "./index.js";

async function optionalEnv(variable: string) {
  const permission = await Deno.permissions.query({ name: "env", variable });
  return permission.state === "granted" ? Deno.env.get(variable) : undefined;
}

const binary = await optionalEnv("MOON_TEST_BINARY");
const fixtureHome = await optionalEnv("MOON_TEST_HOME");

Deno.test({
  name:
    "real SQLite scheduler spends one retry, retains unsupported evidence and processes fresh records",
  ignore: !binary || !fixtureHome,
  fn: async () => {
    assert(binary && fixtureHome);
    const home = `${fixtureHome}/session-retry-${crypto.randomUUID()}`;
    const prefix = [
      "--home",
      home,
      "--database",
      `${home}/state/moon.sqlite`,
      "--dimensions",
      "64",
      "--json",
    ];
    const execute = async (argv: string[], options: { input?: string }) => {
      assert.equal(argv[0], binary);
      const child = new Deno.Command(binary, {
        args: argv.slice(1),
        stdin: "piped",
        stdout: "piped",
        stderr: "piped",
      }).spawn();
      const writer = child.stdin.getWriter();
      await writer.write(new TextEncoder().encode(options.input ?? ""));
      await writer.close();
      const result = await child.output();
      const stdout = new TextDecoder().decode(result.stdout);
      assert.equal(result.code, 0, new TextDecoder().decode(result.stderr));
      return { code: result.code, stdout, stderr: "" };
    };
    const cli = async (...args: string[]) =>
      JSON.parse((await execute([binary, ...prefix, ...args], {})).stdout);
    await cli("config", "init");
    const day = 86_400_000;
    const first = Date.parse("2026-09-14T00:00:00Z");
    const quotes = {
      weak: "The kiln currently holds twelve ceramic bowls.",
      good: "Magenta ribbons identify the rehearsal costumes.",
      later: "Juniper hedges surround the orchard gate.",
    };
    type Id = keyof typeof quotes;
    const record = (id: Id, time: number) =>
      cli(
        "record",
        "--session-id",
        id,
        "--scope",
        "global",
        "--content",
        quotes[id],
        "--completed-at-ms",
        String(time),
      );
    await record("weak", first - day);
    await record("good", first - day + 1);

    const calls: Id[][] = [];
    const errors: string[] = [];
    const api = {
      resolvePath: (path: string) => path,
      pluginConfig: {
        moonPath: binary,
        moonHome: home,
        dimensions: 64,
        embeddingEnabled: false,
      },
      runtime: {
        system: {
          async runCommandWithTimeout(
            argv: string[],
            options: { input?: string },
          ) {
            const result = await execute(argv, options);
            // Enable L2 for this harness without writing an owner config file.
            if (argv.includes("config") && argv.includes("show")) {
              const config = JSON.parse(result.stdout);
              config.learning.l2.enabled = true;
              config.learning.l2.fallback_enabled = false;
              result.stdout = JSON.stringify(config);
            }
            return result;
          },
        },
        agent: {
          runEmbeddedAgent({ prompt }: { prompt: string }) {
            const marker =
              "Original evidence (JSON; selected=true marks the new input): ";
            assert(prompt.includes(marker));
            const evidence: Array<{ session_id: Id; selected: boolean }> = JSON
              .parse(prompt.slice(prompt.lastIndexOf(marker) + marker.length));
            const selected = evidence.filter((row) => row.selected).map((row) =>
              row.session_id
            );
            calls.push(selected);
            const actions = selected.map((id) => ({
              action: "create",
              canonical_key: `fixture:${id}`,
              change_set: id,
              kind: "fact",
              content: id === "weak"
                ? "The kiln currently holds 491 ceramic bowls."
                : quotes[id],
              importance: 0.9,
              confidence: 0.99,
              evidence: [{ session_id: id, quote: quotes[id] }],
            }));
            return { payloads: [{ text: JSON.stringify({ actions }) }] };
          },
        },
      },
      logger: { error: (message: string) => errors.push(message) },
    };
    const original = __moonTest.createLearningScheduler(api);
    const restarted = __moonTest.createLearningScheduler(api);
    try {
      assert.deepEqual(await original.tick(first), {
        status: "empty",
        batches: 1,
      });
      assert.deepEqual(await original.tick(first), {
        status: "empty",
        batches: 0,
      });
      let state = await cli("learning", "status");
      assert.deepEqual([
        state.processed_evidence,
        state.retry_pending_evidence,
        state.parked_evidence,
      ], [1, 1, 0]);
      assert.deepEqual(await original.tick(first + day), {
        status: "empty",
        batches: 1,
      });
      await original.stop();
      assert.deepEqual(await restarted.tick(first + 2 * day), {
        status: "empty",
        batches: 0,
      });
      assert.deepEqual(calls, [["weak", "good"], ["weak"]]);
      await record("later", first + 2 * day);
      assert.deepEqual(await restarted.tick(first + 3 * day), {
        status: "empty",
        batches: 1,
      });
      assert.deepEqual(calls, [["weak", "good"], ["weak"], ["later"]]);
      state = await cli("learning", "status");
      assert.deepEqual([
        state.processed_evidence,
        state.pending_evidence,
        state.parked_evidence,
      ], [2, 1, 1]);
      assert.equal((await cli("health")).active_memories, 2);
      assert.deepEqual(errors, []);
    } finally {
      await original.stop();
      await restarted.stop();
    }
  },
});

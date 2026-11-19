import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { zstdCompressSync } from "node:zlib";
import plugin, { __moonTest } from "./index.js";

async function fixtureSetting(name: string) {
  const permission = await Deno.permissions.query({
    name: "env",
    variable: name,
  });
  return permission.state === "granted" ? Deno.env.get(name) : undefined;
}

const binary = await fixtureSetting("MOON_TEST_BINARY");
const fixtureRoot = await fixtureSetting("MOON_PROFILE_TEST_ROOT");
if (
  await fixtureSetting("MOON_REQUIRE_REAL_BINARY") === "1" &&
  (!binary || !fixtureRoot)
) {
  throw new Error(
    "Profile learning integration requires an isolated fixture directory and Moon binary",
  );
}

type Handler = (
  event: Record<string, unknown>,
  ctx: Record<string, unknown>,
) => Promise<unknown>;

function committedTurn(
  sessionId: string,
  sessionKey: string,
  id: string,
  text: string,
) {
  const admission = {
    agentId: "main",
    sessionId,
    sessionKey,
    storePath: "fixture",
    generation: "1",
    entryId: id,
    logicalTurnId: id,
    idempotencyKey: id,
    role: "user",
    rawSeq: 1,
  };
  return {
    advancementKey: "advance-" + id,
    admission,
    terminal: { ...admission, entryId: "assistant-" + id, rawSeq: 2 },
    sessionId,
    sessionKey,
    messages: [{ role: "user", content: text, timestamp: 100 }, {
      role: "assistant",
      content: "Synthetic generated reply.",
      timestamp: 200,
    }],
  };
}

Deno.test({
  name:
    "registered profiles fence private history and exact Discord room admissions can learn with native memory disabled",
  ignore: !binary || !fixtureRoot,
  fn: async () => {
    const directory = await Deno.makeTempDir({ dir: fixtureRoot });
    const profile = "profile-fixture", master = "discord:123456789012345678";
    const guest = "profile-guest";
    const home = directory + "/moon", host = directory + "/openclaw";
    await Deno.mkdir(home);
    await Deno.mkdir(host + "/state", { recursive: true });
    await Deno.mkdir(host + "/agents/main/agent", { recursive: true });
    const profiles = new DatabaseSync(host + "/state/openclaw.sqlite");
    const transcript = new DatabaseSync(
      host + "/agents/main/agent/openclaw-agent.sqlite",
    );
    try {
      // The native input comes from the same active transcript and identity
      // projections the Gateway maintains. No adapter identity check is stubbed.
      profiles.exec(
        `CREATE TABLE user_profiles(id TEXT PRIMARY KEY,merged_into TEXT);
        CREATE TABLE user_profile_identities(provider TEXT,subject TEXT,profile_id TEXT);
        INSERT INTO user_profiles VALUES('profile-fixture',NULL),('profile-guest',NULL),('gateway-owner',NULL);`,
      );
      profiles.prepare(
        "INSERT INTO user_profile_identities VALUES('channel.identity',?,?)",
      ).run(
        JSON.stringify(["discord", "default", master.slice("discord:".length)]),
        profile,
      );
      transcript.exec(
        `CREATE TABLE session_nodes(session_key,current_session_id,entry_valid,entry_json);
        CREATE TABLE transcript_event_identities(session_id,seq,message_idempotency_key,event_type);
        CREATE TABLE transcript_events(session_id,seq,event_json,event_zstd,event_utf8_bytes,message_role);
        CREATE TABLE session_transcript_active_events(session_id,event_seq);`,
      );
      await Deno.writeTextFile(
        home + "/moon.toml",
        `[master]\nprofile_id = "${profile}"\nmemory_principal = "${master}"\n[learning.l1]\nenabled = false\n`,
      );
      const hooks = new Map<string, Handler>();
      let factory:
        | (() => ReturnType<typeof __moonTest.createMoonContextEngine>)
        | undefined;
      const errors: string[] = [];
      type StorageConfig = {
        agents?: { entries: Record<string, { agentDir: string }> };
        database?: {
          engine?: string;
          families?: Record<string, { engine: string }>;
        };
        session?: { store: string };
        plugins?: { slots: { memory: string } };
      };
      let current: StorageConfig = { plugins: { slots: { memory: "none" } } };
      const identityRuntime = {
        config: { current: () => current },
        agent: {
          resolveAgentDir: (cfg: StorageConfig, id: string) =>
            cfg.agents?.entries[id]?.agentDir ??
              join(host, "agents", id, "agent"),
        },
        state: { resolveStateDir: () => host },
      };
      plugin.register({
        config: {
          plugins: {
            slots: { contextEngine: "moon" },
            entries: { moon: { hooks: { allowConversationAccess: true } } },
          },
        },
        pluginConfig: {
          moonHome: home,
          moonPath: binary,
          mode: "lexical",
          embeddingEnabled: false,
          failOpen: false,
        },
        resolvePath: (value: string) => value,
        logger: { error: (message: string) => errors.push(message) },
        runtime: {
          ...identityRuntime,
          system: {
            async runCommandWithTimeout(
              argv: string[],
              options: { input?: string },
            ) {
              assert.equal(argv[0], binary);
              assert.equal(argv[argv.indexOf("--home") + 1], home);
              const child = new Deno.Command(argv[0], {
                args: argv.slice(1),
                stdin: "piped",
                stdout: "piped",
                stderr: "piped",
              }).spawn();
              const writer = child.stdin.getWriter();
              await writer.write(new TextEncoder().encode(options.input ?? ""));
              await writer.close();
              const result = await child.output();
              return {
                code: result.code,
                stdout: new TextDecoder().decode(result.stdout),
                stderr: new TextDecoder().decode(result.stderr),
              };
            },
          },
        },
        on: (name: string, fn: Handler) => hooks.set(name, fn),
        registerContextEngine: (_name: string, fn: typeof factory) => {
          factory = fn;
        },
        registerService() {},
        registerCompactionProvider() {},
      });
      assert(factory);
      const engine = factory();
      const prepare = hooks.get("before_prompt_build");
      assert(prepare);
      const cases = [
        {
          client: "openclaw-tui",
          user: profile,
          text: "The synthetic terminal label is fern.",
          owner: master,
        },
        {
          client: "openclaw-control-ui",
          user: profile,
          text: "The synthetic website label is lilac.",
          owner: master,
        },
        {
          client: "openclaw-control-ui",
          user: guest,
          text: "The guest's synthetic label is coral.",
          owner: "openclaw:" + guest,
        },
        {
          client: "openclaw-tui",
          user: "gateway-owner",
          text: "The shared synthetic label is grey.",
          owner: null,
        },
      ];
      for (const [index, item] of cases.entries()) {
        const sessionId = "test-session-" + index,
          sessionKey = "agent:main:" + sessionId;
        const id = "user-entry-" + index;
        const ctx = {
          agentId: "main",
          sessionId,
          sessionKey,
          runId: "test-run-" + index,
          trigger: "user",
          inputProvenance: { kind: "external_user" },
          memoryAudience: {
            kind: "conversation",
            agentId: "main",
            sessionId,
            sessionKey,
          },
          assertMemoryAudienceCurrent() {},
        };
        const metadata = {
          senderId: item.user,
          senderIdentity: { type: "profile", id: item.user },
          requesterOrigin: {
            sessionKey,
            messageProvider: "webchat",
            chatType: "direct",
            senderId: item.user,
          },
          transport: { clients: [{ id: item.client }] },
        };
        const native = JSON.stringify({
          message: { role: "user", content: item.text, __openclaw: metadata },
        });
        // OpenClaw stores some native sessions without chatType and some
        // user events as bounded Zstd rather than plaintext JSON.
        transcript.prepare("INSERT INTO session_nodes VALUES(?,?,1,?)").run(
          sessionKey,
          sessionId,
          index % 2 ? '{"chatType":"direct"}' : "{}",
        );
        transcript.prepare(
          "INSERT INTO transcript_event_identities VALUES(?,1,?,'message')",
        ).run(sessionId, id);
        transcript.prepare(
          "INSERT INTO transcript_events VALUES(?,1,?,?,?,'user')",
        ).run(
          sessionId,
          index % 2 ? native : null,
          index % 2 ? null : zstdCompressSync(Buffer.from(native)),
          Buffer.byteLength(native),
        );
        transcript.prepare(
          "INSERT INTO session_transcript_active_events VALUES(?,1)",
        ).run(sessionId);
        await prepare({
          currentUserMessage: item.text,
          currentUserMessageId: id,
        }, ctx);
        const key = __moonTest.admissionKey("main", sessionId, sessionKey, id);
        assert(key);
        const committed = committedTurn(sessionId, sessionKey, id, item.text);
        const database = new DatabaseSync(home + "/state/moon.sqlite", {
          readOnly: true,
        });
        try {
          const binding = database.prepare(
            "SELECT sender, owners_json, audience_json FROM privacy_admissions WHERE admission_key=?",
          ).get(key);
          if (item.owner) {
            assert.equal(
              binding?.sender,
              item.owner,
              "admitted sender for " + item.client + ": " + errors.join(" | "),
            );
            assert.deepEqual(JSON.parse(String(binding.owners_json)), [
              item.owner,
            ]);
            assert.deepEqual(JSON.parse(String(binding.audience_json)), [
              item.owner,
            ]);
          } else {
            assert.equal(binding, undefined);
          }
          assert.deepEqual(await engine.commitTurn(committed), {
            status: "committed",
          });
          assert.deepEqual(await engine.commitTurn(committed), {
            status: item.owner ? "duplicate" : "committed",
          });
          const evidence = database.prepare(
            `SELECT c.content, d.scope, speaker.sender, a.kind, a.readers_json
            FROM evidence_admissions b JOIN evidence_sessions e ON e.id=b.evidence_id
            JOIN documents d ON d.id=e.document_id JOIN chunks c ON c.document_id=d.id
            JOIN speaker_memory_scopes speaker ON speaker.scope=d.scope
            JOIN privacy_audiences a ON a.scope=speaker.audience_scope WHERE b.admission_key=?`,
          ).all(key);
          if (!item.owner) {
            assert.deepEqual(evidence, []);
            continue;
          }
          assert.equal(evidence.length, 1);
          assert.equal(evidence[0].content, "User:\n" + item.text);
          assert.equal(evidence[0].sender, item.owner);
          assert.equal(evidence[0].kind, "private");
          assert.deepEqual(JSON.parse(String(evidence[0].readers_json)), [
            item.owner,
          ]);
          const counts = database.prepare(
            "SELECT count(*) AS scopes FROM speaker_memory_scopes WHERE sender=?",
          ).get(item.owner);
          assert.equal(counts?.scopes, 1);
        } finally {
          database.close();
        }
      }
      // Replace Discord's external /users/<id> read only. The registered hook
      // still resolves profiles in host SQLite and runs every real Moon CLI command.
      const masterId = master.slice("discord:".length),
        otherId = "789456123012345678";
      const services = __moonTest.promptRecallServices;
      const verifyHuman = services.human;
      const verifyAccess = services.access;
      services.human = (
        _api: unknown,
        target: { accountId: string; senderId: string },
      ) =>
        Promise.resolve(
          target.accountId === "default" &&
            [masterId, otherId].includes(target.senderId),
        );
      const database = new DatabaseSync(home + "/state/moon.sqlite", {
        readOnly: true,
      });
      try {
        let turn = 0;
        const dm = async (senderId: string, text: string, save = false) => {
          const id = "dm-entry-" + ++turn, sessionId = "dm-session-" + senderId;
          const sessionKey = "agent:main:discord:direct:" + senderId;
          const packet = await prepare({
            currentUserMessageId: id,
            currentUserMessage: text,
          }, {
            channel: "discord",
            accountId: "default",
            trigger: "user",
            senderId,
            chatId: senderId,
            agentId: "main",
            sessionId,
            sessionKey,
            runId: "dm-run-" + turn,
            inputProvenance: { kind: "external_user" },
            memoryAudience: { kind: "owner-private", agentId: "main" },
            assertMemoryAudienceCurrent() {},
          }) as { prependContext?: string } | undefined;
          const key = __moonTest.admissionKey(
            "main",
            sessionId,
            sessionKey,
            id,
          );
          const row = database.prepare(
            "SELECT sender FROM privacy_admissions WHERE admission_key=?",
          ).get(key);
          assert(
            row,
            "DM must succeed with a verified private grant: " +
              errors.join(" | "),
          );
          if (save) {
            assert.deepEqual(
              await engine.commitTurn(
                committedTurn(sessionId, sessionKey, id, text),
              ),
              { status: "committed" },
            );
          }
          return {
            text: packet?.prependContext ?? "",
            owner: String(row.sender),
          };
        };
        const history = "Show our chat history",
          ordinaryText = "The other Discord user's synthetic label is apricot.";
        assert.equal(
          database.prepare(
            "SELECT principal FROM master_discord_principals WHERE principal=?",
          ).get(master)?.principal,
          master,
        );
        const linked = await dm(masterId, history);
        assert.equal(linked.owner, master);
        assert(
          linked.text.includes(cases[0].text) &&
            linked.text.includes(cases[1].text),
        );
        assert(!linked.text.includes(cases[2].text));

        // The anchor is linked to `profile`, never to `guest`. Profile changes
        // or removal must never free this raw ID to open cross-channel memory.
        for (
          const settings of [
            `[master]\nprofile_id = "${guest}"\nmemory_principal = "${master}"\n`,
            "",
          ]
        ) {
          await Deno.writeTextFile(
            home + "/moon.toml",
            settings + "[learning.l1]\nenabled = false\n",
          );
          const denied = await dm(masterId, history);
          assert.match(denied.owner, /^peer:[a-f0-9]{64}$/);
          assert(
            !denied.text.includes(cases[0].text) &&
              !denied.text.includes(cases[1].text),
          );
        }
        const ordinary = await dm(otherId, ordinaryText, true);
        assert.equal(ordinary.owner, "discord:" + otherId);
        const otherHistory = await dm(otherId, history);
        assert(otherHistory.text.includes(ordinaryText));
        assert(
          !otherHistory.text.includes(cases[0].text) &&
            !otherHistory.text.includes(cases[1].text),
        );

        // OpenClaw omits its memory audience when native memory is disabled.
        // A live Discord prompt can bind only its exact active user receipt.
        const chatId = "345678901234567890";
        const messageId = "456789012345678901";
        const sessionKey = `agent:main:discord:channel:${chatId}`;
        const sessionId = "native-memory-disabled";
        const sourceId = (postId: string) =>
          "channel-user:v1:" + createHash("sha256").update(
            JSON.stringify([
              "discord",
              "default",
              `channel:${chatId}`,
              postId,
            ]),
          ).digest("hex");
        const id = sourceId(messageId);
        const text =
          "Show our chat history for the synthetic apricot workflow.";
        const native = JSON.stringify({
          message: {
            content: text,
            __openclaw: {
              senderId: otherId,
              senderIdentity: {
                type: "observation",
                id: otherId,
                senderKind: "unknown",
                pluginId: "discord",
                accountId: "default",
              },
              requesterOrigin: {
                accountId: "default",
                chatType: "channel",
                conversationId: chatId,
                messageProvider: "discord",
                senderId: otherId,
                sessionKey,
              },
              transport: { channel: "discord", messageId },
              upstreamUserText: text + "\nCurrent time: 09:30 UTC",
            },
          },
        });
        transcript.prepare("INSERT INTO session_nodes VALUES(?,?,1,?)").run(
          sessionKey,
          sessionId,
          '{"chatType":"channel"}',
        );
        transcript.prepare(
          "INSERT INTO transcript_event_identities VALUES(?,1,?,'message')",
        ).run(sessionId, id);
        transcript.prepare(
          "INSERT INTO transcript_events VALUES(?,1,NULL,?,?,'user')",
        ).run(
          sessionId,
          zstdCompressSync(Buffer.from(native)),
          Buffer.byteLength(native),
        );
        const ctx = {
          agentId: "main",
          sessionId,
          sessionKey,
          runId: "discord-room-run",
          channel: "discord",
          accountId: "default",
          senderId: otherId,
          chatId,
          trigger: "user",
          hookInvocation: { assertActive() {} },
        };
        const owner = `discord:${otherId}`;
        const bot = "bot:discord:567890123456789012";
        const laterReader = "discord:456789012345678923";
        const originalHash = createHash("sha256").update(text).digest("hex");
        let inspections = 0;
        services.access = (_api, target) => {
          assert.equal(target.learningReceipt.messageId, messageId);
          assert.equal(target.channelId, chatId);
          assert.equal(target.senderId, otherId);
          inspections++;
          return Promise.resolve(
            target.learningReceipt.textHash === originalHash
              ? {
                readers: [owner, laterReader, bot],
                owners: [owner, laterReader],
              }
              : null,
          );
        };
        const request = { currentUserMessage: text, currentUserMessageId: id };
        const key = __moonTest.admissionKey("main", sessionId, sessionKey, id);
        const binding = () =>
          database.prepare(
            "SELECT sender,owners_json,audience_json FROM privacy_admissions WHERE admission_key=?",
          ).get(key);
        await prepare(request, ctx);
        assert.equal(
          inspections,
          0,
          "an inactive receipt fails before Discord I/O",
        );
        assert.equal(binding(), undefined);
        transcript.prepare(
          "INSERT INTO session_transcript_active_events VALUES(?,1)",
        ).run(sessionId);
        await prepare(request, {
          ...ctx,
          memoryAudience: { agentId: "other" },
        });
        await prepare(request, { ...ctx, senderId: masterId });
        assert.equal(
          inspections,
          0,
          "an invalid host grant or sender cannot fall back",
        );
        assert.equal(binding(), undefined);
        // A queued batch uses the last Discord ID with combined text. The
        // active uncompressed event must not admit any preceding message.
        const stored = JSON.parse(native);
        stored.message.content = "An earlier Discord post.\n" + text;
        transcript.prepare(
          "UPDATE transcript_events SET event_json=?,event_zstd=NULL WHERE session_id=?",
        ).run(JSON.stringify(stored), sessionId);
        await prepare(request, ctx);
        assert.equal(binding(), undefined);
        stored.message.content = [{
          type: "text",
          text,
          summary: "An extra projected passage.",
        }];
        transcript.prepare(
          "UPDATE transcript_events SET event_json=? WHERE session_id=?",
        ).run(JSON.stringify(stored), sessionId);
        await prepare(request, ctx);
        assert.equal(binding(), undefined);
        stored.message.content = [{ type: "text", text }];
        transcript.prepare(
          "UPDATE transcript_events SET event_json=? WHERE session_id=?",
        ).run(JSON.stringify(stored), sessionId);
        const host = { config: {}, runtime: identityRuntime };
        assert.equal(
          (await __moonTest.readHostAdmission(host, ctx, id, "channel"))
            ?.sourceTextHash,
          originalHash,
          "a single host text block has the same exact source as a string",
        );
        transcript.prepare(
          "UPDATE transcript_events SET event_json=NULL,event_zstd=? WHERE session_id=?",
        ).run(zstdCompressSync(Buffer.from(native)), sessionId);
        const result = await prepare(request, ctx) as
          | { prependContext?: string }
          | undefined;
        assert(!result?.prependContext?.includes(ordinaryText));
        assert(!result?.prependContext?.includes("could not verify"));
        const saved = binding();
        assert.equal(saved?.sender, owner);
        assert.deepEqual(JSON.parse(String(saved.owners_json)), [owner]);
        assert.deepEqual(
          JSON.parse(String(saved.audience_json)),
          [bot, laterReader, owner].sort(),
        );
        const turnInput = committedTurn(sessionId, sessionKey, id, text);
        assert.deepEqual(await engine.commitTurn(turnInput), {
          status: "committed",
        });
        assert.deepEqual(await engine.commitTurn(turnInput), {
          status: "duplicate",
        });
        const evidence = database.prepare(`
          SELECT d.body,p.kind FROM evidence_admissions a
          JOIN evidence_sessions e ON e.id=a.evidence_id JOIN documents d ON d.id=e.document_id
          JOIN speaker_memory_scopes s ON s.scope=d.scope
          JOIN privacy_audiences p ON p.scope=s.audience_scope WHERE a.admission_key=?
        `).all(key);
        assert.deepEqual(evidence.map((entry) => [entry.body, entry.kind]), [
          [`User:\n${text}`, "private"],
        ]);
        // Older versions recorded multiple owners under the same stable key.
        // A later sender-only prompt cannot widen or replace that binding.
        const legacyPostId = "456789012345678902",
          legacyId = sourceId(legacyPostId);
        const legacyRequest = { ...request, currentUserMessageId: legacyId };
        const legacyTurn = committedTurn(sessionId, sessionKey, legacyId, text);
        const room = {
          readers: [owner, laterReader, bot],
          owners: [owner, laterReader],
        };
        services.access = () => Promise.resolve(room);
        await prepare(legacyRequest, {
          ...ctx,
          runId: "legacy-room-run",
          memoryAudience: {
            kind: "conversation",
            agentId: "main",
            sessionKey,
            sessionId,
          },
          assertMemoryAudienceCurrent() {},
        });
        assert.deepEqual(await engine.commitTurn(legacyTurn), {
          status: "committed",
        });
        stored.message.content = text;
        stored.message.__openclaw.transport.messageId = legacyPostId;
        transcript.prepare(
          "INSERT INTO transcript_event_identities VALUES(?,2,?,'message')",
        ).run(sessionId, legacyId);
        transcript.prepare(
          "INSERT INTO transcript_events VALUES(?,2,?,NULL,0,'user')",
        ).run(sessionId, JSON.stringify(stored));
        transcript.prepare(
          "INSERT INTO session_transcript_active_events VALUES(?,2)",
        ).run(sessionId);
        await prepare(legacyRequest, { ...ctx, runId: "legacy-room-retry" });
        assert.deepEqual(await engine.commitTurn(legacyTurn), {
          status: "duplicate",
        });
        const legacyKey = __moonTest.admissionKey(
          "main",
          sessionId,
          sessionKey,
          legacyId,
        );
        const preserved = database.prepare(
          `SELECT owners_json FROM privacy_admissions
          JOIN evidence_admissions USING(admission_key) WHERE admission_key=?`,
        ).all(legacyKey);
        assert.deepEqual(
          preserved.map((r) => JSON.parse(String(r.owners_json))),
          [[laterReader, owner].sort()],
        );
      } finally {
        services.human = verifyHuman;
        services.access = verifyAccess;
        database.close();
      }
      assert.deepEqual(errors, []);
      // Old rows remain available on disk after a move. Changing the live
      // configuration must invalidate a retried admission and stale state links.
      const storageProbe = { config: {}, runtime: identityRuntime };
      const admission = () =>
        __moonTest.readHostAdmission(storageProbe, {
          agentId: "main",
          sessionId: "test-session-0",
          sessionKey: "agent:main:test-session-0",
        }, "user-entry-0");
      const links = () =>
        __moonTest.readMasterChannelSenders(
          storageProbe,
          { profile_id: profile },
          "discord",
          "default",
        );
      assert.equal((await admission())?.sender, profile);
      assert.deepEqual([...await links()], [master.slice("discord:".length)]);
      current = {
        agents: { entries: { main: { agentDir: directory + "/moved" } } },
      };
      assert.equal(await admission(), null);
      current = { session: { store: directory + "/moved/sessions.json" } };
      assert.equal(await admission(), null);
      current = { database: { families: { agent: { engine: "postgres" } } } };
      assert.equal(await admission(), null);
      assert.equal((await links()).size, 1);
      current = { database: { engine: "postgres" } };
      assert.equal(await admission(), null);
      assert.equal((await links()).size, 0);
    } finally {
      transcript.close();
      profiles.close();
      await Deno.remove(directory, { recursive: true });
    }
  },
});

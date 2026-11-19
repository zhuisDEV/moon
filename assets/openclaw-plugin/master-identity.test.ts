import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { __moonTest as moon } from "./index.js";

const profile = "p-test";
const principal = "discord:123456789012345678";
const sessionKey = "agent:main:example:direct:234";
const ctx = {
  runId: "run-1",
  agentId: "main",
  sessionId: "session-1",
  sessionKey,
  trigger: "user",
  channel: "example",
  accountId: "default",
  senderId: "234",
  inputProvenance: { kind: "external_user" },
  memoryAudience: {
    kind: "conversation",
    agentId: "main",
    sessionId: "session-1",
    sessionKey,
  },
  assertMemoryAudienceCurrent() {},
};
const event = { currentUserMessageId: "admission-1" };
const api = {
  config: {
    channels: { example: { dmPolicy: "allowlist", allowFrom: ["234"] } },
  },
};

function fixture() {
  const master = {
    profile_id: profile,
    memory_principal: principal,
    channels: [] as Array<
      { channel: string; account_id: string; sender_id: string; source: string }
    >,
  };
  const row = {
    identity: {
      type: "observation",
      senderKind: "human",
      id: "234",
      pluginId: "example",
      accountId: "default",
    },
    sender: "234",
    origin: {
      senderId: "234",
      messageProvider: "example",
      chatType: "direct",
      sessionKey,
      accountId: "default",
    },
    transport: { channel: "example" },
  };
  let linked: string | null = null;
  const writes: string[] = [];
  const checks = {
    admission: () => Promise.resolve(row),
    authority: () =>
      Promise.resolve({ senders: new Set<string>(), linkedProfile: linked }),
    enroll: (
      _api: unknown,
      _master: unknown,
      _ctx: unknown,
      source: string,
    ) => {
      writes.push(source);
      const first = master.channels[0];
      if (first && first.sender_id !== "234") return Promise.resolve(false);
      master.channels.splice(0, 1, {
        channel: "example",
        account_id: "default",
        sender_id: "234",
        source,
      });
      return Promise.resolve(true);
    },
    currentMaster: () => Promise.resolve(master),
  };
  return {
    master,
    row,
    checks,
    writes,
    link: (id: string | null) => {
      linked = id;
    },
  };
}

Deno.test("the first explicitly allowed human on a direct route enrols once and revocation fences an in-flight grant", async () => {
  const f = fixture();
  const grant = await moon.resolveMasterTurn(
    api,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.equal(grant?.sender, principal);
  assert.equal(grant?.privateRecall, false);
  assert.deepEqual(f.writes, ["allowlist"]);
  await grant!.recheck();
  const enlarged = {
    config: {
      channels: {
        example: { dmPolicy: "allowlist", allowFrom: ["234", "345"] },
      },
    },
  };
  assert.equal(
    (await moon.resolveMasterTurn(enlarged, f.master, event, ctx, f.checks))
      ?.sender,
    principal,
  );
  const duplicate = await moon.resolveMasterTurn(
    api,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.equal(duplicate?.sender, principal);
  assert.deepEqual(f.writes, ["allowlist"]);
  f.link("another-person");
  await assert.rejects(() => grant!.recheck());
  const other = await moon.resolveMasterTurn(
    api,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.match(other!.sender, /^peer:[0-9a-f]{64}$/);
  assert.deepEqual(other!.owners, [other!.sender]);
});

Deno.test("an existing allowlist binding observes live removal from the channel policy", async () => {
  const f = fixture();
  let config = api.config;
  const host = { ...api, runtime: { config: { current: () => config } } };
  const grant = await moon.resolveMasterTurn(
    host,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.equal(grant?.sender, principal);
  config = {
    channels: { example: { dmPolicy: "allowlist", allowFrom: ["345"] } },
  };
  await assert.rejects(() => grant!.recheck());
  const revoked = await moon.resolveMasterTurn(
    host,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.match(revoked!.sender, /^peer:/);
  assert.deepEqual(f.writes, ["allowlist"]);
});

Deno.test("first human enrolment captures only the first verified DM and respects conflicting profile links", async () => {
  const f = fixture();
  const master = Object.assign(f.master, { enroll_first_direct_sender: true });
  const open = {
    config: { channels: { example: { dmPolicy: "open", allowFrom: ["*"] } } },
  };
  const first = await moon.resolveMasterTurn(
    open,
    master,
    event,
    ctx,
    f.checks,
  );
  assert.equal(first?.sender, principal);
  assert.equal(first?.privateRecall, false);
  assert.deepEqual(f.writes, ["first-human"]);
  await first!.recheck();
  const other = { ...ctx, senderId: "345" };
  f.row.identity.id = f.row.sender = f.row.origin.senderId = "345";
  const second = await moon.resolveMasterTurn(
    open,
    master,
    event,
    other,
    f.checks,
  );
  assert.match(second!.sender, /^peer:/);
  assert.deepEqual(f.writes, ["first-human"]);
  const reserved = fixture();
  const g = await moon.resolveMasterTurn(
    open,
    { ...reserved.master, enroll_first_direct_sender: true },
    event,
    ctx,
    {
      ...reserved.checks,
      authority: () =>
        Promise.resolve({
          senders: new Set(["somebody-else"]),
          linkedProfile: null,
        }),
    },
  );
  assert.match(g!.sender, /^peer:/);
  assert.deepEqual(reserved.writes, []);
  reserved.link("other-profile");
  const otherProfile = await moon.resolveMasterTurn(
    open,
    { ...reserved.master, enroll_first_direct_sender: true },
    event,
    ctx,
    reserved.checks,
  );
  assert.match(otherProfile!.sender, /^peer:/);
  assert.deepEqual(reserved.writes, []);
});

Deno.test("an unlinked native account cannot silently reclaim a gateway-verified master route", async () => {
  const f = fixture();
  f.link(profile);
  const grant = await moon.resolveMasterTurn(
    api,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.equal(grant?.sender, principal);
  assert.deepEqual(f.writes, ["gateway"]);
  f.link(null);
  await assert.rejects(() => grant!.recheck());
  const later = await moon.resolveMasterTurn(
    api,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.match(later!.sender, /^peer:/);
  assert.deepEqual(f.writes, ["gateway"]);
});

Deno.test("a native DM's attributed requester must match the current link for its exact sender", async () => {
  const f = fixture();
  const attributed = (id: string | null) =>
    Object.assign(f.row.origin, { delegatedCreatorProfileId: id });
  const resolve = () =>
    moon.resolveMasterTurn(api, f.master, event, ctx, f.checks);
  f.link(profile);
  attributed(profile);
  const master = await resolve();
  assert.equal(master?.sender, principal);
  await master!.recheck();

  f.link("second-person");
  await assert.rejects(() => master!.recheck());
  assert.equal(await resolve(), null);
  attributed("second-person");
  assert.match((await resolve())!.sender, /^peer:/);

  f.link(null);
  assert.equal(await resolve(), null);
  attributed(null);
  assert.equal(await resolve(), null);
  f.link(profile);
  assert.equal(await resolve(), null);
  Reflect.deleteProperty(f.row.origin, "delegatedCreatorProfileId");
  assert.equal((await resolve())?.sender, principal);
  f.link(null);
  assert.match((await resolve())!.sender, /^peer:/);
  assert.deepEqual(f.writes, ["gateway"]);
});

Deno.test("an unclassified native sender requires an exact personal link, and a bot marker excludes it", async () => {
  const f = fixture();
  f.row.identity.senderKind = "unknown";
  f.link(profile);
  const grant = await moon.resolveMasterTurn(
    api,
    f.master,
    event,
    ctx,
    f.checks,
  );
  assert.equal(grant?.sender, principal);
  assert.deepEqual(f.writes, ["gateway"]);
  f.link("another-person");
  await assert.rejects(() => grant!.recheck());
  assert.equal(
    await moon.resolveMasterTurn(api, f.master, event, ctx, f.checks),
    null,
  );
  f.link(profile);
  f.row.identity.senderKind = "bot";
  assert.equal(
    await moon.resolveMasterTurn(api, f.master, event, ctx, f.checks),
    null,
  );
});

Deno.test("open rooms, wildcard/user lists, missing authority and nonhuman turns never enrol Master", async () => {
  for (
    const config of [{ dmPolicy: "open", allowFrom: ["234"] }, {
      dmPolicy: "allowlist",
      allowFrom: ["*"],
    }, { dmPolicy: "allowlist", allowFrom: ["234", "345"] }]
  ) {
    const f = fixture();
    const g = await moon.resolveMasterTurn(
      { config: { channels: { example: config } } },
      f.master,
      event,
      ctx,
      f.checks,
    );
    assert.match(g!.sender, /^peer:/);
    assert.deepEqual(f.writes, []);
  }
  for (
    const mutate of [(r: ReturnType<typeof fixture>["row"]) => {
      r.origin.chatType = "group";
    }, (r: ReturnType<typeof fixture>["row"]) => {
      r.identity.senderKind = "bot";
    }, (r: ReturnType<typeof fixture>["row"]) => {
      r.identity.senderKind = "unknown";
    }, (r: ReturnType<typeof fixture>["row"]) => {
      r.identity.id = "345";
    }, (r: ReturnType<typeof fixture>["row"]) => {
      r.identity.pluginId = "spoofed";
    }]
  ) {
    const f = fixture();
    mutate(f.row);
    assert.equal(
      await moon.resolveMasterTurn(api, f.master, event, ctx, f.checks),
      null,
    );
    assert.deepEqual(f.writes, []);
  }
});

Deno.test("TUI and website require the signed-in profile on the exact admitted human message", async () => {
  for (const surface of ["openclaw-tui", "openclaw-control-ui"]) {
    const f = fixture();
    const row = {
      identity: { type: "profile", id: profile },
      sender: profile,
      origin: {
        messageProvider: "webchat",
        chatType: "direct",
        sessionKey,
        senderId: profile,
      },
      transport: { clients: [{ id: surface }] },
    };
    const c = {
      ...ctx,
      channel: "webchat",
      senderId: profile,
      memoryAudience: { kind: "owner-private", agentId: "main" },
    };
    const checks = { ...f.checks, admission: () => Promise.resolve(row) };
    const grant = await moon.resolveMasterTurn(api, f.master, event, c, checks);
    assert.equal(grant?.sender, principal);
    assert.equal(grant?.privateRecall, false);
    row.identity.id = row.sender = row.origin.senderId = "gateway-owner";
    await assert.rejects(() => grant!.recheck());
    assert.equal(
      await moon.resolveMasterTurn(api, f.master, event, {
        ...c,
        senderId: "gateway-owner",
      }, checks),
      null,
    );
    row.identity.id = row.sender = row.origin.senderId = "another-person";
    const other = await moon.resolveMasterTurn(api, f.master, event, {
      ...c,
      senderId: "another-person",
    }, checks);
    assert.equal(other?.sender, "openclaw:another-person");
    assert.equal(other?.sourceSenderId, "another-person");
    assert.deepEqual(f.writes, []);
  }
});

Deno.test("Gateway profile links are exact per account and expire on unlink or profile retirement", () => {
  const db = new DatabaseSync(":memory:");
  try {
    db.exec(
      "CREATE TABLE user_profiles(id TEXT PRIMARY KEY,merged_into TEXT); CREATE TABLE user_profile_identities(provider TEXT,subject TEXT,profile_id TEXT);",
    );
    db.prepare("INSERT INTO user_profiles VALUES (?,NULL)").run(profile);
    const insert = db.prepare(
      "INSERT INTO user_profile_identities VALUES ('channel.identity',?,?)",
    );
    insert.run(JSON.stringify(["discord", "default", "one"]), profile);
    insert.run(JSON.stringify(["discord", "elsewhere", "two"]), profile);
    insert.run(JSON.stringify(["discord", "default", "three"]), "other");
    const master = { profile_id: profile },
      query = () =>
        moon.hostChannelAuthority(db, master, "discord", "default", "three");
    assert.deepEqual([...query()!.senders], ["one"]);
    assert.equal(query()!.linkedProfile, "other");
    db.prepare("DELETE FROM user_profile_identities WHERE profile_id = ?").run(
      profile,
    );
    assert.equal(query()!.senders.size, 0);
    db.prepare("UPDATE user_profiles SET merged_into='elsewhere' WHERE id=?")
      .run(profile);
    assert.equal(query(), null);
  } finally {
    db.close();
  }
});

Deno.test("host admission lookup excludes inactive, mismatched, assistant and stale session entries", () => {
  const db = new DatabaseSync(":memory:");
  try {
    db.exec(
      `CREATE TABLE session_nodes(session_key,current_session_id,entry_valid,entry_json);
      CREATE TABLE transcript_event_identities(session_id,seq,message_idempotency_key,event_type);
      CREATE TABLE transcript_events(session_id,seq,event_json,event_zstd,event_utf8_bytes,message_role);
      CREATE TABLE session_transcript_active_events(session_id,event_seq);
      INSERT INTO session_nodes VALUES ('key','session',1,'{"chatType":"direct"}');
      INSERT INTO transcript_event_identities VALUES ('session',1,'exact-host-id','message');
      INSERT INTO session_transcript_active_events VALUES ('session',1);`,
    );
    db.prepare(
      "INSERT INTO transcript_events VALUES ('session',1,?,NULL,0,'user')",
    ).run(JSON.stringify({
      message: {
        role: "user",
        content: "should never appear in the identity projection",
        __openclaw: {
          senderId: profile,
          senderIdentity: { type: "profile", id: profile },
          upstreamUserText: "sensitive content",
        },
      },
    }));
    const c = { sessionKey: "key", sessionId: "session" },
      q = () => moon.selectHostAdmission(db, c, "exact-host-id");
    assert.equal(JSON.parse(q().metadata).senderId, profile);
    assert(!JSON.stringify(q()).includes("sensitive"));
    assert(!JSON.stringify(q()).includes("should never appear"));
    assert.equal(moon.selectHostAdmission(db, c, "mismatched"), undefined);
    assert.equal(
      moon.selectHostAdmission(db, { ...c, sessionId: "old" }, "exact-host-id"),
      undefined,
    );
    db.exec("UPDATE transcript_events SET message_role='assistant'");
    assert.equal(q(), undefined);
    db.exec(
      "UPDATE transcript_events SET message_role='user'; DELETE FROM session_transcript_active_events;",
    );
    assert.equal(q(), undefined);
  } finally {
    db.close();
  }
});

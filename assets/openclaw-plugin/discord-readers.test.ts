import { __moonTest, discordRoomAccess } from "./index.js";
import { createHash } from "node:crypto";
import { join } from "node:path";
const [
  guild,
  channel,
  owner,
  sender,
  muted,
  admin,
  bot,
  viewerRole,
  adminRole,
] = Array.from({ length: 9 }, (_, i) => String(10000000000000000n + BigInt(i)));
function fixtures(extraBot = false, type = 0) {
  const data: Record<string, unknown> = {
    [`/channels/${channel}`]: {
      id: channel,
      type,
      guild_id: guild,
      permission_overwrites: [
        { id: guild, type: 0, allow: "0", deny: "1024" },
        { id: viewerRole, type: 0, allow: "1024", deny: "0" },
        { id: muted, type: 1, allow: "0", deny: "1024" },
      ],
    },
    [`/guilds/${guild}`]: {
      id: guild,
      owner_id: owner,
      features: [],
      roles: [
        { id: guild, permissions: "0" },
        { id: viewerRole, permissions: "0" },
        { id: adminRole, permissions: "8" },
      ],
    },
    "/users/@me": { id: bot, bot: true },
    [`/guilds/${guild}/members?limit=1000&after=0`]: [
      { user: { id: owner }, roles: [] },
      { user: { id: sender }, roles: [viewerRole] },
      { user: { id: muted }, roles: [viewerRole] },
      { user: { id: admin, bot: extraBot }, roles: [adminRole] },
      { user: { id: bot, bot: true }, roles: [viewerRole] },
    ],
  };
  return (url: string) => Promise.resolve(data[url]);
}
Deno.test("offline owners/admins retain history while view-only people can see a new post", async () => {
  const access = await discordRoomAccess(fixtures(), channel, sender);
  const principals = (...ids: string[]) =>
    ids.sort().map((id) => `discord:${id}`);
  if (
    JSON.stringify(access?.readers) !==
      JSON.stringify(principals(owner, sender, admin)) ||
    JSON.stringify(access?.owners) !== JSON.stringify(principals(owner, admin))
  ) {
    throw Error("wrong effective viewing or history permissions");
  }
});
Deno.test("a member-level history allow or deny takes precedence over role overwrites", async () => {
  for (const allowed of [true, false]) {
    const source = fixtures();
    const get = async (url: string) => {
      const value = await source(url);
      if (url !== `/channels/${channel}`) return value;
      const c = value as {
        permission_overwrites: Array<
          { id: string; type: number; allow: string; deny: string }
        >;
      };
      c.permission_overwrites[1].allow = allowed ? "1024" : "66560";
      c.permission_overwrites.push({
        id: sender,
        type: 1,
        allow: allowed ? "65536" : "0",
        deny: allowed ? "0" : "65536",
      });
      return c;
    };
    const access = await discordRoomAccess(get, channel, sender);
    if (
      !access?.readers.includes(`discord:${sender}`) ||
      access?.owners.includes(`discord:${sender}`) !== allowed
    ) {
      throw Error(
        "history override did not preserve the full disclosure boundary",
      );
    }
  }
});
Deno.test("an unidentified integration or unsupported thread blocks private recall", async () => {
  for (const get of [fixtures(true), fixtures(false, 11)]) {
    if (await discordRoomAccess(get, channel, sender) !== null) {
      throw Error("unverified reader set");
    }
  }
});

Deno.test("learning counts a readable bot in the audience but never as a private owner", async () => {
  const access = await discordRoomAccess(
    fixtures(true),
    channel,
    sender,
    false,
    true,
  );
  if (
    !access?.readers.includes(`bot:discord:${admin}`) ||
    !access.readers.includes(`discord:${sender}`) ||
    access.owners.some((id: string) => id.includes(admin)) ||
    access.readers.some((id: string) => id.endsWith(bot))
  ) {
    throw Error("wrong human and bot ACLs for private learning");
  }
});

Deno.test("learning without a host audience checks the exact human Discord post before verifying room owners", async () => {
  const messageId = "20000000000000000";
  const message = {
    id: messageId,
    channel_id: channel,
    type: 19,
    author: { id: sender },
    content: "One exact Discord post.\nPreserve this second line.",
  };
  const verify = (reply: Record<string, unknown>, channelType = 0) => {
    const get = fixtures(true, channelType);
    return discordRoomAccess(
      (route: string) =>
        route === `/channels/${channel}/messages/${messageId}`
          ? Promise.resolve(reply)
          : get(route),
      channel,
      sender,
      false,
      true,
      {
        messageId,
        textHash: createHash("sha256").update(message.content).digest("hex"),
      },
    );
  };
  const valid = await verify(message);
  if (
    !valid?.readers.includes(`discord:${sender}`) ||
    !valid.readers.includes(`bot:discord:${admin}`)
  ) throw Error("an exact human reply should admit the mixed room");
  for (
    const invalid of [
      { ...message, author: { id: sender, bot: true } },
      { ...message, author: { id: owner } },
      { ...message, id: owner },
      { ...message, channel_id: guild },
      { ...message, webhook_id: admin },
      { ...message, edited_timestamp: "2026-11-19T00:00:00Z" },
      { ...message, message_reference: { type: 1 } },
      { ...message, content: "Only the final post in a batch." },
      { ...message, content: message.content + "\nCurrent time: 09:30 UTC" },
      { ...message, content: "" },
    ]
  ) {
    if (await verify(invalid)) throw Error("unverified Discord source message");
  }
  if (await verify(message, 11)) throw Error("a thread should not be admitted");
});

Deno.test("fresh external bot reports use an immutable private admission only for a linked master in the room", async () => {
  const now = Date.now();
  const message = {
    id: String(BigInt(now - 1420070400000) << 22n),
    channel_id: channel,
    guild_id: guild,
    author: { id: admin, bot: true },
    type: 0,
    content: "",
    embeds: [
      {
        type: "rich",
        title: "Build status",
        description: "The cobalt job passed.",
      },
    ],
  };
  const master = {
    profile_id: "fixture-user",
    memory_principal: "openclaw:fixture-user",
  };
  const writes: Array<{ argv: string[]; input?: string }> = [];
  const api = {
    config: {
      channels: {
        discord: {
          groupPolicy: "allowlist",
          guilds: { [guild]: { channels: { [channel]: { enabled: true } } } },
        },
      },
    },
    pluginConfig: {
      moonHome: "/private/tmp/synthetic-moon",
      moonPath: "/synthetic/moon",
    },
    runtime: {
      system: {
        runCommandWithTimeout: (
          argv: string[],
          options: { input?: string },
        ) => {
          if (argv.includes("config")) {
            return {
              code: 0,
              stdout: JSON.stringify({
                master,
                learning: { l1: {}, l2: { timezone: "UTC" } },
              }),
            };
          }
          writes.push({ argv, input: options.input });
          return {
            code: 0,
            stdout: JSON.stringify({
              bound: true,
              owners: [master.memory_principal],
            }),
          };
        },
      },
    },
  };
  const room = fixtures(true);
  let actual = message;
  let masterSender = owner;
  const services = {
    client: () =>
      Promise.resolve((route: string) =>
        route.startsWith(`/channels/${channel}/messages/`)
          ? Promise.resolve(actual)
          : room(route)
      ),
    masterSenders: () => Promise.resolve(new Set([masterSender])),
  };
  const ctx = {
    channelId: "discord",
    accountId: "default",
    signal: new AbortController().signal,
  };
  const capture = (m: unknown) =>
    __moonTest.captureDiscordBotMessage(api, { event: m }, ctx, services);
  await capture(message);
  await capture(message);
  if (writes.length !== 4) throw Error("missing private writes");
  const [bind, record, retry] = writes;
  const option = (argv: string[], name: string) => argv[argv.indexOf(name) + 1];
  if (
    !bind.argv.includes("bind-bot") ||
    option(bind.argv, "--sender") !== `bot:discord:${admin}` ||
    option(bind.argv, "--master-profile-id") !== master.profile_id ||
    option(bind.argv, "--master-memory-principal") !==
      master.memory_principal ||
    !JSON.parse(option(bind.argv, "--audience-json")).includes(
      master.memory_principal,
    ) ||
    !record.argv.includes("record") ||
    record.input !== "Bot:\nBuild status\nThe cobalt job passed." ||
    option(record.argv, "--admission-key") !==
      option(retry.argv, "--admission-key")
  ) {
    throw Error("lost bot attribution, private ownership or idempotency");
  }
  writes.length = 0;
  // OpenClaw checks a guild/room name before its wildcard. The raw event
  // cannot resolve either exception from trusted names at this stage.
  for (
    const guilds of [
      {
        [guild]: {
          channels: {
            "*": { enabled: true },
            "staff-private": { enabled: false },
          },
        },
      },
      {
        "*": { channels: { [channel]: { enabled: true } } },
        "staff-guild": { channels: { [channel]: { enabled: false } } },
      },
    ]
  ) {
    const discord = { ...api.config.channels.discord, guilds };
    const restricted = { ...api, config: { channels: { discord } } };
    await __moonTest.captureDiscordBotMessage(
      restricted,
      { event: message },
      ctx,
      services,
    );
    if (writes.length) {
      throw Error("a wildcard bypassed a named Discord exclusion");
    }
  }
  await capture({ ...message, content: "Modified before storage" });
  await capture({ ...message, webhook_id: bot });
  await capture({ ...message, flags: 64 });
  await capture({ ...message, guild_id: "199999999999999999" });
  await capture({ ...message, id: "100000000000000000" });
  actual = { ...message, author: { id: bot, bot: true } };
  await capture(actual);
  actual = message;
  masterSender = muted;
  await capture(message);
  // This human can see new posts but cannot read channel history.
  masterSender = sender;
  await capture(message);
  if (writes.length) {
    throw Error("ineligible private bot evidence was recorded");
  }
});
Deno.test("fresh room snapshot excludes original author after they lose access", async () => {
  const access = await discordRoomAccess(fixtures(), channel, muted);
  if (
    !access || access.readers.includes(`discord:${muted}`) ||
    access.owners.includes(`discord:${muted}`)
  ) {
    throw Error("a historical grant affected the reply's ACL");
  }
});

Deno.test("another bot blocks private disclosure only while it can view the room", async () => {
  // The guest has the viewer role, no history permission and an individual
  // VIEW_CHANNEL override. Both cases use the same guild membership.
  for (const canView of [true, false]) {
    const source = fixtures();
    const access = await discordRoomAccess(
      async (route: string) => {
        const value = await source(route);
        if (route === `/guilds/${guild}/members?limit=1000&after=0`) {
          const members = value as Array<
            { user: { id: string; bot?: boolean } }
          >;
          members.find((member) => member.user.id === muted)!.user.bot = true;
        }
        if (canView && route === `/channels/${channel}`) {
          const room = value as {
            permission_overwrites: Array<{ id: string; deny: string }>;
          };
          room.permission_overwrites.find((entry) => entry.id === muted)!.deny =
            "0";
        }
        return value;
      },
      channel,
      sender,
    );
    if (
      canView ? access !== null : !access?.readers.includes(`discord:${sender}`)
    ) {
      throw Error(
        "private room eligibility disagrees with effective bot access",
      );
    }
  }
});

Deno.test("guild pagination includes an offline reader on the last page and rejects overlapping pages", async () => {
  const base = fixtures();
  const firstRoute = `/guilds/${guild}/members?limit=1000&after=0`;
  const firstPage = await base(firstRoute) as {
    user: { id: string };
    roles: string[];
  }[];
  while (firstPage.length < 1000) {
    firstPage.push({
      user: { id: String(BigInt(bot) + BigInt(firstPage.length)) },
      roles: [],
    });
  }
  const lastId = firstPage[999].user.id;
  for (const overlap of [false, true]) {
    const finalReader = String(BigInt(lastId) + (overlap ? 0n : 1n));
    const pages: string[] = [];
    const access = await discordRoomAccess(
      (route: string) => {
        if (route === firstRoute) {
          pages.push(route);
          return Promise.resolve(firstPage);
        }
        if (route === `/guilds/${guild}/members?limit=1000&after=${lastId}`) {
          pages.push(route);
          return Promise.resolve([{
            user: { id: finalReader },
            roles: [adminRole],
          }]);
        }
        return base(route);
      },
      channel,
      sender,
    );
    if (
      pages.length !== 2 ||
      (overlap
        ? access !== null
        : !access?.owners.includes(`discord:${finalReader}`))
    ) throw Error("incomplete or overlapping room membership accepted");
  }
});

Deno.test("Discord DM channel IDs resolve to recipient IDs, including non-owner DMs", async () => {
  const dm = () =>
    Promise.resolve({ id: channel, type: 1, recipients: [{ id: sender }] });
  const group = () =>
    Promise.resolve({
      id: channel,
      type: 3,
      recipients: [{ id: sender }, { id: owner }],
    });
  const actual = await discordRoomAccess(dm, channel, sender, true);
  if (
    actual?.owners.join() !== `discord:${sender}` ||
    actual?.readers.join() !== `discord:${sender}`
  ) {
    throw Error("direct audience lost");
  }
  if (await discordRoomAccess(group, channel, sender, true) !== null) {
    throw Error("group accepted as DM");
  }
  if ((await discordRoomAccess(group, channel, sender))?.owners.length !== 2) {
    throw Error("group readers dropped");
  }
  if (await discordRoomAccess(dm, channel, muted) !== null) {
    throw Error("sender mismatch");
  }
});

async function testEnv(name: string) {
  const permission = await Deno.permissions.query({
    name: "env",
    variable: name,
  });
  return permission.state === "granted" ? Deno.env.get(name) : undefined;
}

const binary = await testEnv("MOON_TEST_BINARY");
const botHome = await testEnv("MOON_BOT_TEST_HOME");
Deno.test({
  name:
    "real Moon stores a replayed bot event once for Master with bot provenance, hidden from the source room",
  ignore: !botHome || !binary,
  fn: async () => {
    if (!binary || !botHome) throw new Error("missing fixture");
    const database = join(botHome, "state", "moon.sqlite");
    const command = async (argv: string[], input?: string) => {
      if (
        argv[0] !== binary || argv[argv.indexOf("--home") + 1] !== botHome ||
        argv[argv.indexOf("--database") + 1] !== database
      ) {
        throw Error("non-fixture database");
      }
      const child = new Deno.Command(binary, {
        args: argv.slice(1),
        stdin: "piped",
        stderr: "piped",
        stdout: "piped",
      }).spawn();
      const writer = child.stdin.getWriter();
      if (input !== undefined) {
        await writer.write(new TextEncoder().encode(input));
      }
      await writer.close();
      const result = await child.output();
      if (!result.success) {
        throw Error(
          "fixture Moon command failed: " +
            new TextDecoder().decode(result.stderr),
        );
      }
      return {
        code: 0,
        stdout: new TextDecoder().decode(result.stdout),
        stderr: "",
      };
    };
    const cli = async (args: string[]) =>
      JSON.parse(
        (await command([
          binary,
          "--home",
          botHome,
          "--database",
          database,
          "--dimensions",
          "384",
          "--json",
          ...args,
        ])).stdout,
      );
    await cli(["init"]);
    const actor = `bot:discord:${admin}`, master = "openclaw:fixture-user";
    const message = {
      type: 0,
      id: String(BigInt(Date.now() - 1420070400000) << 22n),
      guild_id: guild,
      channel_id: channel,
      author: { id: admin, bot: true },
      content: "The cobalt fixture build passed.",
    };
    const room = fixtures(true);
    const services = {
      client: () =>
        Promise.resolve((route: string) =>
          route === `/channels/${channel}/messages/${message.id}`
            ? Promise.resolve(message)
            : room(route)
        ),
      masterSenders: () => Promise.resolve(new Set([owner])),
    };
    const api = {
      config: {
        channels: {
          discord: {
            guilds: { [guild]: { channels: { [channel]: { enabled: true } } } },
          },
        },
      },
      pluginConfig: { moonHome: botHome, moonPath: binary },
      runtime: {
        system: {
          runCommandWithTimeout: (args: string[], opts: { input?: string }) =>
            command(args, opts.input),
        },
      },
    };
    const ctx = {
      channelId: "discord",
      accountId: "default",
      signal: new AbortController().signal,
    };
    for (let i = 0; i < 2; i++) {
      await __moonTest.captureDiscordBotMessage(
        api,
        { event: message },
        ctx,
        services,
      );
    }
    const history = async (readers: string[]) =>
      await cli([
        "privacy",
        "history",
        "--audience-json",
        JSON.stringify(readers),
      ]);
    const rows = await history([master]);
    if (
      rows.length !== 1 || rows[0].role !== "bot" ||
      rows[0].turn_sender !== actor ||
      rows[0].text !== message.content ||
      (await history([master, actor])).length ||
      (await history([`discord:${sender}`])).length ||
      (await history([])).length
    ) {
      throw Error("private bot history isolation or attribution failed");
    }
    const preview = await cli([
      "learning",
      "prepare",
      "--preview",
      "--run-key",
      "bot-test",
    ]);
    if (
      preview.evidence?.length !== 1 ||
      preview.evidence[0].verified_sender !== actor ||
      !preview.evidence[0].content.includes(
        "Bot:\nThe cobalt fixture build passed.",
      ) ||
      !(await cli(["health"])).ok
    ) throw Error("bot learning or storage integrity failed");
  },
});

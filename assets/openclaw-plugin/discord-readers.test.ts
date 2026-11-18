import { discordRoomAccess } from "./index.js";
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
Deno.test("fresh room snapshot excludes original author after they lose access", async () => {
  const access = await discordRoomAccess(fixtures(), channel, muted);
  if (
    !access || access.readers.includes(`discord:${muted}`) ||
    access.owners.includes(`discord:${muted}`)
  ) {
    throw Error("a historical grant affected the reply's ACL");
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

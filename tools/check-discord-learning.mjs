// Check the default OpenClaw Discord account's eligibility for private Moon
// learning in a configured guild text room. Outputs no identities or content.
import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const usage =
  "node tools/check-discord-learning.mjs OPENCLAW_PACKAGE [ROOM_ID [SENDER_ID]]";
const [pkg, roomArg, senderArg, ...extra] = process.argv.slice(2);
const snowflake = /^[1-9]\d{15,19}$/;
const checks = {};
class ProbeError extends Error {}

async function checkRoom() {
  if (
    !pkg || extra.length || (roomArg && !snowflake.test(roomArg)) ||
    (senderArg && !snowflake.test(senderArg))
  ) throw new ProbeError("invalid_arguments");
  const root = resolve(pkg);
  const host = createRequire(join(root, "package.json"));
  const sdk = (name) =>
    import(pathToFileURL(host.resolve(`openclaw/plugin-sdk/${name}`)).href);
  const cli = (...args) =>
    JSON.parse(execFileSync(process.execPath, [
      join(root, "openclaw.mjs"),
      ...args,
      "--json",
    ], {
      encoding: "utf8",
      timeout: 30_000,
      stdio: ["ignore", "pipe", "pipe"],
    }));
  const { readConfigFileSnapshot } = await sdk("health");
  const snapshot = await readConfigFileSnapshot();
  if (!snapshot.valid) throw new ProbeError("invalid_openclaw_config");
  const config = snapshot.config;
  const discord = config.channels?.discord;
  if (
    !discord || discord.enabled === false ||
    Object.keys(discord.accounts ?? {}).length
  ) {
    throw new ProbeError("requires_single_default_discord_account");
  }
  const rooms = new Set(
    Object.values(discord.guilds ?? {}).flatMap((guild) =>
      Object.entries(guild.channels ?? {}).filter(([, policy]) =>
        policy?.enabled !== false
      )
        .map(([id]) => id).filter((id) => snowflake.test(id))
    ),
  );
  let room = roomArg;
  if (!room) {
    const list = cli(
      "sessions",
      "--active",
      "1440",
      "--all-agents",
      "--limit",
      "all",
    );
    const recent = new Set(
      list.sessions.map((s) =>
        /^agent:[^:]+:discord:channel:(\d+)$/.exec(s.key)?.[1]
      ).filter((id) => rooms.has(id)),
    );
    if (list.hasMore || recent.size !== 1) {
      throw new ProbeError("specify_room_id");
    }
    [room] = recent;
  }
  if (!rooms.has(room)) {
    throw new ProbeError("room_outside_configured_channels");
  }
  const senders = new Set(
    (discord.allowFrom ?? []).map((value) =>
      /^(?:discord:)?(?:user:)?([1-9]\d{15,19})$/.exec(String(value))?.[1]
    ).filter(Boolean),
  );
  if (!senderArg && senders.size !== 1) {
    throw new ProbeError("specify_sender_id");
  }
  const sender = senderArg ?? [...senders][0];

  const { plugin } = cli("plugins", "inspect", "moon");
  if (plugin?.id !== "moon" || plugin.status !== "loaded") {
    throw new ProbeError("moon_unavailable");
  }
  const { discordRoomAccess } = await import(pathToFileURL(plugin.source).href);
  const { resolveConfiguredSecretInputString } = await sdk(
    "secret-input-runtime",
  );
  const token = await resolveConfiguredSecretInputString({
    config,
    env: process.env,
    value: discord.token,
    path: "channels.discord.token",
    unresolvedReasonStyle: "generic",
  });
  if (!token.value) throw new ProbeError("discord_credential_unavailable");

  let selfId;
  const snapshots = new Map();
  const otherBots = new Set();
  const get = async (path) => {
    let endpoint;
    if (path === `/channels/${room}`) endpoint = "room";
    else if (/^\/guilds\/\d+$/.test(path)) endpoint = "guild";
    else if (path === "/users/@me") endpoint = "self";
    else if (/^\/guilds\/\d+\/members\?limit=1000&after=\d+$/.test(path)) {
      endpoint = "members";
    } else throw new ProbeError("unsupported_verifier_request");
    const response = await fetch(`https://discord.com/api/v10${path}`, {
      method: "GET",
      headers: { Authorization: `Bot ${token.value}` },
      redirect: "error",
      signal: AbortSignal.timeout(8000),
    });
    checks[endpoint] = response.status;
    if (!response.ok) throw new ProbeError("discord_http_error");
    const data = await response.json();
    snapshots.set(path, data);
    if (endpoint === "self") selfId = data.id;
    if (endpoint === "members" && selfId && Array.isArray(data)) {
      for (const member of data) {
        if (member.user?.bot && member.user.id !== selfId) {
          otherBots.add(member.user.id);
        }
      }
    }
    return data;
  };
  // New adapters retain external bots as readers while verifying human
  // ownership. Older installed releases refuse a mixed room outright.
  const access = await discordRoomAccess(get, room, sender, false, true);
  const verified = access?.readers.includes(`discord:${sender}`) === true;
  let readableBotConfirmed = access
    ? access.readers.some((id) => id.startsWith("bot:discord:"))
    : null;
  if (!verified && otherBots.size) {
    // Explain a refusal using only the captured responses. Changing the bot
    // flag in memory leaves every role and permission overwrite intact. A bot
    // appearing as a reader in this replay proves it can view the room. The
    // real verifier result above is the only source for eligibility and exit.
    const replay = await discordRoomAccess(
      (route) => {
        const value = snapshots.get(route);
        if (!route.includes("/members?") || !Array.isArray(value)) return value;
        return value.map((m) =>
          otherBots.has(m.user?.id)
            ? { ...m, user: { ...m.user, bot: false } }
            : m
        );
      },
      room,
      sender,
    );
    if (
      replay?.readers.some((id) => otherBots.has(id.slice("discord:".length)))
    ) {
      readableBotConfirmed = true;
    }
  }
  console.log(
    JSON.stringify({
      verified,
      human_learning_audience_verified: verified,
      sender_history_permission_verified:
        access?.owners.includes(`discord:${sender}`) === true,
      private_recall_audience_verified: verified &&
        readableBotConfirmed === false,
      readable_other_bot_confirmed: readableBotConfirmed,
      checks,
    }),
  );
  process.exitCode = verified ? 0 : 1;
}

if (pkg === "--help" || !pkg) {
  console.log(
    `${usage}\nDefaults to the sole configured room active in the last 24 hours and the sole allowed sender.\nExit codes: 0 human learning audience verified, 1 learning audience denied, 2 check incomplete.`,
  );
  process.exitCode = pkg ? 0 : 2;
} else {
  await checkRoom().catch((error) => {
    const reason = error instanceof ProbeError
      ? error.message
      : "check_incomplete";
    console.log(JSON.stringify({ verified: false, error: reason, checks }));
    process.exitCode = 2;
  });
}

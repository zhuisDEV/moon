# Public and private memory

Moon 2.9.4 uses schema 21. It checks ownership independently for saved facts,
original messages and delivered replies used as evidence. See the
[migration and rollback guide](migration.md#schema-21-delivered-reply-citations).

## Personal OpenClaw profile

Sign in to OpenClaw as an individual and identify your personal profile with
`openclaw users list`. The shared `gateway-owner` represents clients using the
same Gateway credential. OpenClaw cannot link a channel identity to it. A
Gateway administrator can link the personal profile to the channel `discord`,
the configured bot account ID and the human's immutable Discord user ID.
Substitute the identifiers from your Gateway in these examples.

```sh
openclaw gateway call users.linkChannelIdentity --json --params \
  '{"profileId":"PERSONAL_PROFILE_ID","identity":{"channelId":"discord","accountId":"default","senderId":"DISCORD_USER_ID"}}'
openclaw gateway call users.listChannelIdentities --json --params \
  '{"profileId":"PERSONAL_PROFILE_ID"}'
```

OpenClaw can grant its linked channel sender owner authority when the profile
has an administrator role. Check the profile's effective role before linking.

Set the profile once in the operator-owned Moon home:

```toml
[master]
profile_id = "YOUR_PERSONAL_PROFILE_ID"
# Treat the first verified human DM on a new channel account as yours.
enroll_first_direct_sender = true
# For an existing installation, preserve the verified owner's existing Discord memories.
memory_principal = "discord:YOUR_EXISTING_DISCORD_USER_ID"
```

On a new installation, omitting `memory_principal` uses `openclaw:<profile_id>`.
To keep using a historical Discord collection, the configured legacy ID must
match a current Discord link to the personal profile. Once that ID owns
profile-bound evidence, disabling the setting or changing it to another
principal does not restore raw Discord access to the old collection.

The installed OpenClaw Gateway is the authority for personal links. Moon uses
the active local SQLite profile store and the admitted message in the default
agent session storage. Custom transcript stores and PostgreSQL are outside this
identity check. A new direct channel must supply a trusted human identity,
account and direct-conversation receipt. By default Moon can enrol a sender with
a personal Gateway link or a sole, exact DM allowlist entry. With
`enroll_first_direct_sender = true`, the first verified human to send a direct
message on a new channel account is assumed to be Master, even if other people
can message it. Send the first DM yourself when connecting a new tool. Moon
records that sender and account under `master.channels` in `moon.toml`.
Subsequent senders keep their own private learning. Changing the enrolled sender
requires an operator edit. A conflicting Gateway link blocks first-use enrolment
and a revoked personal link stops authorising the old identity. This setting
controls Moon's learning attribution and grants no Gateway role.

In TUI and Control UI chat, Moon checks the personal profile on the accepted
message. Personal direct turns contribute evidence to the same collection as the
linked Discord account. Replies in TUI, Control UI and other new chat tools use
public context only. OpenClaw broadcasts native chat replies by session, and
external adapters can implement their own delivery. Moon can verify the sender
on these routes but cannot check all recipients at delivery. Use a verified
one-to-one Discord DM to recall what Moon learned across these channels. A
shared token login, a client label or a display name supplies no personal
identity.

New external bot messages in Discord text rooms configured by exact guild and
channel IDs can enter the Master's private memory when their linked Discord
identity has both `VIEW_CHANNEL` and `READ_MESSAGE_HISTORY` at receipt.
Effective guild owner and administrator permissions also qualify. A member with
live viewing access alone can see new posts, but cannot acquire a durable copy
of another author's message. The observer captures ordinary new bot posts and
replies, including rich-card text, after verifying the source message with
Discord. Moon keeps the bot as the source, deduplicates deliveries and queues
its evidence for L2 when L2 is enabled. Human turns keep their own ownership.
Replies in rooms with readable bots receive public context only.

## What a conversation can recall

Each private memory has an owner set. Before Lilac can repeat it, every person
who could read the destination must belong to that set. A DM is the one-reader
case. A room can have more potential readers than the original conversation had
owners. Those additional readers prevent private recall in that room. Discord
principal IDs survive display-name changes. Identities from other platforms
would need independent verification before linking them.

| Source of memory          | Alice alone | Bob alone | Alice and Bob | New reader present |
| ------------------------- | ----------- | --------- | ------------- | ------------------ |
| Alice's private message   | Yes         | No        | No            | No                 |
| Bob's private message     | No          | Yes       | No            | No                 |
| A message they both own   | Yes         | Yes       | Yes           | No                 |
| Explicit public statement | Yes         | Yes       | Yes           | Yes                |

The Discord adapter uses trusted host turn metadata and verifies the human
sender. For an ordinary guild text room it enumerates guild members and applies
the guild owner, administrator, roles and per-member channel permissions. Silent
readers count. Membership listing requires the bot's Guild Members privileged
intent. Threads, unsupported transports, discoverable guilds, unverified
readers, and rooms with another readable bot do not get private recall. On
unknown routes the adapter can retrieve public material and does not create
private learning evidence. A user saying who is present is not proof of
membership. OpenClaw and Discord must support the required hooks and
authenticated lookups. An embedded host path without a trusted message identity
uses public recall only and skips private learning.

The signed v2.8.1 adapter requires a host audience grant for private DM recall.
With `plugins.slots.memory` set to `none`, OpenClaw can omit this grant. Since
v2.8.2 the adapter can verify a one-to-one Discord DM independently. It matches
the author and exact text of the active user message in OpenClaw's default
SQLite agent store to one original human post fetched from Discord. The author
must be the DM's sole peer. Set `session.dmScope` to `per-account-channel-peer`
and keep that peer's Discord session separate from other users in
`identityLinks`. A verified DM can recall private memories owned by that peer,
including facts learned from their guild posts. If any check fails, Moon
withholds private context. Reading or recording private reply history still
requires a host grant.

For an ordinary Discord guild text room, Moon can still learn from a completed
human turn by verifying the active admission in OpenClaw's default SQLite agent
store, the author, the exact text of one Discord post and current room access.
Moon skips combined posts, edited posts and mismatched text. The verified author
alone owns the question because a queued turn can run after channel membership
changes. Guild replies on this route use public recall.

At question admission, Moon records the verified speaker, the people who can
view the destination, and the owners of the question. The speaker owns their own
words. With a host audience grant in a supported Discord text room, every
verified human with both `VIEW_CHANNEL` and `READ_MESSAGE_HISTORY` also owns the
question, including offline members. A confirmed reply gets a separate ownership
snapshot at delivery. An owner added after a question can own the reply alone.
An owner who has lost access can keep the question alone. In a DM, the verified
recipient owns the reply.

For disclosure, everyone with effective `VIEW_CHANNEL` counts, including a
member who can see new posts but lacks history access. Moon freezes each
verified owner list. Later membership changes never add someone to an earlier
message. A current permissions check cannot reconstruct old Discord permissions.

### Searching a shared room

Moon selects the public collection and the private owner sets that contain every
current destination reader. It searches those collections together in one read
transaction and returns only eligible records and their citations. If Alice and
Bob independently use identical wording in their DMs, those are still two
separate records. Neither can enter the context of their shared room. A
statement they both own from a shared conversation is eligible. The same rule
applies independently to each question and reply in chat history. A summary may
use only the messages available to that destination.

Within the same room, two people can state different preferences. Moon keeps
each speaker’s learned statements separate and identifies the verified source
when recalling them. That identity tells Lilac who said something. If a person
describes somebody else, the wording of the statement determines whom it is
about. Historical evidence without a verified speaker retains that uncertainty.

Moon keeps accepted human input as learning evidence. A confirmed Lilac reply
can be kept as private conversation history under Lilac's role. Looking up that
reply does not turn Lilac's statement into a fact asserted by the human.
Recognised credentials are excluded. Pattern matching cannot identify every
possible secret.

## Finding conversations by sender

On the trusted operator CLI, use `privacy by-sender --sender discord:USER_ID` to
list private evidence associated with a verified sender in chronological order.
The `owners` field belongs to the question. Each confirmed entry in
`reply_deliveries` has its own owners, Discord message ID and delivery time.
`question_access_at_ms` identifies when Moon recorded the question's room access
snapshot. The list omits message text. Use the last evidence ID as `--after` for
the next page. The lookup covers attributed turns from DMs and shared rooms. It
does not grant the sender or other chat users additional recall permissions.

Schema 16 leaves existing evidence without a verified author unattributed.
Historical names, room memberships and single-reader audience records do not
establish who authored a particular turn.

## Recalling private replies

The OpenClaw main model decides from the current request whether the user wants
saved facts, recent role-labelled messages or neither. Defining “chat history”
and asking to quote a previous reply are different tasks. A history request on a
saved-facts-only route produces an unavailable notice and performs no
stored-content lookup. Original-message searches default to the current
conversation. A request explicitly covering other conversations can search
messages that every reader owns. Moon filters by conversation before selecting
recent records. `privacy history` applies the destination audience to each
question and each reply independently, before returning any text. An eligible
answer can stand alone without revealing the private question or its author.

Moon retains an assistant reply for recall when its accepted final text matches
a successful Discord delivery for the same turn and destination with verified
delivery permissions. Missing receipts, unstable delivery permissions, and
changed or split text can leave gaps. Previous replies with no independent
delivery snapshot stay in the database for local administration. Raw tool
traffic stays outside this history.

## Publishing and withdrawal

Public means every current and future authorised Moon user. A human can
explicitly publish the wording using the Discord command:

```text
/moonpublic fact Our delivery entrance is on Birch Street.
/moonpublic skill Use OCR before extracting totals from a scanned invoice.
/moonpublic list
/moonpublic withdraw 42
```

Discord command retries deduplicate by actor, kind and exact wording, including
after withdrawal. Publishing the identical wording again requires a fresh
operator-supplied request key because this command surface lacks a unique event
ID.

The shared record contains the approved wording. It does not link a reader to a
private transcript. Withdraw with the account that published it. Withdrawal
removes the record from future Moon recall. Already delivered messages, provider
transcripts, and backups can retain it. Successful recognised web tools can
register fixed, reusable technique descriptions. Raw arguments, paths, results
and provider credentials are never copied by this mechanism. Novel techniques
can be published explicitly. Learning a method grants no permission to execute
it.

## Reviewing the old global collection

Take a consistent online backup with the installed version and keep its matching
binary, adapter and configuration. Restore the backup into a private temporary
home and test the candidate there first. Schema 15 preserves old global
documents and citations in an operator-only review collection. Its new public
collection starts empty. Quarantined pins, evidence and unfinished learning work
cannot enter audience-based recall or learning. Other operator collections
remain available through the local administrative CLI.

All examples below assume that `moon_copy` points to the rehearsal directory and
`candidate` to the 2.9.4 binary. The database path is explicit so an inherited
environment variable cannot select the live database.

```sh
"$candidate" --home "$moon_copy" --database "$moon_copy/state/moon.sqlite" init
"$candidate" --home "$moon_copy" --database "$moon_copy/state/moon.sqlite" --json health
"$candidate" --home "$moon_copy" --database "$moon_copy/state/moon.sqlite" --json privacy inventory
"$candidate" --home "$moon_copy" --database "$moon_copy/state/moon.sqlite" --json privacy candidates --limit 20
```

When the operator attests that the entire pre-cutover global memory collection
belongs to one person, use the one-time assignment on the copy:

```sh
"$candidate" --home "$moon_copy" --database "$moon_copy/state/moon.sqlite" --json privacy assign-legacy --owner discord:USER_ID
```

This assigns even memories with mixed or missing citations. Only current records
enter the owner's private recall. Archived records retain their status. The
assignment does not grant access to raw source transcripts, backfill an author
or publish anything. Repeating it for the same person is safe. The command
rejects reassignment. For collections without this ownership attestation, use
individual review instead.

Use `privacy review --id ID` in a private operator terminal. Verify the original
audience using contemporaneous, trusted records. Present-day room membership and
display names are insufficient. When the original, single user-authored citation
and audience can be verified, pass its exact wording on stdin to
`privacy recover --document-id ID --audience-json '["discord:PERSON_ID"]' --verified`.
This creates a separate private memory with narrow evidence and retains the
original. Repeating the same recovery is safe. Ambiguous provenance, mixed
citations and assistant-only text stay in quarantine. An owner may independently
publish a reviewed general method or expressly approved statement through the
normal publication workflow. Never bulk-assign legacy globals to the public
scope. An unresolved record can stay in quarantine indefinitely. If a person
confirms a fact again in a verified DM, Moon can learn from that new private
message while preserving the original record in quarantine.

Check the resulting inventory, run the audience matrix with synthetic examples,
then restore the pre-migration backup into another temporary home and check it
with the matching old binary. Keep the backup private. Rollback requires the
matching runtime and database together.

## OpenClaw and Discord boundaries

The audience-filtered CLI and worker are trusted adapter interfaces. Principals
passed on an operator's command line are assertions by that operator. Never
expose the administrative CLI, database, backups, arbitrary process execution or
another user's session tools to a chat agent that must enforce privacy.

A [restricted chat configuration](../examples/openclaw-private-chat.json) shows
the policy for a dedicated Discord gateway. It gives the model only its own
session-status tool, uses automatic replies, disables ACP and elevated
execution, and selects per-session sandboxing without workspace access. The
operator still configures the channel, model, plugin and sandbox backend. Apply
the policy on a separate gateway with a clean workspace, check the effective
tools for a Discord turn, and keep the host administration gateway separate. Use
an OS or sandbox boundary that denies access to Moon storage and host
credentials, restrict session visibility to the current conversation, and
restrict cross-agent delegation and outbound routing. Start fresh sessions at
cutover; old prompts or summaries may contain previously recalled global data.
Check native model tools as well as OpenClaw tools in the actual host policy.

The adapter checks the audience before releasing private context to the model
and again before delivery when OpenClaw identifies the same run and destination.
When a host omits the run ID, the adapter checks each matching protected
conversation before allowing delivery. Recording reply ownership still requires
a correlated delivery receipt. These checks and Discord delivery are separate
operations. Discord membership can change between them, and Discord can reveal
posted history to later members. Moon does not mediate every delivery, alternate
message tool, or platform history change. Use a verified DM when historical
confidentiality is required. Discord access rules, model-provider access,
operator access and retention are independent of Moon recall.

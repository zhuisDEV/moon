# Moon

Moon gives OpenClaw persistent, cited memory. One Rust binary and one SQLite
database store evidence, memories, sources, topics and search indexes. Local
keyword and vector search assemble context for the current question.

## Start here

| Your situation            | Guide                                                                       |
| ------------------------- | --------------------------------------------------------------------------- |
| First installation        | [Getting started](docs/getting-started.md)                                  |
| Existing installation     | [Upgrade checklist](docs/updating.md#upgrade-checklist)                     |
| Agent instructions        | [Refresh SKILL.md and its guides](docs/updating.md#refresh-the-agent-skill) |
| Check recall or learning  | [Verification](docs/getting-started.md#verify-the-installation)             |
| Capture articles or notes | [Source capture and review](docs/source-storage.md)                         |
| Combine reviewed sources  | [Topic synthesis](docs/topics.md)                                           |

**Moon 2.8.1 updates `SKILL.md`. Refresh every separately installed copy**,
including Codex or other agent folders. The updater refreshes its managed
OpenClaw skill. The six-file compatibility bundle omits the supporting guides.
Copy the guides from the same release using the
[skill update instructions](docs/updating.md#refresh-the-agent-skill).

## What runs automatically

Moon retrieves public and audience-permitted private memories before a turn.
Each newly recorded private chat turn keeps the verified sender ID, question
owners and potential readers. Confirmed replies have separate delivery owners.
Moon extracts eligible durable memories through L1, and drains local embedding
work. Daily L2 reconciliation is optional and **disabled by default**. L1 and L2
have independent model and reasoning settings in `~/.moon/moon.toml`. OpenClaw
owns authentication and model execution; Moon creates no provider login or
API-key store.

With OpenClaw's native memory disabled, Moon can recall a user's private
memories in a verified one-to-one Discord DM. The session must be isolated to
the bot account and the user. In ordinary guild text rooms, Moon can still learn
from a verified post for its author while using public context for replies. See
the [privacy rules](docs/memory-privacy.md#what-a-conversation-can-recall).

The adapter can give one personal OpenClaw profile a private Moon memory across
linked accounts. It can also save new messages from external bots for that
person in Discord rooms they can read. The bot remains the recorded author. See
[profile setup and supported chat routes](docs/memory-privacy.md#personal-openclaw-profile).

The new public collection starts empty. Assign a known owner or review legacy
global memories using the
[audience and migration guide](docs/memory-privacy.md).

Daily L2 gives rejected session evidence one routine retry on a later
occurrence. Evidence that still lacks support stays pending. Fresh records take
priority.
[Retry and review policy](docs/learning.md#daily-schedule-and-recovery)

Source learning and topic compilation are explicit workflows. Capturing a source
does not make it trusted or recallable. Eligible sources queue recoverable jobs;
L1 can propose verbatim claims and L2 can propose typed topic statements.
Separate review and exact-result acceptance are required. Withdrawal immediately
removes source support and invalidates dependent topics. No automatic
source/topic worker is installed.

The CLI and adapter default packet cap is **3,500 characters**. The measured
recommendation for source/topic use is **8,000**, explicitly configured with
adapter `maxChars` or CLI `--max-chars`. Packet capacity and stored reference
count are separate limits. See [packet comparison](docs/packet-selection.md) and
[retrieval measurements](docs/performance.md).

## Storage and safety

- Runtime: `~/.moon`; database: `~/.moon/state/moon.sqlite`.
- Moon 2.8.1 uses schema **20**. Signed updates accept schemas **6–20** and
  migrate transactionally. Retain the old executable with its matching database
  backup.
- SQLite uses WAL with `synchronous=FULL`. Unix runtime directories and data
  files are owner-only.
- `health` and source/topic inspection do not create or migrate storage. Normal
  write commands can migrate it. Back up using the **installed pre-upgrade
  binary**.
- Bundled SQLite 3.54.1 and refreshed TLS dependencies address the security
  findings discovered during release validation.
- Markdown is an intake/export format, not another writable knowledge store.
- Retrieved text is untrusted. Citations establish where text occurred; review
  remains necessary for meaning and truth.
- Local search needs no provider login. First hybrid use downloads its local
  embedding model. Model calls use OpenClaw's configured providers.

Read [Security](SECURITY.md) for trust and deletion boundaries. Development and
tests must pass an explicit temporary `--home`; never test writes on `~/.moon`.

## Guides

- [How Moon works](docs/how-it-works.md): evidence, memories, references and
  packets.
- [Learning](docs/learning.md): L1/L2 models, scheduling and recovery.
- [Source storage](docs/source-storage.md): intake, claims, review and
  lifecycle.
- [Topics](docs/topics.md): compilation, acceptance and freshness.
- [Migration](docs/migration.md): schema changes, backup rehearsal and legacy
  imports.
- [Updating](docs/updating.md): signed releases, skill refresh and recovery.
- [Architecture](docs/architecture.md): invariants and ownership boundaries.
- [Model canary](docs/source-learning-canary.md): isolated real-model
  validation.
- [Recall evaluation](docs/memory-improvement-plan.md),
  [topic baseline](docs/topic-baseline.md) and
  [packet comparison](docs/packet-selection.md): evidence and limitations.
- [Compaction](docs/compaction.md): optional host-owned transcript
  summarisation.
- [Support](SUPPORT.md), [contributing](CONTRIBUTING.md) and
  [releasing](RELEASE.md).

[`SKILL.md`](SKILL.md) is the operating guide for agents. Read it before
maintenance, then open the detailed guide relevant to the task.

## Development

Rust stable and Cargo build the self-contained binary. Deno 2 runs adapter and
developer-tool tests; it is not a Moon runtime dependency.

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
sh tools/test-openclaw-adapter.sh "$PWD/target/release/moon"
```

Offline, isolated smoke test:

```bash
moon_test_home="$(mktemp -d /tmp/moon-test.XXXXXX)"
target/release/moon --home "$moon_test_home" --database "$moon_test_home/state/moon.sqlite" init
target/release/moon --home "$moon_test_home" --database "$moon_test_home/state/moon.sqlite" remember \
  --kind decision --scope example --content "Moon uses one SQLite database."
target/release/moon --home "$moon_test_home" --database "$moon_test_home/state/moon.sqlite" context \
  --scope example --query "SQLite database" --mode lexical
```

Use `moon --help` for exact options. The deterministic `hash` embedding provider
is for plumbing tests; production hybrid recall uses local multilingual E5. See
[Contributing](CONTRIBUTING.md) for the full validation commands.

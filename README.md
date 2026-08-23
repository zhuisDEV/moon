# Moon

Moon v2 is the formal Moon memory engine. Production uses one Rust binary and
one SQLite database for structured memory, FTS5 keyword search, embedded vector
search, and hybrid retrieval.

The installed `moon` command and OpenClaw adapter use `~/.moon`. Development,
tests, imports, and replays should always pass an explicit temporary `--home`.

## Safety defaults

- Production runtime root: `~/.moon`
- Production database: `~/.moon/state/moon.sqlite`
- Legacy import is read-only.
- QMD, Node.js, a vector server, and the legacy watcher are not required.
- The retired runtime is retained as a dated, read-only rollback copy.
- Moon does not require `OPENAI_API_KEY`.
- Model calls use OpenClaw's configured primary and fallback providers. Moon
  does not read or copy provider credentials.
- Runtime directories are owner-only on Unix; databases, backups, and exports
  are created with owner-only file permissions.
- `health` never creates or migrates a missing database.
- Retrieved memory is untrusted data. Adapters must keep context below trusted
  instructions and must not promote memory text into system or developer
  instructions.

## Install or upgrade

Moon v2.2.0 introduced the native updater. Use v2.2.1 or later for the initial
bootstrap so existing v2 databases retain their established embedding-model
identity. Once installed, signed updates are the primary path and require no
Rust toolchain:

```bash
moon update --check
moon update --dry-run
moon update
```

`--check` is strictly read-only. `--dry-run` downloads and verifies the signed
compatibility set, checks health, space, platform, schema, executable identity,
and OpenClaw compatibility, then prints the exact plan without changing local
state. Applying requires one interactive confirmation, or `--yes` for an
explicit non-interactive invocation. Moon never updates in the background.

Moon v2.1.0 cannot invoke an updater it does not contain. Its one-time v2.2.1
bootstrap must therefore use the controlled, pinned release procedure in
[docs/updating.md](docs/updating.md). The no-toolchain update promise begins
after that bootstrap. Existing releases and rollback bundles are retained until
the owner separately authorizes cleanup.

The source-build procedure below is recovery/development guidance, not the
normal update path.

Back up an existing Moon runtime before replacing its binary or plugin:

```bash
~/.moon/bin/moon backup --destination /path/to/moon-before-upgrade.sqlite
~/.moon/bin/moon export --destination /path/to/MEMORY-before-upgrade.md
```

Build and install the v2 binary:

```bash
cargo build --locked --release
install -d -m 700 ~/.moon/bin
install -m 755 target/release/moon ~/.moon/bin/moon
export PATH="$HOME/.moon/bin:$PATH"
hash -r
command -v moon
moon --version
moon --json --version
moon init
moon --json health
```

Persist the `PATH` entry in your shell startup file and keep `~/.moon/bin` ahead
of legacy install locations such as `~/.cargo/bin`. Before running a migration,
confirm that `command -v moon` resolves to the v2 binary you just installed and
that `moon --version` reports the expected release. If an older binary shadows
v2, do not follow its migration prompt against the newer database; repair the
command resolution or use `~/.moon/bin/moon` explicitly.

`moon --json --version` is also offline and storage-independent. It reports the
invoked executable, canonical `~/.moon/bin/moon` path, build target, Git commit,
and whether the build came from a dirty checkout. It does not create or inspect
the database. A release artifact must report `git_dirty: false`.

Install the OpenClaw adapter from this checkout and select Moon as the sole
context and memory owner:

```bash
openclaw plugins install --link "$PWD/assets/openclaw-plugin"
openclaw config set --batch-json '[
  {"path":"plugins.entries.moon.enabled","value":true},
  {"path":"plugins.entries.moon.config","value":{
    "moonPath":"~/.moon/bin/moon",
    "moonHome":"~/.moon",
    "mode":"hybrid",
    "compactionModel":"vllm/your-local-model",
    "compactionReasoning":"off",
    "compactionTimeoutMs":180000,
    "compactionMaxTokens":4096,
    "embeddingEnabled":true,
    "embeddingBatchSize":64,
    "embeddingTimeoutMs":120000
  }},
  {"path":"plugins.slots.contextEngine","value":"moon"},
  {"path":"plugins.slots.memory","value":"none"},
  {"path":"agents.defaults.memorySearch.enabled","value":false},
  {"path":"agents.defaults.compaction.mode","value":"safeguard"},
  {"path":"agents.defaults.compaction.provider","value":"moon-local"},
  {"path":"agents.defaults.compaction.model","value":"vllm/your-local-model"},
  {"path":"agents.defaults.compaction.identifierPolicy","value":"strict"},
  {"path":"agents.defaults.compaction.recentTurnsPreserve","value":3},
  {"path":"agents.defaults.compaction.qualityGuard","value":{"enabled":true,"maxRetries":1}}
]'
openclaw config validate
openclaw plugins doctor
openclaw gateway restart --safe
```

The first local embedding request downloads the multilingual E5 model into the
Moon runtime. See [docs/migration.md](docs/migration.md) before upgrading from
Moon v1 and [docs/updating.md](docs/updating.md) for the signed update and
recovery contract.

## Development

```bash
cargo build
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked --release
```

The command surface is documented by:

```bash
cargo run -- --help
```

Release maintainers can build deterministic unsigned compatibility-set archives
with `cargo run --locked --example moon-release -- --help`. The tool validates
candidate provenance; its macOS-only signing command reads the production key
through an interactive, dedicated Keychain and never exports it. See
[RELEASE.md](RELEASE.md) and [docs/release-signing.md](docs/release-signing.md).

See [docs/architecture.md](docs/architecture.md) for the storage and retrieval
contracts. Start with [docs/how-it-works.md](docs/how-it-works.md) for the
workflow-first explanation.

## AI-agent skill

[`SKILL.md`](SKILL.md) is the canonical operating guide for AI agents. Normal
OpenClaw use is automatic; the skill is for health checks, recall diagnosis,
backups, exports, and explicitly authorized maintenance.

To make the skill available to OpenClaw:

```bash
install -d -m 700 ~/.openclaw/skills/moon
install -m 644 SKILL.md ~/.openclaw/skills/moon/SKILL.md
```

Do not carry forward v1 agent commands such as `moon recall`, `moon watch`,
`moon cleanse`, or `moon context-engine`; they are not part of Moon v2.

## Evidence-backed memory workflow

Moon separates completed-session evidence, durable memories, and per-request
context packets:

```bash
moon --home /tmp/moon-test record \
  --session-id example-session \
  --scope moon \
  --file /path/to/completed-session.txt

moon --home /tmp/moon-test distill \
  --key moon:storage \
  --kind decision \
  --scope moon \
  --content "Moon uses one canonical SQLite database." \
  --session-id example-session \
  --evidence-quote "Moon uses one canonical SQLite database."

moon --home /tmp/moon-test context \
  --query "How does Moon store memory?" \
  --scope moon \
  --mode hybrid \
  --provider local
```

The OpenClaw adapter embeds new durable memories automatically and drains
reference backlog in bounded batches. Raw completed-turn evidence remains
lexical and citation-only. When no reviewed canonical memory matches, `context`
can fill the remaining budget with deduplicated source excerpts. These stay
visibly separate as unreviewed references with source and byte citations.

Every user-facing context request also records content-free local metrics. Moon
stores an opaque request ID, timestamp, mode, latency, result counts, packet
size/truncation, adapter injection state, and optional human review label. It
does not store the query, prompt, response, recalled content, source URI, scope,
channel/session identity, credentials, or arbitrary error text in metrics.
Content-free operational events also count completed-turn learning outcomes,
embedding batches and remaining work, and native compaction outcomes.

```bash
moon metrics summary --since 7d
moon metrics recent --since 7d --limit 20
moon metrics review --request <opaque-id> --outcome useful --expected-rank 1
moon metrics export --since 7d --destination /path/to/moon-metrics.json
moon metrics prune --older-than 30d       # dry run
moon metrics prune --older-than 30d --yes # delete matching metric rows only
```

See [docs/memory-improvement-plan.md](docs/memory-improvement-plan.md) for the
review labels, interpretation limits, retention workflow, and release gates.

## Isolated smoke test

Use an explicit test root. Nothing below reads or writes `~/.moon`:

```bash
cargo run -- --home /tmp/moon-test --dimensions 384 init
cargo run -- --home /tmp/moon-test --dimensions 384 remember \
  --kind decision \
  --scope moon \
  --content "Moon uses one embedded SQLite memory store."
cargo run -- --home /tmp/moon-test --dimensions 384 embed --provider hash
cargo run -- --home /tmp/moon-test --dimensions 384 search \
  --query "embedded memory store" \
  --mode hybrid \
  --provider hash
```

The deterministic `hash` provider exists only for offline vector plumbing and
latency validation. Production hybrid search uses the local multilingual E5
provider. It does not require a model login, API key, remote vector service, or
QMD. OpenClaw keeps one private stdio child warm so query inference does not pay
model startup cost on every turn.

The Moon binary performs no remote model calls and owns no provider credential
store. The adapter delegates model work to OpenClaw. By default it inherits
`agents.defaults.model.primary` and the first entry in
`agents.defaults.model.fallbacks`; provider authentication remains entirely
inside OpenClaw.

The adapter can also register `moon-local` as OpenClaw's safeguard compaction
provider. This routes summary generation through `compactionModel` with
`compactionReasoning=off` by default while OpenClaw retains tool-pairing,
recent-turn, transcript, checkpoint, and rollback ownership. The provider
disables hidden model fallbacks for its isolated call; configure OpenClaw's
explicit compaction model to the same local route so its provider-failure
fallback also stays local.

Embedding workers claim bounded, expiring leases before local inference.
Memories run before references; failures back off and keep a redacted
diagnostic. `requeue-embeddings` refuses to clear vectors while another worker
has an active lease. Use `embed --provider local --drain` for a full rebuild.

## Read-only legacy trial

Import into an isolated test database. The old Moon root is read-only input:

```bash
cargo run -- --home /tmp/moon-shadow import-legacy \
  --source-home ~/.moon
cargo run -- --home /tmp/moon-shadow embed --provider hash
cargo run -- --home /tmp/moon-shadow shadow \
  --legacy-home ~/.moon \
  --query "a known prior decision" \
  --provider hash
```

Before an upgrade or embedding-model migration, create a database backup and a
generated memory export:

```bash
cargo run -- backup --destination /path/to/moon-backup.sqlite
cargo run -- export --destination /path/to/MEMORY.export.md
```

There is deliberately no legacy-delete command.

## OpenClaw integration

The adapter in [`assets/openclaw-plugin`](assets/openclaw-plugin) registers the
`moon` context engine. It retrieves a defensive Markdown packet, injects it
immediately before the current user message, records completed turns as
immutable evidence, and selectively distills durable memories with exact
citations. Greetings and irrelevant queries inject no packet. Retrieval and
learning fail open, and OpenClaw retains transcript compaction. For non-trivial
queries, the adapter also marks the matching content-free metric row as injected
or not injected and logs only its opaque request ID and numeric result summary.

Normal context is capped at 3,500 characters. Learning uses the OpenClaw primary
model and tries the configured fallback if the primary request fails. Provider
diagnostics are not copied into Moon logs. Both routes default to reasoning
`off` for low-latency structured extraction.

`primaryModel` and `fallbackModel` may override OpenClaw's routing with any
provider-qualified references, such as `vllm/local-model`, `openai/gpt-model`,
`anthropic/claude-model`, or `google/gemini-model`. `primaryReasoning` and
`fallbackReasoning` independently override their OpenClaw reasoning levels.

The adapter is installed in production from `~/.moon/openclaw-plugin`, with the
`moon` context-engine slot active. Use
[docs/openclaw-canary.md](docs/openclaw-canary.md) when validating future
releases in an isolated profile. Before changing retrieval policy, follow the
multi-day observation and acceptance gates in
[docs/memory-improvement-plan.md](docs/memory-improvement-plan.md).

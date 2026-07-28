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
- Model calls use Codex authentication without reading or copying tokens.
- Runtime directories are owner-only on Unix; databases, backups, and exports
  are created with owner-only file permissions.
- `health` never creates or migrates a missing database.
- Retrieved memory is untrusted data. Adapters must keep context below trusted
  instructions and must not promote memory text into system or developer
  instructions.

## Install or upgrade

Back up an existing Moon runtime before replacing its binary or plugin:

```bash
moon backup --destination /path/to/moon-before-upgrade.sqlite
moon export --destination /path/to/MEMORY-before-upgrade.md
```

Build and install the v2 binary:

```bash
cargo build --locked --release
install -d -m 700 ~/.moon/bin
install -m 755 target/release/moon ~/.moon/bin/moon
~/.moon/bin/moon init
~/.moon/bin/moon --json health
```

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
    "embeddingEnabled":true,
    "embeddingBatchSize":64,
    "embeddingTimeoutMs":120000
  }},
  {"path":"plugins.slots.contextEngine","value":"moon"},
  {"path":"plugins.slots.memory","value":"none"},
  {"path":"agents.defaults.memorySearch.enabled","value":false},
  {"path":"agents.defaults.compaction.mode","value":"default"}
]'
openclaw config validate
openclaw plugins doctor
openclaw gateway restart --safe
```

The first local embedding request downloads the multilingual E5 model into the
Moon runtime. See [docs/migration.md](docs/migration.md) before upgrading from
Moon v1.

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

See [docs/architecture.md](docs/architecture.md) for the storage and retrieval
contracts. Start with [docs/how-it-works.md](docs/how-it-works.md) for the
workflow-first explanation.

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

```bash
moon --home /tmp/moon-test --json auth status
printf 'Return exactly READY.' | \
  moon --home /tmp/moon-test --json auth exec \
    --model gpt-5.6-sol
```

The model path first uses an authenticated OpenClaw runtime when called by the
adapter, then a private Moon Codex login, then the normal local Codex login. Run
`moon auth login` only if you want the middle level. Moon does not parse, copy,
or store access tokens itself. The default is `gpt-5.6-sol` with high reasoning;
`gpt-5.6-luna` defaults to medium reasoning in the adapter for lower-latency
work.

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
learning fail open, and OpenClaw retains transcript compaction.

Normal context is capped at 3,500 characters. Learning uses `gpt-5.6-luna` with
medium reasoning through OpenClaw's session-bound model runtime, then the Moon
and local Codex fallbacks. No model prompt or distillation payload is placed in
process arguments.

The adapter is installed in production from `~/.moon/openclaw-plugin`, with the
`moon` context-engine slot active. Use
[docs/openclaw-canary.md](docs/openclaw-canary.md) when validating future
releases in an isolated profile.

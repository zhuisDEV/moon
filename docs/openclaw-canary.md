# OpenClaw release canary

## Purpose

The isolated canary proves the Moon retrieval boundary without changing the live
OpenClaw profile, `~/.moon`, installed binary, or active plugin. A release
canary always uses an explicit temporary runtime and OpenClaw state directory.

## Turn workflow

For each agent turn:

1. The adapter takes the explicit prompt or latest user message as the query.
2. It invokes lexical context directly or sends hybrid context over Moon's
   private stdio child with an explicit runtime, character budget, mode, and
   embedding-space dimensions.
3. Moon selects reviewed canonical memories first.
4. Remaining slots may contain unreviewed indexed references with source and
   byte citations.
5. The adapter injects the defensive packet immediately before the current user
   message.
6. If retrieval fails, the adapter preserves the original message list by
   default.
7. OpenClaw owns compaction throughout this phase.
8. After a successful completed turn, the adapter records the user request and
   final answer as immutable evidence under a stable turn fingerprint.
9. Greetings stop there. Eligible turns use Luna-medium to propose at most three
   durable memories through the Codex authentication chain.
10. Deterministic confidence, importance, exact-quote, numeric-entailment, and
    correction checks run before SQLite accepts a proposal.
11. Valid proposals are sent in one bounded batch.
12. Moon drains a bounded local-embedding batch. Active memories are processed
    before references; raw evidence is excluded.

## Reproducible checks

Build and test the engine and adapter:

```bash
cargo test --locked --all-targets --all-features
cargo build --locked --release
deno fmt --check assets/openclaw-plugin tools
deno lint assets/openclaw-plugin tools
deno test --node-modules-dir=none \
  --allow-read --allow-write --allow-env --allow-run \
  assets/openclaw-plugin/index.test.ts
```

For a real-binary adapter test, point the test at an existing isolated runtime:

```bash
MOON_TEST_BINARY="$PWD/target/release/moon" \
MOON_TEST_HOME="/path/to/isolated/moon-home" \
MOON_TEST_MODE="hybrid" \
MOON_TEST_QUERY="What model should Moon use for fast work and deep work?" \
MOON_TEST_EXPECTED="gpt-5.6-luna|gpt-5.6-sol" \
deno test --node-modules-dir=none --allow-read --allow-run --allow-env \
  assets/openclaw-plugin/index.test.ts
```

Load the plugin only through a temporary OpenClaw state directory:

```bash
profile_dir="$(mktemp -d /tmp/openclaw-moon-canary.XXXXXX)"
chmod 700 "$profile_dir"
OPENCLAW_STATE_DIR="$profile_dir" \
OPENCLAW_CONFIG_PATH="$profile_dir/openclaw.json" \
openclaw plugins install --link "$PWD/assets/openclaw-plugin"
```

Configure that temporary profile with `plugins.slots.contextEngine=moon` and
explicit `plugins.entries.moon.config.moonPath` and `moonHome` values. Then
require:

```bash
OPENCLAW_STATE_DIR="$profile_dir" \
OPENCLAW_CONFIG_PATH="$profile_dir/openclaw.json" \
openclaw config validate

OPENCLAW_STATE_DIR="$profile_dir" \
OPENCLAW_CONFIG_PATH="$profile_dir/openclaw.json" \
openclaw plugins inspect moon --runtime --json

OPENCLAW_STATE_DIR="$profile_dir" \
OPENCLAW_CONFIG_PATH="$profile_dir/openclaw.json" \
openclaw plugins doctor
```

The runtime inspection must report:

- plugin status `loaded`;
- context-engine id `moon`;
- activation from the selected context-engine slot;
- no plugin diagnostics;
- no dependencies.

## Acceptance before persistent shadow use

- Real legacy import completes without modifying the source hash.
- `health` reports schema, foreign keys, logical checks, vectors, and queue
  state as healthy.
- Known historical queries return relevant canonical memories or cited
  references instead of an empty packet.
- Active-memory and eligible-reference vector coverage both reach 100%; evidence
  vector count remains zero and dead-letter count remains zero.
- Packets stay within the configured character budget.
- Adapter success and fail-open paths pass.
- A real hybrid adapter request succeeds through the persistent stdio worker.
- The isolated OpenClaw profile validates and loads the runtime.
- Isolated validation does not change the live OpenClaw configuration or Moon
  process state.

Before the real model canary, run `moon --json auth status`. The expected
fallback order is OpenClaw, Moon, then local Codex. The adapter may fall through
only for a missing, expired, or rejected login. Confirm that a simulated rate
limit does not trigger another credential level and that prompts are sent
through stdin rather than process arguments.

Replay a selected range from an existing OpenClaw JSONL session only against an
isolated runtime:

```bash
deno run --allow-read --allow-run --allow-env \
  tools/replay_openclaw_session.ts \
  --binary "$PWD/target/release/moon" \
  --home /path/to/isolated/moon-home \
  --session-file /path/to/session.jsonl \
  --session-id isolated-acceptance \
  --from-turn 1 \
  --to-turn 3
```

The replay tool sends no Discord messages. It reports counts and lifecycle
events without printing conversation content.

Before switching a live profile from lexical to hybrid, perform one real native
compaction canary while the new adapter is installed but retrieval is still
lexical:

1. create a private, non-delivered canary session and complete at least one
   turn;
2. confirm the Moon packet is not persisted in the transcript;
3. run `openclaw sessions compact <key> --agent <agent> --json`; for the stock
   OpenClaw harness require `compacted=true`, while Codex must safely report
   `compacted=false` instead of invoking the lossy generic fallback;
4. complete a successor turn on the same key;
5. confirm the turn is recorded once, recalled memory survives, and no Moon
   packet was copied into the native summary;
6. retain `ownsCompaction=false` and `agents.defaults.compaction.mode=default`.

The offline hash provider remains suitable only for plumbing tests. Production
semantic acceptance uses the local multilingual provider and a representative
recall corpus.

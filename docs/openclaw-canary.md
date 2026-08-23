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
7. OpenClaw owns the compaction lifecycle. If `moon-local` is selected, Moon
   supplies only the summary text through OpenClaw's compaction-provider seam.
8. After a successful completed turn, the adapter records the user request and
   final answer as immutable evidence under a stable turn fingerprint.
9. Greetings stop there. Eligible turns use the configured OpenClaw primary
   model, then its provider-neutral fallback if needed, to propose at most three
   durable memories.
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
profile_root="$(mktemp -d /tmp/openclaw-moon-canary.XXXXXX)"
mkdir -m 700 "$profile_root/home" "$profile_root/state"
HOME="$profile_root/home" \
OPENCLAW_STATE_DIR="$profile_root/state" \
OPENCLAW_CONFIG_PATH="$profile_root/state/openclaw.json" \
openclaw plugins install --link "$PWD/assets/openclaw-plugin"
```

Configure that temporary profile with `plugins.slots.contextEngine=moon` and
explicit `plugins.entries.moon.config.moonPath` and `moonHome` values. Configure
provider-qualified `agents.defaults.model.primary` and at least one fallback. To
canary local summary generation, also configure a provider-qualified
`plugins.entries.moon.config.compactionModel`, set `compactionReasoning=off`,
and select `agents.defaults.compaction.mode=safeguard` with
`agents.defaults.compaction.provider=moon-local`. Keep `HOME`,
`OPENCLAW_STATE_DIR`, and `OPENCLAW_CONFIG_PATH` pointed at the same temporary
root for every command so OpenClaw cannot migrate live state. Then require:

```bash
HOME="$profile_root/home" \
OPENCLAW_STATE_DIR="$profile_root/state" \
OPENCLAW_CONFIG_PATH="$profile_root/state/openclaw.json" \
openclaw config validate

HOME="$profile_root/home" \
OPENCLAW_STATE_DIR="$profile_root/state" \
OPENCLAW_CONFIG_PATH="$profile_root/state/openclaw.json" \
openclaw plugins inspect moon --runtime --json

HOME="$profile_root/home" \
OPENCLAW_STATE_DIR="$profile_root/state" \
OPENCLAW_CONFIG_PATH="$profile_root/state/openclaw.json" \
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
- A `moon-local` canary uses an isolated raw-model session, thinking off, no
  implicit model fallback, and returns non-empty summary text.
- OpenClaw preserves complete tool-call/result pairs and recent turns around the
  resulting summary.
- Isolated validation does not change the live OpenClaw configuration or Moon
  process state.

Before the real model canary, configure provider-qualified primary and fallback
models in the isolated OpenClaw profile. Prove that the primary is used when it
succeeds, the fallback is used when the primary fails or returns invalid
structured output, and both default `thinkLevel` values are `off`. Verify
independent reasoning overrides and prove that neither provider's raw failure
body reaches Moon logs. The Moon binary must not own or inspect any provider
credential store.

Before switching a live profile from lexical to hybrid or selecting
`moon-local`, perform one real compaction canary while the new adapter is
installed but retrieval is still lexical:

1. create a private, non-delivered canary session and complete at least one
   turn;
2. confirm the Moon packet is not persisted in the transcript;
3. run `openclaw sessions compact <key> --agent <agent> --json`; for the stock
   OpenClaw harness require `compacted=true`, while Codex must safely report
   `compacted=false` instead of invoking the lossy generic fallback;
4. complete a successor turn on the same key;
5. confirm the turn is recorded once, recalled memory survives, and no Moon
   packet was copied into the native summary;
6. retain `ownsCompaction=false`; use `mode=default` without a custom provider,
   or `mode=safeguard` with `provider=moon-local` so OpenClaw keeps structural
   ownership.

The offline hash provider remains suitable only for plumbing tests. Production
semantic acceptance uses the local multilingual provider and a representative
recall corpus.

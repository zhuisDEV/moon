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
5. With conversation-hook access granted, the adapter returns the defensive
   packet through `before_prompt_build` as `prependContext`, and returns
   messages unchanged from context-engine assembly. Otherwise it uses the legacy
   assembly injection immediately before the current user message.
6. If retrieval fails, the adapter preserves the original message list by
   default.
7. OpenClaw owns the compaction lifecycle. If `moon-local` is selected, Moon
   supplies only the summary text through OpenClaw's compaction-provider seam.
8. After a successful completed turn, the adapter records the user request and
   final answer as immutable evidence under a stable turn fingerprint.
9. Greetings stop there. Eligible turns use the L1 model, then its enabled
   provider-neutral fallback if needed, to propose a bounded number of memories,
   three by default.
10. Deterministic confidence, importance, exact-quote, numeric, uncertainty,
    correction, and freshness checks run before SQLite accepts a proposal.
11. Valid proposals are sent in one bounded, atomic batch. A rejected proposal
    leaves all memories from that batch unchanged.
12. Moon drains a bounded local-embedding batch. Active memories are processed
    before references; raw evidence is excluded.

When explicitly enabled, L2 also runs in the plugin service at its daily IANA
time. It prepares a leased snapshot of pending evidence and related claims with
original citations, applies supported changes atomically, and retains ambiguous
conflicts for review. The default configuration leaves L2 disabled. Read
[learning.md](learning.md) for date keys, DST, catch-up, partial-context, and
retry semantics.

## Reproducible checks

Build and test the engine and adapter:

```bash
cargo test --locked --all-targets --all-features
cargo build --locked --release
deno fmt --check assets/openclaw-plugin tools
deno lint assets/openclaw-plugin tools
sh tools/test-openclaw-adapter.sh "$PWD/target/release/moon"
```

The helper creates and removes a temporary synthetic corpus, requires the real
binary retrieval and SQLite retry tests, and checks that ambient Moon
environment overrides cannot redirect storage. CI runs the same helper against
its Linux release artifact. It makes no provider or OpenClaw gateway calls.
Without an explicit test binary and home, direct Deno runs mark those four
integration tests as ignored; `MOON_REQUIRE_REAL_BINARY=1` makes missing
configuration a failure.

The focused learning and native-probe fixture checks also make no model calls:

```bash
cargo test --locked --test cli_config --test cli_learning
deno test --no-config --no-remote --node-modules-dir=none \
  tools/probe-codex-route.test.ts
```

Review the isolated
[learning CLI rehearsal](learning.md#inspect-and-rehearse-with-the-cli) with the
newly built binary. Require strict config validation, independent
model/effort/fallback inheritance, disabled-L2 no-call behaviour, and bounded
prompt-file handling. Prepare a preview before creating any lease. Verify atomic
dry-run/apply, stale-snapshot rejection, exact evidence, question and negation
handling, observation expiry from source time, same-content merges, review
preservation, interrupted lease recovery, DST date keys, and exhausted retry
behaviour. These checks use temporary homes and synthetic evidence, not the live
Moon database or provider authentication.

For a real-binary adapter test, point the test at an existing isolated runtime:

```bash
MOON_TEST_BINARY="$PWD/target/release/moon" \
MOON_TEST_HOME="/path/to/isolated/moon-home" \
MOON_REQUIRE_REAL_BINARY=1 \
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
provider-qualified `agents.defaults.model.primary`. Configure a fallback only
when it is part of the authorised test; `fallback_enabled=false` in a stage must
prevent inherited fallback calls. Put that profile's `moon.toml` in its explicit
temporary `moonHome`, and leave L2 disabled while inspecting previews. To canary
local summary generation, also configure a provider-qualified
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

## Native recall compatibility

OpenClaw 2026.9.7 deliberately omits context-engine assembly for native-owned
models. Moon's completed-turn recording still runs, so healthy storage and
growing evidence can coexist with absent recall. Moon uses the supported
`before_prompt_build` hook when the host exposes it and conversation access is
explicitly granted. The same path handles stock runtimes, with `assemble`
passing messages through to avoid injecting twice.

For the isolated profile above, merge these keys into the existing config:

```json
{
  "plugins": {
    "slots": { "contextEngine": "moon" },
    "entries": {
      "moon": {
        "hooks": {
          "allowConversationAccess": true,
          "allowPromptInjection": true
        }
      }
    }
  }
}
```

OpenClaw treats these as plugin permissions, outside Moon's `config` object.
Deploying this grant to a live profile requires operator approval; Moon does not
write it automatically. Without permission or hook support, the adapter warns
and retains legacy assembly recall only where the host invokes it. A loaded
plugin is not proof of working native recall.

Use synthetic evidence in the temporary Moon home. Require exactly one retrieval
for a stable native admission, a non-empty packet returned to the prompt hook,
no duplicate assembly injection, and no retrieval on heartbeat or Moon helper
turns. An explicit empty `currentUserMessage` must not retrieve from
reconstructed history. Check cancellation and hook-lifetime expiry before
accepting delivery metrics. The adapter tests exercise these boundaries without
provider calls; deployment acceptance also needs a permitted real native turn
that recalls a unique synthetic fact. An adapter injection metric records its
handoff, not proof that the final model consumed or used the packet.

Learning needs a separate helper-route canary. OpenClaw 2026.9.7 treats any
non-empty `streamParams` as a provider transport override and excludes native
Codex. Moon therefore omits the advisory output-token hint for native runtimes
and implicit OpenAI routing. Explicit stock runtimes and ordinary local routes
retain the hint. Verify the exact L1 and L2 model/effort routes with isolated
synthetic evidence when inference is authorised; a direct Codex probe alone does
not exercise the OpenClaw helper boundary. Do not add credentials or change
model IDs to make a failed route pass.

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
- L1 and L2 retain separate configured models and effort, with no hidden
  provider fallback or native tool access.
- L2 previews identify omitted comparison context; reviews expose original
  evidence IDs without claiming every conflict has been resolved.
- Schema-8 rehearsal preserves immutable evidence, unique active heads,
  citations, review information, expiry, and embedding-queue integrity.

Keep failure-injection and routing assertions in offline fixtures. Prove that an
enabled fallback follows a failed primary, a disabled fallback is never called,
and the Astra starter passes L1 `low` and L2 `xhigh` independently. Existing
valid inherited effort must survive an absent TOML override. Neither provider's
raw failure body may reach Moon logs. The Moon binary must not own or inspect
any provider credential store. A real request is a separate check of the
selected host/account route, not evidence that the fixture's model output is
representative.

## Native Codex route check

The native route uses the Codex executable configured by OpenClaw and the
existing user's `CODEX_HOME` OAuth. In the verified setup the native plugin owns
the `openai` provider. Do not create a replacement login, copy an auth file into
a temporary home, or introduce an API-key route for this test.

From the repository root, the discovery tool starts the selected app binary and
reports its resolved path/version, native home, auth mode, and advertised model
efforts. It does not submit a model turn or print account identities, tokens,
RPC bodies, or arbitrary native errors. The macOS example below uses the
verified app path; on another host, use the exact executable configured by
OpenClaw and adjust the read/run permissions accordingly:

```bash
codex_binary="/Applications/ChatGPT.app/Contents/Resources/codex"
native_codex_home="${CODEX_HOME:-$HOME/.codex}"
deno run --no-config --no-remote --node-modules-dir=none --no-prompt \
  --allow-env=PATH,HOME,CODEX_HOME \
  --allow-read="/Applications,$native_codex_home" \
  --allow-run="$codex_binary" \
  tools/probe-codex-route.ts --codex "$codex_binary"
```

Compare the reported binary and native home with OpenClaw's Codex plugin
configuration, including `homeScope: "user"`. Require `auth_mode: "chatgpt"` and
advertised `low` and `xhigh` Astra efforts. Catalog discovery alone does not
prove inference works.

Only after separately authorising two subscription-backed synthetic turns, add
the smoke option and its temporary-directory permissions:

```bash
deno run --no-config --no-remote --node-modules-dir=none --no-prompt \
  --allow-env=PATH,HOME,CODEX_HOME \
  --allow-read="/Applications,$native_codex_home,/tmp" --allow-write=/tmp \
  --allow-run="$codex_binary" \
  tools/probe-codex-route.ts --codex "$codex_binary" --smoke --timeout-ms 60000
```

The smoke path allows at most two turns: Astra `low`, then `xhigh`, each asked
for the same JSON constant. It uses fresh ephemeral threads in its own temporary
working directory, disables tools and MCP, rejects route changes, and reports
only the constant, effort, duration, and available numeric token counters. The
existing native global instructions may still be loaded and are reported as a
boolean; no Moon evidence or saved conversations are supplied. Unknown native
versions or managed policy require review instead of bypassing the probe's
isolation checks. A failure reports how many turns were attempted; do not
blindly repeat a possibly dispatched turn.

Passing this probe confirms native authentication, supported effort, and basic
execution. It does not prove that the OpenClaw plugin is configured to use that
same binary without the configuration comparison, and it does not measure L2
accuracy or representative synthesis speed. The inspected OpenClaw 2026.9.4
native Codex backend ignores `max_output_tokens`; timeout and attempt bounds do
not provide an exact subscription-token cap.

## Compaction and deployment gates

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

Before deploying Moon 2.6.1 or newer, back up the database and configuration,
rehearse schema 8 on a copy, and retain the prior binary with its matching
database backup. The release schema range is 6–8. Do not change live
configuration, install the adapter, restart the gateway, or enable daily L2
merely because source tests pass. Those are deployment actions requiring their
own authorised scope. `moon.toml` remains in the runtime root across release
switches.

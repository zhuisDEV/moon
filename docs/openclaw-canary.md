# OpenClaw release canary

## Purpose

The isolated canary proves the Moon retrieval boundary without changing the live
OpenClaw profile, `~/.moon`, installed binary, or active plugin. A release
canary always uses an explicit temporary runtime and OpenClaw state directory.

## Turn workflow

1. OpenClaw identifies the current turn, its model and its audience. The main
   model uses the current request and up to two already-visible verified
   excerpts to decide whether to recall saved facts, request original messages,
   ask for clarification or answer directly.
2. Moon checks the requested sources against the audience grant. An opt-out
   causes zero stored-memory reads. A verified DM with a saved-facts-only grant
   can retrieve eligible facts. On that route, an original-message request
   produces an unavailable notice.
3. Moon runs up to three saved-fact searches under one budget. It calls the CLI
   for lexical retrieval and its private stdio worker for hybrid retrieval.
   Original-message searches default to the current conversation.
4. When there are eligible candidates, the main model selects supporting IDs.
   Moon rechecks private access, then sends the exact selected records through
   `before_prompt_build`. An empty selection produces an unavailable notice. The
   context engine passes messages through when that hook is active. Stock legacy
   assembly requires a host-resolved model and only supplies explicitly
   published records.
5. OpenClaw owns transcript compaction. With `moon-local` selected, Moon
   supplies the requested summary through the host's compaction-provider hook.
6. After an eligible turn, Moon records the accepted human message as private
   evidence under the verified speaker. A confirmed Discord reply retains its
   own delivery-time owners.
7. L1 proposes up to three memories by default from an independently supported
   human turn. Moon verifies evidence, numeric literals, target freshness and
   score bounds. SQLite commits the valid batch atomically and Moon drains a
   bounded local embedding batch.

When explicitly enabled, L2 also runs in the plugin service at its daily IANA
time. It prepares a leased snapshot of pending evidence and related claims with
original citations, applies supported changes atomically, and retains ambiguous
conflicts for review. The default configuration leaves L2 disabled. Read
[learning.md](learning.md) for date keys, DST, catch-up, partial-context, and
retry semantics.

## Check a running installation

The health probe inspects the configured Gateway and its Moon database without
sending any messages or starting a model run. Use the binary and adapter from
the same release when comparing production and the probe.

```sh
deno run --no-prompt --allow-run tools/check-openclaw-moon.js
```

The health report includes Gateway and Discord connectivity, Codex OAuth, Moon
integrity, turn admissions and the current scheduled L2 batch. Pass
`--since <ISO timestamp with timezone>` to count only turns since an upgrade.
Successful retrievals with zero verified admissions mean those turns supplied no
new private evidence for L1. An empty L2 window leaves no durable receipt. An
exhausted scheduled batch remains visible even if a separate operator run later
succeeds. Inspect `scheduled_batches` and `retry_pending` for those cases.

The isolated adapter suite below verifies private recall after L1, duplicate
suppression, shared profile ownership and the effect of another bot's room
permissions. On the development checkout, use the binary built from that same
checkout. The tests exercise real Moon SQLite storage with synthetic Gateway
records and model responses. They also advance a simulated Sydney clock across
03:00 and restart the service to check that L2 commits the due batch once.

The 2.9.1 release rehearsal used real OpenClaw model calls and temporary SQLite
databases with simulated Discord admissions and delivery receipts. Sol recalled
the notebook label in the owner DM case and answered “unknown” for the shared
room. A memory opt-out made zero saved-fact or history reads. Terra learned two
confirmed standing defaults. Ambiguous and undelivered proposals produced no
standing default.

For a configured Discord guild text room, run the following with the path to the
installed OpenClaw package. The probe checks the current permissions using
Discord's read-only endpoints and the installed Moon verifier. It can choose the
room and sender automatically when both are unambiguous.

```sh
node tools/check-discord-learning.mjs /path/to/openclaw
```

Exit 0 means the room audience was verified. Exit 1 means Moon declined private
learning there. Exit 2 means the check could not finish.

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

For a real-binary adapter test against an existing isolated runtime, publish a
synthetic public fact in that runtime first. The following example expects the
fact “The coral compass aligns the telescope”.

```bash
MOON_TEST_BINARY="$PWD/target/release/moon" \
MOON_TEST_HOME="/path/to/isolated/moon-home" \
MOON_REQUIRE_REAL_BINARY=1 \
MOON_TEST_MODE="hybrid" \
MOON_TEST_QUERY="What aligns the telescope?" \
MOON_TEST_EXPECTED="coral compass" \
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
and can provide published records through legacy assembly when the host supplies
the resolved model identity. Both recall paths need the active agent and model
route. Test a fresh native turn to confirm prompt delivery.

Use an explicitly published synthetic fact for a public route. Exercise private
evidence with a verified Discord audience, then confirm that adding a non-owner
to the destination excludes the record. Require exactly one retrieval for a
stable native admission, a non-empty packet returned to the prompt hook, no
duplicate assembly injection, and no retrieval on heartbeat or Moon helper
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

Before deploying Moon 2.9.2, back up with the installed pre-upgrade binary,
retain configuration and rehearse schema 21 on a copy. Preserve the prior binary
with its matching database backup. The signed release accepts schemas 6–21.
Rollback to 2.9.0 requires its schema-20 backup. Follow the
[upgrade checklist](updating.md#upgrade-checklist) and refresh the changed
skill. Do not change live configuration, install the adapter, restart the
gateway, or enable daily L2 merely because source tests pass. Those are
deployment actions requiring their own authorised scope. `moon.toml` remains in
the runtime root across release switches.

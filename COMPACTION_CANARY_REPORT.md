# Moon local compaction canary report

Date: 2026-08-23 (Australia/Sydney)

## Scope

This report records the isolated OpenClaw canary used to configure Moon as the
summary provider for safeguard compaction with the local
`vllm/qwen3.8-27b-uncensored-fp8` model. It focuses on the initial 4,096-token
run that exceeded the 180-second deadline, the root cause, and whether the
timeout should be increased.

No production transcript was used. The canary ran in a temporary OpenClaw
profile and temporary Moon home, without channel delivery. Credentials and raw
provider bodies are excluded from this report.

## Executive conclusion

The timeout was **not a model warm-up problem**.

The model had already completed a canary turn, and the vLLM server began the
compaction response within milliseconds. The failure occurred because OpenClaw
did not yet have the Qwen-specific vLLM thinking format configured. As a result,
`thinkLevel=off` did not produce the required
`chat_template_kwargs.enable_thinking=false` request shape. The model spent its
generation budget in hidden reasoning, OpenClaw saw reasoning-only turns, and
the compaction reached the 180-second deadline without a usable visible summary.

The 4,096-token allowance increased the amount of work the faulty request could
perform, but it was not the root cause. After configuring
`compat.thinkingFormat=qwen-chat-template`, a 1,024-token compaction completed
successfully in approximately 19.4 seconds and a successor turn recovered the
preserved identifiers exactly.

Recommendation: **keep the timeout at 180 seconds**. Increasing it now would
make genuine failures slower without improving successful compaction latency.

## What happened

| Stage                | Configuration                                           | Observed result                                                                                                             | Interpretation                                                                       |
| -------------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| Baseline canary turn | Local Qwen, thinking requested off                      | vLLM began responding in about 33 ms; the turn completed in about 22 seconds                                                | The server and model were already available before compaction                        |
| First compaction     | 4,096 output tokens, 180-second timeout                 | vLLM began responding in about 8 ms; the run was aborted at about 180.0 seconds; `compacted=false`                          | Not cold start; the model generated without producing a usable visible summary       |
| Reduced allowance    | 1,024 output tokens                                     | OpenClaw detected reasoning-only output, retried visible-answer continuation, and still reached the 180-second deadline     | Reducing the cap alone did not disable hidden reasoning                              |
| Diagnostic minimum   | 512 output tokens                                       | Three reasoning-only attempts ended after about 118.7 seconds and surfaced OpenClaw's failure sentinel instead of a summary | Confirmed incorrect thinking request shaping; also exposed a provider validation gap |
| Corrected canary     | `qwen-chat-template`, thinking off, 1,024 output tokens | Compaction completed in about 19.4 seconds with `compacted=true`                                                            | Qwen thinking was now genuinely disabled                                             |
| Successor turn       | Same isolated session                                   | Returned `call_abc123` and `/tmp/moon-canary` exactly in about 5.8 seconds                                                  | The compacted continuation retained the requested identifiers                        |

The temporary 512-token prompt experiment was removed. The final design relies
on OpenClaw's native Qwen request shaping rather than a model-specific prompt
trick.

## Root cause

The local model is marked as reasoning-capable. OpenClaw's vLLM integration
needs the model compatibility field below to translate a thinking level into the
request format expected by a Qwen chat template:

```json
{
  "compat": {
    "thinkingFormat": "qwen-chat-template"
  }
}
```

Without that field, the adapter still passed `thinkLevel=off`, but the vLLM
request was not patched with `chat_template_kwargs.enable_thinking=false`.
OpenClaw therefore received reasoning content without a visible final answer.

The evidence against warm-up is strong:

1. A normal model turn completed before the first compaction attempt.
2. The first compaction HTTP stream began in about 8 ms.
3. Later reduced-token attempts reproduced the same reasoning-only behaviour.
4. The behaviour changed immediately after the compatibility field was added,
   without changing the model or increasing the timeout.

A genuinely cold model can add latency after a service restart, but that was not
the observed failure mode here.

## Why the 4,096-token allowance mattered

`compactionMaxTokens` is an output ceiling, not the model context window. A
4,096-token ceiling gave the incorrectly thinking model more room to consume
time before stopping. It did not mean that a valid summary required 4,096
tokens.

The current 1,024-token ceiling is a better starting point for this local 27B
model. OpenClaw separately preserves the most recent three turns and appends its
own structural sections for split-turn context, tool failures, file operations,
and workspace context. Those safeguards reduce the amount Moon's summary body
must carry.

## Timeout recommendation

Keep both current deadlines at 180 seconds:

- `models.providers.vllm.timeoutSeconds = 180`
- `plugins.entries.moon.config.compactionTimeoutMs = 180000`

The successful canary used about 19.4 seconds, leaving more than nine times that
duration as headroom. A timeout increase would not make compaction faster; it
would only wait longer when the model, provider, or request shaping is broken.

Consider a temporary increase to 240 or 300 seconds only if all of the following
are true:

1. logs confirm `qwen-chat-template` is active and thinking is off;
2. the model returns visible summary text rather than reasoning-only output;
3. the slowdown occurs on representative long transcripts rather than a
   synthetic failure;
4. the local GPU is not contending with another large workload; and
5. measured compaction repeatedly approaches 150 seconds despite the 1,024-token
   ceiling.

If the timeout is increased, both the Moon compaction timeout and the vLLM
provider timeout must be reviewed. Raising only Moon's timeout cannot extend a
request that the provider transport aborts at 180 seconds.

## Changes made

- Added the optional `moon-local` compaction summary provider.
- Kept `ownsCompaction=false`; OpenClaw retains transcript mutation, tool-pair
  boundaries, recent-turn preservation, quality checks, checkpoints, and
  rollback.
- Corrected Moon model effort routing to use OpenClaw's `thinkLevel` field.
- Disabled the model fallback chain inside Moon's summary request.
- Added isolated raw-model sessions with no tools or Moon context injection.
- Rejected empty, error, and OpenClaw failure-sentinel model responses so they
  cannot become compaction summaries.
- Configured Qwen's vLLM compatibility as `qwen-chat-template`.
- Set the live Moon summary ceiling to 1,024 tokens and retained the 180-second
  deadline.
- Enabled strict identifier preservation, three recent turns, safeguard mode,
  the quality guard, and mid-turn precheck.

## Current live configuration

```text
OpenClaw compaction mode:       safeguard
OpenClaw compaction provider:   moon-local
Compaction model:               vllm/qwen3.8-27b-uncensored-fp8
Moon compaction reasoning:      off
Qwen thinking format:           qwen-chat-template
Moon compaction max tokens:     1024
Moon compaction timeout:        180000 ms
vLLM provider timeout:          180 seconds
Identifier policy:              strict
Recent turns preserved:         3
Quality guard:                  enabled, one retry
Context window:                 200000 tokens
Reserve floor:                  20000 tokens
```

## Validation evidence

- Deno format and lint passed.
- All 27 adapter tests passed, including error-payload rejection and isolated
  reasoning-off routing.
- The full Rust suite passed 88 tests.
- The isolated real-model compaction returned `compacted=true` in about 19.4
  seconds.
- The successor turn recovered the exact marker and path.
- The deployed plugin reports version `2.4.2-local.1`, status `loaded`, no
  diagnostics, and the `moon` context engine active.
- The restarted Gateway is active and reachable.
- Moon schema 7 health reports SQLite integrity OK, zero logical violations, and
  no pending, failed, or dead embedding jobs.

## Follow-up updates

1. Add a redacted preflight warning when a reasoning-capable vLLM Qwen model is
   selected with thinking off but lacks a supported `compat.thinkingFormat`.
2. Record content-free compaction-provider latency, outcome, and output-token
   metrics so slowdowns can be distinguished from warm-up or contention.
3. Observe the first few representative long-session compactions before changing
   the 1,024-token ceiling or 180-second deadline.
4. Commit the current repository changes and publish a signed Moon release so a
   future `moon update` cannot replace the local adapter with the stock 2.4.2
   plugin.

# Isolated source-learning model canary

For installation and ordinary upgrades, start with
[Getting started](getting-started.md) or [Updating](updating.md). This advanced
operator check ships in the tagged source, not the six-file 2.8.1 binary bundle.
Install the updated [skill and guides](updating.md#refresh-the-agent-skill) with
the release.

`tools/source-learning-canary.ts` is a versioned operator tool for validating a
candidate binary and OpenClaw adapter before deployment. Its default stage is
zero-inference preflight. It tests a fixed synthetic Aster fixture, never live
Moon memories. Moon remains one Rust binary and one SQLite database; this tool
uses Deno to supervise the existing OpenClaw Node host, without adding a Moon
runtime dependency or a Node package-manager workflow.

## Preconditions and boundary

- Build and freeze a candidate binary and adapter outside installed releases.
- Choose a new canonical absolute `REPO/target/moon-canary-NAME` home. The tool
  refuses foreign homes, symlinked homes, ambient database redirects and changed
  candidate hashes on resume.
- Use the existing authorised OpenClaw account. The reviewed OpenClaw versions
  are 2026.11.3 and 2026.11.5; other versions fail closed until their SDK is
  reviewed.
- Catalogue discovery uses public SDK exports. The reviewed host also needs an
  exact execution-owner lease that its public SDK does not expose: a narrow
  compatibility bridge discovers its unique named export, checks capabilities
  and pins the host version. Missing or ambiguous exports fail closed; hashed
  module filenames are never hardcoded. Preflight checks this preparation
  without inference. It does not prove a subsequent model call will succeed.
- Real calls require `--smoke` on `l1` or `l2`. They use `openai/gpt-6.1-terra`,
  medium for L1 and max for L2, with host fallback disabled. They consume the
  existing account's model allowance. They are not CI tests.
- Automatic Moon services and chat channels do not start. Model tools/messages
  are disabled; sessions are detached/incognito. The real host retains its
  normal permissions: Deno's supervisor permissions do not sandbox the Node
  child.
- The tool does not alter login policy, copy credentials or print arbitrary host
  errors. Reports record safe codes and whether native Codex authentication
  remained unchanged, without its hash or contents.

## Run and review

Run `deno run --no-config tools/source-learning-canary.ts --help` for the exact
arguments and scoped permission template. Resolve Node's executable symlink
before supplying `--node` and `--allow-run`. Read permission covers candidate,
tools, host metadata and the existing model cache; write permission covers only
the selected canary home. No `--allow-all` is needed. Use the same invocation
prefix and home for each stage:

The `verify` stage exercises the adapter's Node-compatible stdio worker. Deno
requires environment enumeration for that spawn: run verification under `env -i`
with only PATH/HOME/LANG supplied and replace the scoped environment flag with
`--allow-env`. Keep the same scoped read/write/run permissions. Do not grant
broad environment access to an unsanitised parent shell. Model stages continue
using the scoped allowlist and existing host account.

1. Supply `--binary`, `--plugin`, `--host`, `--host-state`, `--host-config`,
   `--agent-dir`, `--codex-plugin` and `--node`. The default `preflight` must
   pass with zero reserved/completed inferences.
2. `--stage prepare` creates two synthetic eligible sources in the isolated DB.
3. `--stage l1 --smoke` makes at most two L1 calls and leaves quotes in review.
4. `--stage review` writes `review-proposals.json`. Inspect all proposed quotes,
   original wording, negations, qualifications and exact source citations. Only
   then run `--stage accept-l1 --approve HASH1,HASH2` using the exact result
   hashes. No automatic semantic check constitutes this review.
5. `--stage l2 --smoke` makes at most one L2 topic call. Run `review` again and
   inspect every fact/interpretation/question and supporting input ID. Only then
   run `--stage accept-l2 --approve HASH`.
6. `--stage verify --model-cache ABSOLUTE_CACHE` checks lexical and actual local
   embedding hybrid recall, both independent source premises, exact original
   citation ranges, and withdrawal invalidation. The registered prompt hook must
   recall an explicitly published synthetic fact while excluding the operator
   sources and topic from chat.

Each model dispatch reserves its budget durably before execution. A home has at
most three reservations, including failures and uncertain interrupted calls;
resuming cannot replay a reserved source/topic call. Investigate failures and
obtain authorisation before starting another paid test run. Stage deadlines,
bounded output and owned-child cleanup prevent a stuck host from leaving the
operator tool waiting indefinitely. The tool records process cleanup outcomes;
it never kills processes by matching names.

State checkpoints are flushed before dispatch. An exclusive operator lock is
acquired before reading resume state and held through child cleanup. A crashed
supervisor may leave a lock: verify that its recorded owner has stopped before
removing that canary-local lock. The tool never treats a stale lock as
permission to replay an uncertain inference.

Artifacts remain in the private canary home: state/checkpoints, safe host
reports, review proposals, accepted and post-withdrawal DB snapshots, and the
final validation report. Preserve them with the candidate hashes as deployment
evidence. A failed assertion remains a failure; do not change the fixture or
discard unsupported statements merely to obtain a passing report.

## What a pass establishes

A pass proves this candidate's reviewed synthetic source-to-topic flow works
through the actual registered host API. Returned provider/model must match the
request. The host may report the backend as `openclaw` or `codex`; neither is
assumed. Missing effort/fallback observations stay `null`, and a confirmed
unexpected fallback or model identity fails the test. Requested medium/max
reasoning is not independently observed transport effort.

Model stages use the registered OpenClaw runtime API. The prompt delivery check
directly invokes the adapter's registered hook in an isolated test API and
compares it with independently rendered CLI output; it is not a live Gateway
chat turn.

This is not a general quality benchmark, live database migration, Gateway chat
delivery test, independent Moon login implementation or execution of the next
scheduled daily session-L2 run. Deployment remains separately authorised and
requires live database/configuration backups and post-cutover checks.

The offline contract tests use no permissions, authentication or model calls:

```sh
deno test --no-config --no-remote tools/source-learning-canary.test.ts
```

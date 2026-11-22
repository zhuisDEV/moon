# Native signed updates

First installation? Start with [Getting started](getting-started.md). This guide
is for an existing Moon runtime. For the data changes, see
[Migration](migration.md).

## Upgrade checklist

Moon **2.9.2** accepts **schemas 6–21** and migrates to **schema 21**. To roll
back to 2.9.0, restore its matching schema-20 backup. On the first audience
migration, old global memories enter operator review and the public collection
starts empty. The operator may explicitly assign the frozen legacy collection to
one verified owner when its ownership is known. The assignment preserves
archived records and leaves historical authorship unverified. See the
[legacy memory procedure](memory-privacy.md#reviewing-the-old-global-collection).

Moon filters what it supplies to a chat. OpenClaw session history, shell and
filesystem tools have their own access controls. Review those permissions for
shared agents as part of the cutover. Existing model settings are preserved.

1. Confirm the current binary and runtime before changing anything:

   ```bash
   command -v moon
   moon --json --version
   moon --json health
   openclaw --version
   ```

   Use the canonical path reported by Moon. If health reports a newer schema
   than the invoked binary supports, stop and resolve the executable mismatch.
2. Take a consistent backup using the **currently installed pre-upgrade
   binary**. Retain the old executable, Moon configuration and private OpenClaw
   configuration. The native updater also makes its own quiesced rollback
   bundle. For a separate precautionary copy, replace these destinations with
   new private paths:

   ```bash
   ~/.moon/bin/moon backup --destination /private/backup/moon-before-2.9.2.sqlite
   ~/.moon/bin/moon export --destination /private/backup/MEMORY-before-2.9.2.md
   ```

   Create the destination directory with mode `0700`. Copy `~/.moon/moon.toml`
   when present and the file reported by `openclaw config file` with mode
   `0600`. Do not run a newly built binary's `backup` against an older live
   database: write commands open and migrate storage first. An export is not a
   full backup.
3. Review the signed plan, then apply when ready for a Gateway restart:

   ```bash
   ~/.moon/bin/moon update --version=2.9.2 --check
   ~/.moon/bin/moon update --version=2.9.2 --dry-run
   ~/.moon/bin/moon update --version=2.9.2
   ```

   Read the plan and confirmation. If active leases are reported, allow work to
   finish; do not clear leases or kill work to force the update. If recovery is
   required, follow [Apply](#apply) before retrying.
4. Check the result:

   ```bash
   ~/.moon/bin/moon --json --version
   ~/.moon/bin/moon --json health
   openclaw config validate
   openclaw gateway status
   openclaw plugins info moon --json
   openclaw plugins doctor
   ```

   Expect version `2.9.2`, schema `21`, integrity `ok`, zero logical violations
   and the loaded adapter from the selected release. Verify a fresh conversation
   and current injection evidence separately from storage health. See
   [verification](getting-started.md#verify-the-installation).
5. **Refresh your skill and its guides** as described below. Retain backup,
   journal and old release until recovery is no longer needed.

### Six-file compatibility bridge

Moon 2.9.2 retains the six signed payloads introduced by the 2.7.0 bridge. The
published 2.6.6 updater accepts this archive shape. The binary, adapter files
and `SKILL.md` are in the signed archive. Install the supporting Markdown guides
from the matching release tag. For local development builds, check both the
commit reported by `moon --json --version` and the database schema.

## Refresh the agent skill

**`SKILL.md` changed in 2.9.2. Update it wherever you previously copied it.**
The native updater refreshes its managed OpenClaw skill; it does not find or
update separate Codex/other-agent copies. Verify the installed
`moon-version: 2.9.2` marker. The updated skill has pinned online links so it
works without local guides.

To install the complete guide set, use a reviewed checkout of the **same tag**:

```bash
git clone --branch v2.9.2 --depth 1 https://github.com/zhuisDEV/moon.git moon-2.9.2-guides
cd moon-2.9.2-guides
```

For an existing skill, first preserve its entire directory in a new private
backup location and compare local customisations. Then, from that checkout:

```bash
install -d -m 700 "$HOME/.openclaw/skills/moon" "$HOME/.openclaw/skills/moon/docs"
install -m 644 SKILL.md "$HOME/.openclaw/skills/moon/SKILL.md"
install -m 644 docs/*.md "$HOME/.openclaw/skills/moon/docs/"
```

Use the actual configured skill destination if different. Repeat for each
separately installed agent copy. Copy matching files together, preserve
unrelated local files, and restart or reload the agent session so it reads the
updated instructions. Copying the skill changes instructions only; it neither
upgrades the binary nor migrates the database. Do not replace skills during an
active update transaction or recovery.

## Transaction reference

Moon v2.2.0 introduced the native updater. Use v2.2.1 or later for the initial
bootstrap; v2.2.1 preserves the embedding identity already stored by v2
databases. The updater changes the binary, OpenClaw adapter, agent skill, and
database schema as one compatibility-set transaction. It never compiles remote
source, evaluates manifest fields as shell, downloads trust roots, deletes prior
releases, or copies credentials.

## Source/topic hardening in 2.7.0

The schema-13 release extends this transaction to source-processing and topic
compilation jobs. Preflight rejects active source, topic and session-learning
leases. After the host worker stops, the updater snapshots job columns, row
counts and content hashes; post-migration checks compare the original columns
and rows so additive schema changes are permitted but lost or rewritten jobs are
not. Queued, pending-review and completed jobs must survive. Recovery also
checks leases before replacing an intact database; a corrupt failed-migration
database remains restorable after workers have stopped.

Before restarting the candidate host, the updater fingerprints a consistent
SQLite backup that includes committed WAL pages. If validation later fails, Moon
locks the candidate database, compares the full fingerprint and holds the lock
through restoration. Any database change, including an operator write, stops
restoration for review. An unreadable post-restart database or an older journal
missing the full fingerprint also requires operator recovery. Migration failures
before the candidate starts can still restore the verified backup.

Signed bundles may include up to 32 regular `skill/docs/*.md` files, each at
most 4 MiB, in addition to the existing required payload. The archive manifest
binds their paths, modes, sizes and hashes. Old six-file bundles remain valid.
The updater backs up and stages the complete installed skill directory, overlays
the candidate's signed files, and restores candidate-managed paths during
rollback. It retains unrelated local files and old documents absent from the
candidate; it cannot assume those files are disposable. Conflicting later edits
to managed paths stop restoration for review rather than being overwritten.

An older updater whose manifest parser requires exactly six payload files cannot
read a documentation-extended release. Publish a signed bridge built with
`moon-release bundle --legacy-skill-only` first, containing the updated reader
but the old six-file payload shape; a subsequent signed release can include the
documents. This switch changes packaging only and never bypasses signature
verification. New readers continue accepting old six-file releases. A local
development deployment follows its separately approved backup/cutover procedure.

These changes retain the existing signed-release and journalled recovery
requirements. They do not add an unsigned installer for development builds.
Deploying a build still requires approval and database/configuration backups.
Use the [isolated model canary](source-learning-canary.md) before cutover; it is
separate from the updater's offline validation.

## Moon 2.6.4 native recall upgrade

Moon 2.6.4 fixes OpenClaw 2026.9.7 learning helpers losing their native route
because of an advisory output-token hint. It also adds prompt-build recall for
native model routes that bypass context-engine assembly. Database schema stays
at 8.

Updating the adapter alone does not enable the new recall hook. Keep
`plugins.slots.contextEngine="moon"` and explicitly grant
`plugins.entries.moon.hooks.allowConversationAccess=true`, with
`allowPromptInjection` allowed. These are OpenClaw plugin permissions, outside
Moon's `config` object, and require operator approval for a live profile. Moon
does not change them automatically. Without the grant, legacy assembly recall
continues only on runtimes that invoke it.

Back up the database and OpenClaw configuration before deploying. Follow the
[native recall canary](openclaw-canary.md#native-recall-compatibility) for the
exact permission patch and isolated checks. After an authorised deployment,
verify a new native turn recalls a unique synthetic fact and that the exact
configured L1 and L2 model/effort routes succeed. A direct Codex probe, healthy
database, or old injection metric does not complete those acceptance checks.

## Read-only inspection

Moon 2.5.3 fixes the live-turn fallback seen with 2.5.2 on OpenClaw 2026.9.2.
From 2.5.2, use `moon update --version=2.5.3`; no gateway recovery helper or
additional configuration migration is needed. After updating, check a real
conversation: Moon context metrics should increase for non-trivial requests, and
gateway logs must no longer report
`current-turn transcript fencing is not
declared` or
`atomic idempotent turn advancement is not declared` for Moon. Database health
alone cannot prove that the host selected the context engine.

```bash
moon update --check
moon --json update --check
moon update --dry-run
```

`--check` downloads only the bounded canonical manifest and detached signature.
It verifies the embedded Ed25519 trust root, selects the exact native target,
and reports the invoked, canonical, PATH-resolved, and OpenClaw-configured Moon
executables. It opens an existing database read-only and creates no cache,
directory, database, lock, or journal.

`--dry-run` additionally downloads and verifies the selected archive in memory,
checks free space, database health and leases, minimum OS/OpenClaw versions, the
Moon-owned OpenClaw configuration fields, and prints the mutation plan. It does
not stage files or stop the gateway.

Gateway startup and the full OpenClaw plugin doctor each have a two-minute
execution limit. The development updater gives a slow Gateway a second readiness
check within the original startup deadline. If OpenClaw reports that the Gateway
is still starting, Moon confirms the service PID and repeats OpenClaw's native
startup check once. The same running process must pass. An explicit startup
failure or a failed recheck triggers rollback. The installed binary controls
this behaviour for both update and recovery.

## HTTPS proxies and CA trust

Moon 2.6.5 adds support for the host-provided CA bundle. Earlier versions can
report `UnknownIssuer` behind OpenClaw's proxy even when curl succeeds. Upgrade
through a trusted direct shell using the normal signed updater, then recheck
from the affected OpenClaw environment. Do not disable TLS verification to
bootstrap the fix.

The release HTTP client uses rustls with bundled Mozilla public roots. When
`SSL_CERT_FILE` is set, it also loads every PEM certificate in that file as an
additional trust root. OpenClaw sets this variable to its managed CA bundle for
the local HTTPS egress proxy; Moon reads the current bundle when constructing
the release client. It does not modify the system trust store or persist the CA.

This policy applies to manifests, detached signatures, allowed redirects, and
archives through the same client. TLS chain, hostname, and date validation
remain enabled, as do signed-release, checksum, host-allowlist, and rollback
checks. An unset variable preserves the default public roots. An empty value,
unreadable file, malformed PEM/X.509 certificate, certificate-free bundle, or
bundle larger than 16 MiB fails clearly before downloading a release. Other PEM
item types are ignored; they do not count as CA certificates.

Use the host-managed CA file reference rather than copying a temporary proxy
leaf certificate into a permanent trust store. `CURL_CA_BUNDLE`,
`REQUESTS_CA_BUNDLE`, and `NODE_EXTRA_CA_CERTS` do not configure Moon's updater.
Validate a repair with `moon update --check` from the affected proxy
environment; success from an ordinary shell does not verify the OpenClaw egress
path.

## Provider-neutral routing transition

The first release containing provider-neutral model routing intentionally drops
Moon's Codex-specific plugin fields. Before checking or applying that release,
verify that OpenClaw itself has both model routes:

```bash
openclaw config get agents.defaults.model
```

Moon 2.5.1 preflight rejects the retired fields below before staging or gateway
downtime when the target is 2.4.0 or newer. Older installed updaters still need
this preparation even when using the recovery helper.

Back up the live OpenClaw configuration with owner-only permissions, then remove
the retired Moon fields in one validated patch while the old adapter is still
installed:

```bash
moon_config_reported="$(openclaw config file)"
moon_openclaw_config="${moon_config_reported/#\~/$HOME}"
moon_backup_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
moon_openclaw_backup="${moon_openclaw_config}.before-provider-router.${moon_backup_stamp}"
install -m 600 "$moon_openclaw_config" "$moon_openclaw_backup"

openclaw config patch --stdin <<'JSON5'
{
  plugins: {
    entries: {
      moon: {
        config: {
          codexProvider: null,
          codexModel: null,
          codexReasoning: null,
          learningModel: null,
          learningReasoning: null,
        },
      },
    },
  },
}
JSON5

openclaw config validate
```

The old adapter accepts those fields being absent, so this preparation does not
require loading unreleased code. Do not remove the private backup until the new
adapter has loaded, primary and fallback model canaries have passed, and
rollback is no longer required. The native updater does not mutate unrelated
OpenClaw configuration or infer provider credentials.

## Apply

```bash
moon update
moon update --version=2.9.2
moon --json update --yes
```

Interactive application prints the plan and asks once. JSON or other
non-interactive application requires `--yes`. A shadowing executable may check
but cannot apply; run the canonical path reported by `--check`. Downgrades need
both an exact signed version and `--allow-downgrade`, and still cannot open a
newer unsupported schema. Recover with the old binary's matching database rather
than treating downgrade as a reverse migration.

The updater installs and recovers the agent skill for the active OpenClaw
profile. It uses `OPENCLAW_STATE_DIR`, then the parent of
`OPENCLAW_CONFIG_PATH`, then the selected profile under the OpenClaw home.
`MOON_SKILL_PATH` explicitly selects a different `SKILL.md`. Use the same
profile and skill path when resuming an interrupted update.

The transaction performs these durable phases:

1. Check for interrupted transactions before fetching a release or checking
   runtime health. With approval, acquire the owner-only lock and recover a
   proven-dead prior transaction. Recovery returns `retry_required: true`; run
   the canonical command again so the restored binary handles the next update.
2. Repeat read-only health, lease, platform, schema, OpenClaw, and disk-space
   preflight.
3. Extract only the declared regular files into owner-only staging. Reject
   traversal, symlinks, special files, duplicates, unexpected paths, size
   excess, mode mismatch, or any hash mismatch.
4. Run the staged binary against an explicit temporary home and database,
   including identity, initialization, health, write, and lexical-recall
   canaries. An inherited `MOON_DATABASE` cannot redirect these checks. Verify
   any inactive retained target before reusing it, or materialize the verified
   release. Reject conflicting files before shutdown.
5. Record restart intent, stop OpenClaw, confirm its Moon worker has exited,
   then create and verify an owner-only rollback bundle containing a consistent
   SQLite backup, canonical memory export, current compatibility files and
   hashes, health, signed release inputs, plan, and only schema-selected
   non-secret Moon integration settings.
6. Persist switch intent, atomically switch `current`, install the skill, and
   run numbered transactional migrations. Moon forwards the approved restart
   plan as `gateway stop --force --json`, including when OpenClaw runs without
   interactive standard input.
7. Verify installed identities and hashes, database/queue health, OpenClaw
   config and plugin doctor results, loaded adapter version and slots, bounded
   gateway readiness, and one local hybrid retrieval canary.
8. Commit the journal and retain the previous release and rollback bundle.

Successful JSON includes the trusted `verified_key_ids` that authenticated the
release. The same IDs are retained in the transaction journal for rotation and
incident review.

If a post-quiesce gate fails and restoration is safe, Moon switches back to the
prior immutable release, restores the prior skill and verified SQLite backup,
restarts OpenClaw, and verifies rollback health before returning
`rollback_completed`. Post-restart database changes or conflicting skill edits
stop restoration for operator review, preserving newer work. If rollback cannot
itself be proven, it returns `rollback_failed`, preserves the lock, journal,
failed database, releases, and backup, and must not be reported as a safe
failure.

`moon update --check` and `moon update --dry-run` report
`recovery_required: true` without changing an interrupted installation. Run
`moon update` interactively, or `moon --json update --yes`, to authorise
recovery. An already selected target version does not bypass recovery. Database
restoration stages and validates the replacement before moving the active
database, and is not subject to the release archive's 512 MiB limit.

Moon records restart intent before requesting a stop, so a failed worker check
after service shutdown still restores gateway availability. If the candidate
gateway has already started, rollback stops it and waits for the Moon worker
before restoring the old release or database. Failed rollback quiescence leaves
the candidate files and database in place for recovery.

After restoring the prior database, Moon records that completion before
restarting the gateway. If the final readiness check fails, a later recovery
retries availability and validation without overwriting newer memories. Older
failed rollback journals without enough evidence stop with `recovery_ambiguous`;
preserve their files for inspection instead of deleting the journal to retry.

## Recover an older updater on OpenClaw 2026.9.2

Moon 2.5.0 and earlier invoke `gateway stop --json` without the non-interactive
consent flag required by OpenClaw 2026.9.2. Manually stopping the gateway does
not help: the updater issues its own stop command. `moon update --yes` approves
Moon's transaction but cannot repair the old subprocess arguments.

From a reviewed Moon 2.5.2 checkout, run the one-time recovery helper:

```bash
sh tools/recover-openclaw-update.sh --version=2.5.2 --dry-run
sh tools/recover-openclaw-update.sh --version=2.5.2
```

First complete the provider-neutral routing preparation above if the installed
configuration still contains the retired Codex-specific fields. The helper does
not change configuration or credentials.

Use the equals form shown above with older updaters: before Moon 2.5.2,
combining `--json` with a separate `--version <target>` argument prints the
installed version instead of running the update. Moon 2.5.2 fixes that parsing
bug; the equals form also works with older releases.

The helper creates a private temporary command wrapper for this invocation. It
adds `--force` only to the old updater's exact `gateway stop --json` call and
forwards every other OpenClaw command unchanged to the original executable. The
installed Moon still verifies signatures and hashes, presents its update
confirmation, creates its rollback bundle, and validates the installed release.
The wrapper is removed on exit; no signed binary or shell startup file is
modified. Subsequent updates use the repaired updater normally.

After a successful update, verify:

```bash
moon --version
moon --json health
openclaw gateway status
openclaw plugins inspect moon --runtime --json
```

If an older updater fails after stopping the gateway, run
`openclaw gateway start` to restore service, then inspect the error and retained
journal before retrying. The helper fixes the old command arguments; it does not
add the newer updater's rollback behaviour to an old executable.

## Layout and retained recovery evidence

```text
~/.moon/
  bin/moon -> ../current/bin/moon
  current -> releases/2.9.2
  openclaw-plugin -> current/openclaw-plugin
  releases/<version>/
  state/moon.sqlite
  update/update.lock
  update/journals/<transaction>.json
  backups/<transaction>/
```

No automatic cleanup exists. Old releases, bootstrap-retired files, failed
databases, staging diagnostics, journals, and rollback bundles remain until a
separate owner-authorized review.

## v2.1.0 bootstrap boundary

Moon v2.1.0 has no `update` command. Install the signed v2.2.1 compatibility set
once through the controlled release procedure: verify the production signature
and every archive/file hash, create and verify the complete rollback bundle,
stop OpenClaw, place the v2.2.1 release without deleting v2.1.0, select the
stable paths, migrate, and run every post-switch gate above. Do not describe
this source/release-operator procedure as toolchain-free. Native no-toolchain
updates begin only after an updater-capable release is installed.

Recovery documentation uses individual commands instead of a commented block
pasted after enabling strict mode. If a maintainer must feed a strict-mode file
to interactive zsh, `setopt interactivecomments` must be the first line. CI
feeds `tools/interactive-zsh-smoke.zsh` to `zsh -f -i` and requires its explicit
`UPDATE SUCCEEDED` marker, preserving the exact shell behavior involved in the
2026-08-10 incident.

## Stable error states

Machine-readable failures include `shadowed_executable`, `update_locked`,
`unsupported_platform`, `release_unavailable`, `signature_invalid`,
`checksum_mismatch`, `insufficient_space`, `unhealthy_runtime`,
`active_embedding_lease`, `candidate_failed`, `migration_failed`,
`plugin_validation_failed`, `gateway_unreachable`, `rollback_completed`, and
`rollback_failed`. Messages and journals pass Moon's redaction boundary and do
not include prompts, recalled memory, credentials, unrelated OpenClaw settings,
or arbitrary remote response bodies.

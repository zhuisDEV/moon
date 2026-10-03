# Native signed updates

Moon v2.2.0 introduced the native updater. Use v2.2.1 or later for the initial
bootstrap; v2.2.1 preserves the embedding identity already stored by v2
databases. The updater changes the binary, OpenClaw adapter, agent skill, and
database schema as one compatibility-set transaction. It never compiles remote
source, evaluates manifest fields as shell, downloads trust roots, deletes prior
releases, or copies credentials.

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
moon update --version 2.2.1
moon update --version 2.2.0 --allow-downgrade
moon --json update --yes
```

Interactive application prints the plan and asks once. JSON or other
non-interactive application requires `--yes`. A shadowing executable may check
but cannot apply; run the canonical path reported by `--check`. Downgrades need
both an exact signed version and `--allow-downgrade`.

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

If any post-quiesce gate fails, Moon switches back to the prior immutable
release, restores the prior skill and verified SQLite backup, restarts OpenClaw,
and verifies rollback health before returning `rollback_completed`. If rollback
cannot itself be proven, it returns `rollback_failed`, preserves the lock,
journal, failed database, releases, and backup, and must not be reported as a
safe failure.

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
  current -> releases/2.2.1
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

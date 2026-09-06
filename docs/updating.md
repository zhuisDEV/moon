# Native signed updates

Moon v2.2.0 introduced the native updater. Use v2.2.1 or later for the initial
bootstrap; v2.2.1 preserves the embedding identity already stored by v2
databases. The updater changes the binary, OpenClaw adapter, agent skill, and
database schema as one compatibility-set transaction. It never compiles remote
source, evaluates manifest fields as shell, downloads trust roots, deletes prior
releases, or copies credentials.

## Read-only inspection

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

1. Acquire an owner-only PID/transaction lock and recover any proven-dead prior
   journal.
2. Repeat read-only health, lease, platform, schema, OpenClaw, and disk-space
   preflight.
3. Extract only the declared regular files into owner-only staging. Reject
   traversal, symlinks, special files, duplicates, unexpected paths, size
   excess, mode mismatch, or any hash mismatch.
4. Run the staged binary against a new temporary home, including identity,
   initialization, health, write, and lexical-recall canaries.
5. Create and verify an owner-only rollback bundle containing a consistent
   SQLite backup, canonical memory export, current compatibility files and
   hashes, health, signed release inputs, plan, and only schema-selected
   non-secret Moon integration settings.
6. Stop OpenClaw, confirm the Moon worker has exited, materialize the immutable
   release directory, atomically switch `current`, install the skill, and run
   numbered transactional migrations. Moon 2.5.1 forwards the approved restart
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

Moon records restart intent before requesting a stop, so a failed worker check
after service shutdown still restores gateway availability. If the candidate
gateway has already started, rollback stops it and waits for the Moon worker
before restoring the old release or database. Failed rollback quiescence leaves
the candidate files and database in place for recovery.

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

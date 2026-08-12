# Moon v1 to v2 migration

Moon v2 is a breaking architectural replacement. It removes the v1 QMD, Node,
watcher, and generated-Markdown-as-primary-store paths. The formal identities
are now:

- command: `moon`
- runtime: `~/.moon`
- database: `~/.moon/state/moon.sqlite`
- OpenClaw plugin and context-engine id: `moon`
- environment variables: `MOON_HOME`, `MOON_DATABASE`, and
  `MOON_EMBEDDING_DIMENSIONS`

The production migration completed on 2026-07-29. The validated v2 database is
active under `~/.moon`; the retired v1 runtime remains read-only at
`/Users/lilac/.moon-legacy-20260728T080052Z`.

## Upgrade contract

1. Stop the v1 watcher before changing the runtime.
2. Make a dated copy of the complete v1 runtime and OpenClaw configuration.
3. Build and validate Moon v2 in a separate checkout and temporary `--home`.
4. Initialize the v2 SQLite database.
5. Import v1 memory from the dated copy with `import-legacy`; never point a
   migration at the only copy of the source.
6. Embed eligible memory and references with the local provider.
7. Require `health` to report schema v6, complete vector coverage, zero evidence
   vectors, no failed/dead work, and zero integrity or logical violations.
8. Install the `moon` OpenClaw plugin, select its context-engine slot, disable
   OpenClaw memory search, and retain native automatic compaction.
9. Run lexical, hybrid, completed-turn, automatic-embedding, and compaction
   canaries before retiring the v1 plugin.
10. Keep the dated v1 runtime and pre/post-migration SQLite backups for
    rollback.

The Moon CLI deliberately has no cutover or legacy-delete command.

For updates between Moon v2 releases, v2.2.0 and later use the native signed
transaction described in [updating.md](updating.md). The first v2.1.0 to v2.2.0
installation is the documented bootstrap boundary; it retains v2.1.0 and does
not change the preserved v1 rollback runtime.

## SQLite migrations

Moon applies numbered migrations transactionally. The current schema is v6.
Opening a normal command migrates older Moon v2 databases forward; `health`
never creates or migrates storage.

Schema v5 adds canonical-head, immutable-revision, citation, and embedding-lease
invariants. Schema v6 adds priority, retry scheduling, and dead-letter state for
automatic embedding. If existing data violates a new invariant, migration stops
and rolls back instead of guessing which record to keep.

## Embedding model changes

A database contains exactly one vector space. Moon rejects indexing or querying
with a model name or dimensions different from the recorded space. To change
models:

1. create an online SQLite backup and memory export;
2. stop other embedding workers;
3. run `requeue-embeddings`;
4. run `embed --provider local --drain`;
5. require complete health coverage before restoring hybrid traffic.

`requeue-embeddings` refuses to clear vectors while an embedding worker owns an
active lease.

## Rollback

Rollback restores the dated v1 runtime and matching OpenClaw configuration
together. Do not point v1 at the v2 SQLite database, and do not attempt a
reverse schema migration. The v2 database and generated memory export remain
available for forensic comparison or a later retry.

# Database migration and rollback

For normal Moon v2 updates, use the signed transaction in
[updating](updating.md). You do not need to export and reimport your knowledge
or convert Markdown files to use Moon 2.8.1. New users should follow
[getting started](getting-started.md). The
[v1 migration](#legacy-v1-to-v2-migration) below applies only to the retired
QMD/watcher runtime.

Moon 2.8.1 uses database **schema 20** and accepts supported databases from
schema 6 through 20. The runtime remains `~/.moon`, the database remains
`~/.moon/state/moon.sqlite`, and the OpenClaw plugin ID remains `moon`. Runtime
settings in `moon.toml` survive updates; migration does not enable L2, call a
model or create a new login.

Moon 2.7.0 updated bundled SQLite from 3.51.3 to patched 3.54.1. The engine
upgrade does not add a Moon schema migration or change the vector model. Keep
earlier engine timings historical and validate the actual release before making
performance claims.

Schema 14 introduces a retry ledger for session learning. It recovers known
deferrals for still-pending sessions from committed learning runs. A migration
does not generate proposals or mark those sessions processed.

## Before an existing v2 runtime is upgraded

1. Verify `command -v moon` and `moon --json --version` identify the currently
   installed binary. Preserve that executable with the recovery point.
2. Back up the database with that **pre-upgrade executable**, then retain
   `moon.toml`, any referenced prompt files and the OpenClaw configuration in a
   private directory. The new binary's normal write commands, including
   `backup`, can migrate storage before doing their requested work; do not use
   it to create the first pre-migration backup. A Markdown memory export is
   useful for inspection but is not a complete database backup.
3. Inspect `moon update --check` and `moon update --dry-run`, including the
   signed version, supported schema range and skill plan. The signed updater
   stops its workers before taking its final recovery snapshot. For a manual
   rehearsal, use a consistent snapshot with an explicit temporary `--home` and
   database path; never copy only the live SQLite file while WAL writers are
   active.
4. Follow the upgrade guide's bridge-release instructions for 2.6.6 and earlier.
   Do not send an older six-file reader a documentation-extended archive.
5. After migration, check health, configuration, plugin discovery and actual
   recall. Refresh `SKILL.md` and its companion documents as described in the
   upgrade guide, including any separately installed agent copies.
6. Retain the previous immutable release and its matching database, plugin,
   skill and configuration backups. Rehearse restoration before relying on them.
   For an upgrade from 2.7.0, restore its schema-13 backup with its matching
   executable.

## Schema 15 audience migration

Schema 15 creates fresh public and private audiences and preserves original
global documents in quarantine. A migration and recovery rehearsal, operator
review commands and cutover considerations are documented in
[Public and private memory](memory-privacy.md).

## Schema 16 sender attribution

New private chat evidence records the verified sender independently of the
original reader set. The migration preserves existing evidence and audience
bindings and leaves their authors unknown. Existing global memories remain in
quarantine.

## Schema 17 conversation ownership

New admissions distinguish owners from potential readers. Historical admissions
retain the provenance originally recorded. An operator may assign the frozen
legacy global memory collection to one private owner. See the
[assignment procedure](memory-privacy.md#reviewing-the-old-global-collection).

## Schema 18 room-access snapshots

Schema 18 preserves existing question ownership, legacy assignments and original
records. New questions use verified room access. A reply needs its own verified
delivery snapshot for private history recall. The migration leaves the owner
fields of old replies unknown. Future room permissions cannot supply that
missing historical evidence.

## Schema 19 speaker attribution

New private learning distinguishes speakers who share the same room and the same
audience. A correction by one speaker can update that speaker’s earlier
statement. Audience checks still apply to every reader before recall. The
migration preserves historical records and does not infer a missing speaker from
a quotation, a room’s current membership, or the operator’s legacy ownership
assignment.

## Schema 20 personal profiles and bot evidence

Moon 2.8.1 adds bot provenance and records historical Discord IDs that have been
used as personal profile memory owners. Existing evidence, speakers and
ownership are retained. The new profile setting in `moon.toml` controls future
captures. See [profile setup](memory-privacy.md#personal-openclaw-profile).

Rehearse with a consistent copy using an explicit temporary `--home` and
`--database`. A rollback to signed Moon 2.8.0 requires the pre-migration SQLite
backup, the matching adapter and skill, and a compatible `moon.toml` from before
the `[master]` setting. Retain those together before applying a migration.

## SQLite migrations

Moon applies numbered migrations transactionally. Moon 2.8.1 uses schema 20.
Opening a normal write command migrates older supported Moon v2 databases
forward; `health` and source inspection (`get`, `list`, `history`, `cite`,
`state`, deletion preview, learning status/preview, and claim inspection) never
create or migrate storage.

Schema v5 adds canonical-head, immutable-revision, citation, and embedding-lease
invariants. Schema v6 adds priority, retry scheduling, and dead-letter state for
automatic embedding. Schema v7 adds content-free metrics, and schema v8 adds
learning lifecycle state. The earlier 2.6.x releases accept schemas 6 through 8
and migrate older supported databases to schema 8. If existing data violates a
new invariant, migration stops and rolls back instead of guessing which record
to keep.

Schema v9 adds separate immutable source identities and revisions. It preserves
the existing indexed documents, completed-session evidence, canonical memories,
citations and learning state. Existing generated memory exports are not source
intake inputs by default; reimporting them would duplicate already retained
knowledge and risk circular evidence. Source bodies stay outside automatic L1/L2
and direct recall. See [source storage](source-storage.md).

Schema v10 adds source lifecycle state, recoverable source-processing jobs,
reviewed verbatim claims and exact source support. The source-learning workflow
requires explicit preparation/submission and operator acceptance. It starts no
automatic worker and makes no model request during migration. Existing eligible
captures are reconciled into durable work according to the migration contract;
captured text is never treated as an already approved claim.

Schema v11 adds scoped topic definitions and aliases, compilation jobs,
immutable typed revisions and flattened source/claim/topic dependencies.
Existing source claims are not automatically grouped or compiled. Migration
calls no model. Only explicitly reviewed fresh topic revisions can join recall;
source deletion also purges affected copied topic material. See
[reviewed topics](topics.md).

Schema v12 replaces the valid-source-claim view's deduplicating join with an
existence check. Each claim still appears once, and expiry, current source
revision, scope, learning eligibility and withdrawal checks remain in force.
This avoids rebuilding the entire valid-claim set during joined retrieval. The
migration changes no retained content, IDs, citations, indexes or model
configuration; it runs transactionally with its migration ledger entry.

Schema v13 adds nullable, constrained `failure_code` columns to source and topic
jobs. It preserves existing columns, rows, accepted reviews and topic revisions;
old errors remain readable. Configuration failures stop until explicit retry,
while timeouts use the existing retry budget. Both alterations and the migration
ledger commit are one transaction. No model is called during migration.

Before deploying schema v19, rehearse migration, integrity checks and backup
restoration with a temporary `--home`. Obtain a consistent online SQLite backup
and configuration backup before the authorised live migration. Preserve the
previous binary with its matching database backup: an older binary must not open
schema v19 as a rollback attempt. Building the checkout does not authorise
replacing the installed binary, plugin or live database.

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

Rollback restores the previous binary/plugin/skill and its matching SQLite
backup together; there is no reverse schema migration. Stop Gateway and verify
its Moon workers and active leases have ended before replacing storage. Preserve
the current database and WAL/SHM in a new private recovery directory before
restoring anything. Configuration restoration must preserve any edits made since
the backup.

The signed updater verifies rollback rather than assuming it succeeded. If the
candidate host has already changed learning state, or managed skill files have
been edited, rollback can stop for operator recovery to avoid overwriting new
work. Preserve the journal, failed database, releases and backups; do not delete
the lock or copy an old snapshot over newer work to force a retry. See the
[recovery procedure](updating.md) and report `rollback_failed` as incomplete
recovery.

## Legacy v1 to v2 migration

Moon v2 replaces the v1 QMD, Node.js watcher and Markdown knowledge store with
one Rust binary and SQLite. This is a separate migration from a normal v2
update. The CLI deliberately has no cutover or legacy-delete command.

1. Stop the v1 watcher and preserve a dated copy of the complete v1 runtime,
   executable/plugin and OpenClaw configuration. Keep the source copy read-only.
2. Build and validate v2 in a separate checkout and explicit temporary `--home`.
   Initialise a new SQLite database, then use `import-legacy` against the dated
   copy, never the only source copy.
3. Embed eligible memories and references with the local provider. Require
   database health, zero logical violations and complete eligible vector
   coverage; raw completed-session evidence should have zero vectors.
4. Follow the installation guide for the Moon context-engine slot, disabled
   duplicate memory search and host-owned compaction. Verify lexical/hybrid
   retrieval, completed-turn learning and embeddings before cutover.
5. Keep the old runtime and pre/post-migration backups. A v1 rollback restores
   the complete v1 runtime and its matching OpenClaw configuration; never point
   v1 at the v2 SQLite database.

Moon v2.1.0 also lacks the native updater. Its controlled v2.2.1 bootstrap is
described in [updating](updating.md); no-toolchain updates begin after that
boundary.

# Release Signing

Moon release manifests use detached Ed25519 signatures. The production private
key is stored as a protected GitHub environment secret for the manually approved
release workflow. A dedicated macOS keychain remains the offline recovery copy
and supports a local signing ceremony. Moon embeds only the public trust root.

## Production key

- Key ID: `moon-release-2026-01`
- Public key document: `assets/release-keys/moon-release-2026-01.pub`
- GitHub environment: `production-release`
- Environment secret: `MOON_RELEASE_SIGNING_KEY`
- Offline recovery keychain:
  `~/Library/Keychains/moon-release-signing.keychain-db`
- Keychain item service: `dev.zhuis.moon.release-signing`
- Workflow policy: exact annotated `vX.Y.Z` tag, manual environment approval,
  tag-restricted environment, least-privilege publication job, and release
  overwrite refusal
- Recovery policy: not in the default keychain search list, lock on sleep, lock
  after five minutes, and explicit confirmation for every private-key read

After using `security show-keychain-info` to inspect the timeout policy, run
`security lock-keychain` again: macOS may leave the inspected custom keychain
unlocked.

The public key and its fingerprint are public release metadata. The private seed
must never be placed in the repository, command arguments, logs, release
staging, workflow artifacts, or cloud-synchronised directories. GitHub exposes
the environment secret only to the approved signing step, which immediately
pipes it to the bounded release signer over standard input.

## Protected production release

The normal production flow is deliberately short:

1. Complete `RELEASE.md`, commit the reviewed release to `main`, and create and
   push one annotated `vX.Y.Z` tag.
2. Run the `Release` workflow at that exact tag:

   ```bash
   gh workflow run release.yml --ref vX.Y.Z
   ```

3. Review the pending `production-release` deployment in GitHub and approve it.
4. The workflow builds all four native archives, assembles the canonical
   manifest, signs it, verifies it through Moon's embedded production trust
   root, and creates the GitHub release. It fails instead of replacing an
   existing release or asset.

Ordinary push and pull-request CI produces unsigned artifacts and cannot access
the signing secret. The release job receives `contents: write` only after its
dependencies pass and the protected environment is approved.

### One-time Keychain migration

Create and protect the `production-release` environment before transferring the
existing seed. Confirm that `MOON_RELEASE_SIGNING_KEY` does not already exist;
the migration is intentionally non-overwriting at the operator level. On the
approved Mac, use a direct pipe so the seed is never written to disk, an
argument, or terminal output:

```bash
gh secret list --repo zhuisDEV/moon --env production-release

security find-generic-password -w \
  -s dev.zhuis.moon.release-signing \
  -a moon-release-2026-01 \
  "$HOME/Library/Keychains/moon-release-signing.keychain-db" |
  xxd -p -c 256 |
  tr -d '\n' |
  gh secret set MOON_RELEASE_SIGNING_KEY \
    --repo zhuisDEV/moon \
    --env production-release

security lock-keychain \
  "$HOME/Library/Keychains/moon-release-signing.keychain-db"
```

Approve the macOS per-use access prompt. GitHub does not permit reading the
stored secret back; validate it by running the protected workflow and requiring
the resulting signature to verify against the repository public key.

## Local recovery signing ceremony

Run the ceremony from a clean, reviewed release checkout. The staging directory
must be owned by the operator and have mode `0700`.

1. Generate every platform archive and the canonical outer manifest as described
   in `RELEASE.md`.
2. Independently inspect the manifest identity, targets, minimum OS versions,
   schema compatibility, archive sizes, and SHA-256 values.
3. Lock the keychain before starting:

   ```bash
   security lock-keychain \
     "$HOME/Library/Keychains/moon-release-signing.keychain-db"
   ```

4. Sign the exact canonical manifest bytes:

   ```bash
   cargo run --locked --example moon-release -- sign \
     --manifest /path/to/release-staging/release-manifest.json \
     --signature-output /path/to/release-staging/release-manifest.sig.json
   ```

   Enter the keychain password only in the macOS SecurityAgent prompt and
   approve the per-use Keychain request. Never pass the password to the command.
   The tool checks that the private key matches the reviewed public key,
   verifies the detached signature before writing it, refuses overwrites, and
   re-locks the keychain when it exits.

5. Record the manifest SHA-256 printed by the tool. Re-run release verification
   using Moon's embedded production trust roots before publishing:

   ```bash
   cargo run --locked --example moon-release -- verify \
     --manifest /path/to/release-staging/release-manifest.json \
     --signature /path/to/release-staging/release-manifest.sig.json
   ```
6. Publish the canonical manifest, detached signature set, and exactly the
   archives named and hashed by that manifest. This path is for recovery; the
   protected workflow is the normal production path.

## Rotation

A replacement key receives a new monotonically named key ID and public key
document. The transition release must contain signatures from both the retiring
and replacement keys. Add the replacement public key to Moon's embedded keyring,
release that version under the dual signature, and retain the retiring key until
the supported client floor trusts the replacement. Removing a trusted key
requires a later reviewed release and must never make the current supported
client floor unable to verify the stable channel.

## Recovery and backup

The Keychain file is encrypted, but a copy alone is not a complete recovery
procedure unless its password is also available through a separate approved
secret-recovery channel. Keep it offline from routine release work. Test
recovery only on an isolated machine or account, and verify a disposable
manifest against the repository public key before declaring the backup usable.

If the private key is lost without a working backup, existing clients cannot
trust a newly generated key. If compromise is suspected, stop publishing,
preserve evidence, and prepare a dual-signed rotation only if the retiring key
is still trustworthy enough to perform that transition.

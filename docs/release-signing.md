# Release Signing

Moon release manifests use detached Ed25519 signatures. The production private
key is kept outside the repository and CI in a dedicated macOS keychain. Moon
embeds only the public trust root.

## Production key

- Key ID: `moon-release-2026-01`
- Public key document: `assets/release-keys/moon-release-2026-01.pub`
- Keychain: `~/Library/Keychains/moon-release-signing.keychain-db`
- Keychain item service: `dev.zhuis.moon.release-signing`
- Policy: not in the default keychain search list, lock on sleep, lock after
  five minutes, and explicit confirmation for every private-key read

After using `security show-keychain-info` to inspect the timeout policy, run
`security lock-keychain` again: macOS may leave the inspected custom keychain
unlocked.

The public key and its fingerprint are public release metadata. The private seed
must never be placed in the repository, CI, environment variables, command
arguments, logs, release staging, or cloud-synchronized directories.

## Signing ceremony

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
   archives named and hashed by that manifest. CI must not sign a production
   release.

## Rotation

A replacement key receives a new monotonically named key ID and public key
document. The transition release must contain signatures from both the retiring
and replacement keys. Add the replacement public key to Moon's embedded keyring,
release that version under the dual signature, and retain the retiring key until
the supported client floor trusts the replacement. Removing a trusted key
requires a later reviewed release and must never make the current supported
client floor unable to verify the stable channel.

## Recovery and backup

The keychain file is encrypted, but a copy alone is not a complete recovery
procedure unless its password is also available through a separate approved
secret-recovery channel. Do not copy or export it until the owner has approved
an offline backup destination and recovery custodian. Test recovery only on an
isolated machine or account, and verify a disposable manifest against the
repository public key before declaring the backup usable.

If the private key is lost without a working backup, existing clients cannot
trust a newly generated key. If compromise is suspected, stop publishing,
preserve evidence, and prepare a dual-signed rotation only if the retiring key
is still trustworthy enough to perform that transition.

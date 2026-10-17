# The Keychain probe, 17 October 2026

Step 5, item 7, before the Keychain credentials slice. The user chose to keep the server key in
the shared team Keychain, meaning the data-protection keychain under the access group
`HAUTK68F56.dev.voidfs`, and to have the daemon read it itself. They asked for a probe first:
can the bundled `void` read the key without a prompt, and is all other code kept out?
macOS 27.0.1 (26A434), Swift 6.4, Rust 1.98.1, Apple M5 Pro.

## The rig

- **Two probes** ([sources](probe/)).
  - `kcprobe` is Rust, using security-framework 3.7.0, the version already in `Cargo.lock`, with
    its `OSX_10_15` feature. It calls `PasswordOptions::use_protected_keychain` and
    `set_access_group`, as the daemon would.
  - `writer` is Swift and calls `SecItemAdd`, `SecItemUpdate`, `SecItemCopyMatching` and
    `SecItemDelete` with `kSecUseDataProtectionKeychain`, as the app's sign-in would.
- **One item.** A generic password, service `dev.voidfs.probe` and account `probe`. It held only
  throwaway values (`probe-<8 hex digits>`) and was deleted at the end: neither keychain has it.
- **Signed variants**, all with the hardened runtime unless the row says otherwise: Developer ID
  Application (HAUTK68F56), Apple Development (team HAUTK68F56), and ad hoc. Their identifiers
  are `dev.voidfs.void` and `dev.voidfs.app`, and they claim one of the three entitlement sets in
  [probe/](probe/).
- **Two probe bundles**, `KCProbe.app` and `KCProbeDev.app`, each with an `Info.plist`
  ([this one](probe/Info.plist), bundle IDs `dev.voidfs.kcprobe` and `dev.voidfs.kcprobedev`).
  - Their main executable is `writer`, with `kcprobe` beside it in `Contents/MacOS`. The dev
    bundle has another copy in `Contents/Helpers`.
  - Each embeds one of this Mac's two provisioning profiles. Both are for the FSKit extension's
    App ID, `dev.voidfs.app.fskit`, and grant `keychain-access-groups HAUTK68F56.*`: "voidfs FSKit
    Developer ID", and Xcode's development profile.
  - They were used only to test the mechanism, and run directly, never opened.
- **Prompts.** Each call ran under a 15 s alarm, with `SecurityAgent` counted before and after.

LaunchServices' records were the same after the probe as before it, so nothing was registered.

## Results

| Code | Result |
| --- | --- |
| A bare tool or the Swift writer with only the App Group (`com.apple.security.application-groups`), signed with Developer ID, Apple Development or ad hoc | **-34018**, "A required entitlement is not present". secd: the App Group "is ignored because of invalid application signature or incorrect provisioning profile" |
| No entitlements | **-34018** |
| The Developer ID bundle with its profile: the Swift main and the Rust tool beside it, both claiming `keychain-access-groups HAUTK68F56.dev.voidfs` with the application and team identifiers | **Each reads, writes and deletes the other's item**, in 1–9 ms. No prompt, and `SecurityAgent` didn't start |
| A bare tool signed with Developer ID, with `keychain-access-groups` and no profile anywhere (with or without the application identifier) | **Runs, reads and writes the item** |
| The same bare tool signed with Apple Development | **Killed at launch** (exit 137). amfid: -413, "No matching profile found" for `keychain-access-groups` |
| A tool in the dev bundle's `Contents/MacOS` or `Contents/Helpers`, signed with Apple Development, the bundle holding the development profile | **Killed at launch**, -413: a tool doesn't use its bundle's profile |
| Ad hoc with `keychain-access-groups`, with or without the hardened runtime | **Killed at launch**, amfid -424: "The file is adhoc signed but contains restricted entitlements" |
| An item written by the dev bundle, read by the Developer ID bundle | **Read**: one team, one group, so development and release builds share items |
| `security find-generic-password -s dev.voidfs.probe` while an item existed | Not found (exit 44): the item is only in the data-protection keychain, not the login keychain |

Outputs: [App Group only](raw/matrix.txt), [profile-backed bundle](raw/matrix-profile.txt),
[bare Developer ID tool](raw/matrix-bare.txt), [ad hoc and Apple Development controls](raw/matrix-controls.txt),
[development bundle](raw/matrix-dev.txt), and [secd and amfid's log lines](raw/log-excerpt.txt).

## What it means

- **The shared team Keychain works as hoped.** There is no prompt, a read takes about 2 ms, and
  only code signed by team HAUTK68F56 that may claim the group gets in. Ad hoc code is refused or
  killed, so a `void` built in a terminal can't read the key.
- **The App Group isn't enough.** Every executable that touches the item must claim
  `keychain-access-groups`. That is a restricted entitlement, backed by a provisioning profile
  embedded in that executable's own bundle. Today the app has no profile, and the bundled `void`
  has no bundle.
- **Developer ID code ran without a profile**, but Apple doesn't document that, and Apple
  Development builds are killed without one. Nothing should rely on it.
- **For the daemon to read the key itself**, `void` needs a bundle of its own, for example
  `Contents/Library/Helpers/VoidfsDaemon.app` with an `Info.plist`, `embedded.provisionprofile`
  and `Contents/MacOS/void`. It also needs an App ID, a development profile and a Developer ID
  profile, and the job's `BundleProgram` moves into that bundle. The app needs its two profiles
  for `dev.voidfs.app` as well, since the sign-in writes the item.
- **If the app hands the key over**, only the app's profiles are needed. The job would run the
  app's binary, which reads the item and runs `void` with the key on a pipe.

## Not covered

- **The job's launchd context.** The probe ran from a shell in the user's session. The daemon
  runs in `gui/<uid>`, the same session, but that wasn't tested.
- **Before the first unlock after a reboot.** The writer used
  `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`, which a login daemon can read once the user
  has unlocked the Mac.
- **The real app and `void`**: neither was rebuilt or signed with a profile.

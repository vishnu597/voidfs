# The bridge agent's kept requirement, 18 October 2026

Step 5, item 7. The [logout, login and reboot check](../item7-login/README.md#installing-and-registering)
found the bridge agent `dev.voidfs.agent` refused in the Developer ID copy in `/Applications`.
Background Task Management had kept a development build's launch requirement (`validation-category`
3, not 6). One unregister/register round kept it again, and a second cleared it
([agent.txt](../item7-login/raw/agent.txt)). The app now does that itself, as #84 made it do for the
login job ([step 5](../../docs/step-5-macos.md#the-bridge-agents-kept-requirement-18-october)).
macOS 27.0.1 (26A434), Xcode 27.0 (27A5194q), Apple M5 Pro.

Nothing was registered for this work: no SMAppService call, no `launchctl` change, no
LaunchServices or pluginkit change and no `defaults`. The self-tests ran from a plain `swiftc` build
of the app's sources in the scratchpad. That is a bare executable, not a bundle, so nothing
registered it, and its SMAppService services were "not found". `/Applications/voidfs.app` was only
read: its signature and `launchctl print` for its agent.

## What changed

`apps/macos/App/LaunchRequirement.swift`, shared by both jobs, which run the same binary:
- **The check** reads `launchctl print gui/<uid>/<label>`. launchd refuses the job if its LWCR
  names another `signing-identifier`, `team-identifier` or `validation-category` than this binary's
  own, or if its last exit code is 78 (EX_CONFIG). The agent's kept requirement had the right
  identifier and the wrong category, which #84's check, comparing identifiers only, didn't see.
- **This binary's category** comes from its certificate, by code requirement: Apple Development
  (leaf `1.2.840.113635.100.6.1.12`) is 3, Developer ID (intermediate `…6.2.6` and leaf `…6.1.13`)
  is 6. Any other is left out and not compared ([signatures.txt](raw/signatures.txt): two
  development builds meet only the first, `/Applications` only the second).
- **A job launchd runs now isn't refused**, whatever its requirement. With two copies registered,
  another copy's program may be the one running.

`apps/macos/App/Agent.swift`:
- `Agent.repair` registers the agent again while launchd refuses it, up to twice, asking
  `launchctl` again after each round, and then says what it found. That includes a refusal that
  stays, or a registration that failed.
- **At the app's launch**, once per run: mounts need the agent whether or not the menu opens. The
  menu shows the note, for example "launchd refused the bridge agent (its launch requirement
  names validation category 3 (development), not 6 (Developer ID)), so it was registered again
  twice."
- **`agent-register`** does the same after registering, and exits 1 if launchd still refuses the
  agent. **`agent-status`** reports a refusal and changes nothing. **Turning Start at login on**
  checks the agent when it registers it.

`apps/macos/App/LoginItem.swift` learned the same:
- **The category.** On 18 October `login-item on` caught the job's kept requirement only by exit
  code 78, after launchd had tried the program ([login-item-on.txt](../item7-login/raw/login-item-on.txt)).
  The category catches it before that.
- **Up to two rounds.** The agent's first round kept its requirement. The menu's check and
  turning it on while it's on start from an enabled job, as the agent did, so one round may not
  be enough there either. The wait registers it again at most twice in all, and says if launchd
  still refuses it.
- `login-item look` no longer runs the menu's repairs, so that it registers nothing, as it says.

## The self-tests

Built with `swiftc -swift-version 5 -target arm64-apple-macos27.0 -O` (Swift 6.4) from `App`,
`Shared` and `Badges`, then signed with the Apple Development identity as `dev.voidfs.app`, as Xcode
signs the app ([run-all.sh](raw/scripts/run-all.sh), [breaks.py](raw/scripts/breaks.py), with the
identity in `IDENTITY`).

**`voidfs agent-selftest`** passed all 21 checks ([output](raw/agent-selftest.txt)):
- the check, for the Developer ID copy, against the live run's states in order: the first look,
  as `agent.txt` recorded it (an excerpt, whose requirement block has no end); after one round,
  scheduled with the kept requirement; after `bridge-check`, exit 78; and after two rounds,
  running. The later three are laid out as this Mac prints the agent now. Also: an idle agent and
  a clean exit with its own requirement, another team, a Developer ID requirement kept for a
  development build (the 9 October direction), a running agent, no known category, an unsigned
  binary, and a category after the requirement's block;
- this binary's signature: `dev.voidfs.app`, team `HAUTK68F56`, category 3 (development);
- the rounds, with launchd's view after each round taken from those states: the live run (two
  rounds), one round, none, a refusal that stays (two rounds, not more), and a registration that
  fails, with each note.

It also printed `/Applications/voidfs.app` as `dev.voidfs.app`, team `HAUTK68F56`, category 6
(Developer ID), read from its signature.

**`voidfs login-item selftest`** passed all 23 checks ([output](raw/login-item-selftest.txt)): #84's
15, adjusted for the shared check and the second round, and new ones for the 18 October case (a
development requirement kept for the Developer ID copy, before launchd has tried the program), the
Developer ID copy's own requirement, a running job, the agent's two rounds as the job's, a refusal
that stays, and the wait after a round of its own.

The same build ad hoc signed, as `swiftc` links it, fails only the signature checks, as it should
([unsigned.txt](raw/unsigned.txt)).

## Breaks

Each break is a copy of the sources with one change, built and signed the same way. The worktree's
sources weren't edited ([output](raw/breaks.txt)). Where there was older code, the break puts it
back as it was.

| Break | agent-selftest | login-item selftest |
| --- | --- | --- |
| 1. The check as #84 had it: only the signing identifier compared | 12 failed | 3 failed |
| 2. Registered again once, as #84 did (`rounds = 1`) | 4 failed | 4 failed |
| 3. A running job judged by its requirement, as before | 1 failed | 1 failed |
| 4. The requirement's block doesn't end | 1 failed | 1 failed |
| 5. The agent's rounds: one more than allowed | 2 failed | passed (not its code) |
| 6. The agent's rounds: not asked again after a round | 4 failed | passed (not its code) |
| 7. The job's wait: registered again on every refusal | passed (not its code) | 3 failed |

Break 4 passed both at first, because the check keeps the first value of each key: a key after
the block counted only when the block had none of its own. Both block-end checks now use a
requirement that lacks the key the later block names.

## Not covered

- **A live run.** A kept requirement needs a switch between registered copies, which is a live
  registration.
- **The Xcode build**, which would register another copy with LaunchServices and pluginkit. The
  plain build covers the same sources but not Xcode's settings.
- **Two differently signed copies registered at once.** While the agent is idle, each sees the
  other's requirement as a refusal and would register it again for itself. Keep one copy
  registered, as before.
- **At login.** The check runs when the app opens, so after an update the app must be opened once.
  The login job runs this binary at every login and could check the agent too.

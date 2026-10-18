# Each break: a copy of the app's sources with one change, built, signed for development as
# dev.voidfs.app (the identity in $IDENTITY), and both self-tests run. The worktree's sources are
# never edited. Usage: breaks.py <apps/macos> <out>
import os, shutil, subprocess, sys
src = sys.argv[1]
out = sys.argv[2]
breaks = [
    ("the check as #84 had it: only the signing identifier compared", "App/LaunchRequirement.swift",
     """            if let team = named["team-identifier"], let ownTeam = own.team, team != ownTeam {
                return "its launch requirement names team \\(team), not \\(ownTeam)"
            }
            if let category = named["validation-category"].flatMap({ Int($0) }), let ownCategory = own.category, category != ownCategory {
                return "its launch requirement names validation category \\(describe(category: category)), not \\(describe(category: ownCategory))"
            }
""", ""),
    ("registered again once, as #84 did", "App/LaunchRequirement.swift",
     "    static let rounds = 2\n", "    static let rounds = 1\n"),
    ("a running job judged by its requirement, as before", "App/LaunchRequirement.swift",
     """        if lines.first(where: { tabs($0) == 1 && trimmed($0).hasPrefix("state = ") }).map(trimmed) == "state = running" { return nil }\n""", ""),
    ("the requirement's block doesn't end", "App/LaunchRequirement.swift",
     """                if t == "}" && tabs(line) == depth { break }\n""", ""),
    ("the agent's rounds: one more than allowed", "App/Agent.swift",
     "        while repair.rounds < LaunchRequirement.rounds {\n", "        while repair.rounds <= LaunchRequirement.rounds {\n"),
    ("the agent's rounds: not asked again after a round", "App/Agent.swift",
     "            repair.still = await refused()\n", "            repair.still = nil\n"),
    ("the job's wait: registered again on every refusal", "App/LoginItem.swift",
     "            if waited.rounds < LaunchRequirement.rounds, ContinuousClock.now - checked >= .seconds(1) {\n",
     "            if ContinuousClock.now - checked >= .seconds(1) {\n"),
]
os.makedirs(out, exist_ok=True)
for i, (name, path, old, new) in enumerate(breaks, 1):
    d = os.path.join(out, f"b{i}")
    shutil.rmtree(d, ignore_errors=True)
    for sub in ["App", "Shared", "Badges"]:
        shutil.copytree(os.path.join(src, sub), os.path.join(d, sub))
    f = os.path.join(d, path)
    s = open(f).read()
    assert s.count(old) == 1, (name, s.count(old))
    open(f, "w").write(s.replace(old, new))
    diff = subprocess.run(["/usr/bin/diff", "-u", os.path.join(src, path), f], capture_output=True, text=True).stdout
    files = [os.path.join(d, sub, n) for sub in ["App", "Shared", "Badges"] for n in sorted(os.listdir(os.path.join(d, sub))) if n.endswith(".swift")]
    exe = os.path.join(d, "voidfs-break")
    b = subprocess.run(["xcrun", "swiftc", "-swift-version", "5", "-target", "arm64-apple-macos27.0", "-Onone", "-o", exe] + files, capture_output=True, text=True)
    print(f"## break {i}: {name}")
    print("\n".join(l for l in diff.splitlines()[2:] if l.startswith(("-", "+"))))
    if b.returncode != 0:
        print("BUILD FAILED\n" + b.stderr[-2000:])
        continue
    subprocess.run(["codesign", "-f", "-s", os.environ["IDENTITY"], "-i", "dev.voidfs.app", exe], capture_output=True, check=True)
    for test in [["agent-selftest"], ["login-item", "selftest"]]:
        r = subprocess.run([exe] + test, capture_output=True, text=True, timeout=120)
        lines = r.stdout.splitlines()
        for l in lines:
            if l.startswith("FAIL") or "self-test" in l: print(l)
        print(f"{' '.join(test)}: exit {r.returncode}, {sum(l.startswith(('ok', 'FAIL')) for l in lines)} checks")
    print()
    sys.stdout.flush()

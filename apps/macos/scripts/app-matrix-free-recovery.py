#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Restore server versions of separate synthetic Blender/LibreOffice document copies.

Existing app-matrix-free.py fixtures supply actual native first/edited file bytes. Blender's
first save lives in .blend1; Writer/Calc's first saves come from nonempty earlier history
versions. A fresh mounted subtree receives the first then edited bytes, publishes both, and
restores its own first versions. Fresh native processes check restored semantics. This tests
version restore, never process-crash recovery, and leaves the original app rows untouched.
Credentials and isolated daemon state are inherited, never printed.
"""

import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mount", type=Path, required=True)
    parser.add_argument("--drive", required=True)
    parser.add_argument("--void", type=Path, required=True)
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--source-root", default="free-apps-20261013", help="existing synthetic free-app subtree key")
    parser.add_argument("--blender", type=Path, required=True)
    parser.add_argument("--libreoffice", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    mount, output = args.mount.absolute(), args.out.absolute()
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="free-app-recovery-", dir=mount))
    scripts = Path(__file__).absolute().parent
    report = {"coverage": "server version restore and native reopen; no crash test", "root": str(root), "operations": [], "documents": []}

    def run(name, command, binary=False, quiet=False):
        started = time.monotonic()
        result = subprocess.run([str(x) for x in command], stdout=subprocess.DEVNULL if quiet else subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=180)
        operation = {"name": name, "command": [str(x) for x in command], "exit_code": result.returncode,
                     "seconds": round(time.monotonic() - started, 3), "stderr": result.stderr.decode(errors="replace")}
        if binary:
            operation.update(size=len(result.stdout), sha256=sha(result.stdout))
        elif not quiet:
            operation["stdout"] = result.stdout.decode(errors="replace")
        report["operations"].append(operation)
        print(f"{name}: exit {result.returncode}", flush=True)
        if result.returncode:
            raise RuntimeError(f"{name}: {operation['stderr']}")
        return result.stdout

    def history(key):
        return json.loads(run("history " + key, [args.void, "--json", "history", args.drive, key, "--all"]))["versions"]

    def payload(key, version):
        return run("read native version " + key, [args.void, "show", args.drive, key, "--version", version], binary=True)

    def latest(versions):
        return next(v for v in versions if v.get("isLatest"))

    def collect(name):
        run("collector " + name, [sys.executable, scripts / "app-matrix.py", "--row", "collect", "--root", root, "--out", output / (name + ".jsonl"),
                                   "--drive", args.drive, "--mount", mount, "--void", args.void, "--metadata", args.metadata], quiet=True)

    try:
        fixtures = [
            ("Blender", "blender/scene café.blend", "blender/scene café.blend1"),
            ("LibreOffice Writer", "libreoffice/Writer café.odt", None),
            ("LibreOffice Calc", "libreoffice/Calc café.ods", None),
        ]
        contents = []
        for app, relative, backup in fixtures:
            key = str(Path(args.source_root) / relative)
            versions = history(key)
            current = latest(versions)
            old_key = str(Path(args.source_root) / backup) if backup else key
            initial = latest(history(old_key)) if backup else next(v for v in versions if v.get("size", 0) > 0 and not v.get("isLatest"))
            before, after = payload(old_key, initial["versionId"]), payload(key, current["versionId"])
            if before == after:
                raise AssertionError(f"{app}: first and edited bytes are not distinct")
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(before)
            contents.append((path, before, after))
            report["documents"].append({"app": app, "relative": relative, "source_initial_key": old_key, "source_initial_version": initial["versionId"],
                                         "source_edited_key": key, "source_edited_version": current["versionId"], "initial_sha256": sha(before), "edited_sha256": sha(after)})
        texture_key = str(Path(args.source_root) / "blender/assets/texture.png")
        texture = payload(texture_key, latest(history(texture_key))["versionId"])
        assets = root / "blender" / "assets"
        assets.mkdir()
        (assets / "texture.png").write_bytes(texture)
        collect("initial-published")
        for document, (path, before, after) in zip(report["documents"], contents):
            key = str(path.relative_to(mount))
            document["recovery_key"] = key
            document["recovery_initial_version"] = latest(history(key))["versionId"]
            path.write_bytes(after)
        collect("edited-published")
        for document, (path, before, after) in zip(report["documents"], contents):
            document["recovery_edited_version"] = latest(history(document["recovery_key"]))["versionId"]
            run("restore earlier " + document["app"], [args.void, "--json", "restore", args.drive, document["recovery_key"], "--version", document["recovery_initial_version"]])
            started = time.monotonic()
            while True:
                if path.read_bytes() == before:
                    break
                if time.monotonic() - started > 5:
                    raise AssertionError(f"{document['app']}: restored bytes not visible within 5 seconds")
                time.sleep(0.05)
            document["restore_visible_seconds"] = round(time.monotonic() - started, 3)
            document["restored_sha256"] = sha(path.read_bytes())
            document["restored_bytes_match_initial"] = True
        collect("restored-published")
        run("native reopen restored first versions", [sys.executable, scripts / "app-matrix-free.py", "--folder", root, "--output", output / "native-reopen", "--label", "FSKit server-restored first versions",
                                                       "--verify-initial", "--blender", args.blender, "--libreoffice", args.libreoffice], quiet=True)
        native = json.loads((output / "native-reopen" / "results.json").read_text())
        report["native_checks"] = sum(len(x.get("checks", [])) for x in native["operations"])
        report["native_passed"] = native["passed"]
        if not native["passed"]:
            raise AssertionError("restored native semantics failed")
        collect("after-native-reopen")
        report["passed"] = True
    except Exception as error:
        report["passed"] = False
        report["error"] = str(error)
        print(str(error), file=sys.stderr)
    finally:
        (output / "results.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    return 0 if report.get("passed") else 1


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Native save/reopen controls for temporary copies of Blender and LibreOffice on a test volume.

Pass a NEW --folder on APFS or a test mount, and --output outside the mount. This creates only
synthetic files, isolates app profiles, launches LibreOffice with a private named UNO pipe,
and reports semantic checks and hashes/xattrs for later server verification. The apps must
already be available; this script neither downloads software nor changes /Applications.
Blender phases each run in a fresh process; LibreOffice phases each use a fresh office process.
The latter requires that installation's bundled Python/UNO bridge, with no added dependency.
"""

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for piece in iter(lambda: f.read(1 << 20), b""):
            h.update(piece)
    return h.hexdigest()


def xattrs(path):
    """Apple's system Python omits os.listxattr/getxattr; use its bundled xattr command."""
    names = subprocess.run(["/usr/bin/xattr", str(path)], check=True, capture_output=True, text=True).stdout.splitlines()
    values = {}
    for name in names:
        raw = subprocess.run(["/usr/bin/xattr", "-px", name, str(path)], check=True, capture_output=True, text=True).stdout
        value = bytes.fromhex(raw)
        values[name] = {"size": len(value), "sha256": hashlib.sha256(value).hexdigest()}
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--folder", type=Path, required=True, help="new synthetic-test directory")
    parser.add_argument("--output", type=Path, required=True, help="logs/results outside test volume")
    parser.add_argument("--blender", type=Path)
    parser.add_argument("--libreoffice", type=Path, help="app's Contents/MacOS/soffice")
    parser.add_argument("--lo-python", type=Path, help="defaults to the app's Contents/Resources/python")
    parser.add_argument("--label", default="control")
    parser.add_argument("--collect-only", action="store_true", help="collect artifacts and saved phase results without rerunning apps")
    parser.add_argument("--verify-initial", action="store_true", help="fresh native reopen of existing restored first-revision documents, without edits")
    args = parser.parse_args()
    if not args.blender and not args.libreoffice and not args.collect_only:
        parser.error("select at least one app")
    folder, output = args.folder.absolute(), args.output.absolute()
    if args.collect_only and args.verify_initial:
        parser.error("select either collection or first-revision verification")
    if args.collect_only or args.verify_initial:
        if not folder.is_dir():
            parser.error("collection/verification requires the previous test folder")
    else:
        folder.mkdir(parents=True, exist_ok=False)
    output.mkdir(parents=True, exist_ok=True)
    report = {"label": args.label, "started": datetime.now(timezone.utc).isoformat(), "folder": str(folder),
              "platform": platform.platform(), "coverage": "native background create/save/reopen/edit/save", "operations": [], "artifacts": []}
    env = dict(os.environ)
    for name in ("PYTHONPATH", "PYTHONHOME"):
        env.pop(name, None)
    scripts = Path(__file__).absolute().parent
    profile_root = Path(tempfile.mkdtemp(prefix="voidfs-free-profiles-"))
    phases = ("verify-initial",) if args.verify_initial else ("create", "edit", "verify")
    if args.verify_initial:
        report["coverage"] = "fresh native reopen of server-restored first revisions; no process-crash test"

    def run(name, command, extra_env=None, timeout=180):
        start = time.monotonic()
        completed = subprocess.run([str(x) for x in command], env=extra_env or env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout)
        (output / (name + ".log")).write_bytes(completed.stdout)
        report["operations"].append({"name": name, "command": [str(x) for x in command], "exit_code": completed.returncode, "seconds": round(time.monotonic() - start, 3)})
        print(f"{name}: exit {completed.returncode}, {report['operations'][-1]['seconds']} s", flush=True)
        if completed.returncode:
            raise RuntimeError(f"{name}: exit {completed.returncode}; see {output / (name + '.log')}")

    def import_checks(path):
        data = json.loads(path.read_text())
        report["operations"][-1]["checks"] = data["checks"]
        if not data.get("passed"):
            raise RuntimeError(f"native checks failed: {path}")

    try:
        if args.collect_only:
            for app in ("blender", "libreoffice"):
                expected = [output / f"{app}-{phase}.json" for phase in ("create", "edit", "verify")]
                selected = (output / f"{app}-version.log").is_file() or any(path.is_file() for path in expected)
                if selected and not all(path.is_file() for path in expected):
                    raise RuntimeError(f"{app}: native phase results incomplete")
                for phase in ("create", "edit", "verify"):
                    path = output / f"{app}-{phase}.json"
                    if path.is_file():
                        report["operations"].append({"name": f"{app}-{phase}", "source": str(path)})
                        import_checks(path)
            if not report["operations"]:
                raise RuntimeError("no saved native phase results to collect")
        if args.blender and not args.collect_only:
            blender_env = dict(env, BLENDER_USER_RESOURCES=str(profile_root / "blender"))
            run("blender-version", [args.blender, "--version"], blender_env)
            target = folder / "blender"
            target.mkdir(exist_ok=args.verify_initial)
            for phase in phases:
                result = output / f"blender-{phase}.json"
                run(f"blender-{phase}", [args.blender, "--background", "--factory-startup", "--python-exit-code", "1", "--python", scripts / "app-matrix-blender.py", "--", target, phase, result], blender_env)
                import_checks(result)
        if args.libreoffice and not args.collect_only:
            python = args.lo_python or args.libreoffice.parent.parent / "Resources" / "python"
            run("libreoffice-version", [args.libreoffice, "-env:UserInstallation=" + (profile_root / "lo-version").as_uri(), "--headless", "--version"])
            run("libreoffice-python-version", [python, "-c", "import sys,uno; print(sys.version); print(uno.__file__)"])
            target = folder / "libreoffice"
            target.mkdir(exist_ok=args.verify_initial)
            for phase in phases:
                pipe = "voidfs_test_" + uuid.uuid4().hex
                profile = profile_root / f"lo-{phase}"
                command = [str(args.libreoffice), "-env:UserInstallation=" + profile.as_uri(), "--headless", "--nologo", "--nodefault", "--norestore", "--accept=pipe,name=" + pipe + ";urp;StarOffice.ComponentContext"]
                with (output / f"libreoffice-{phase}-office.log").open("wb") as log:
                    office = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
                    try:
                        result = output / f"libreoffice-{phase}.json"
                        run(f"libreoffice-{phase}", [python, scripts / "app-matrix-libreoffice.py", target, phase, pipe, result])
                        import_checks(result)
                    finally:
                        try:
                            office.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            office.terminate()
                            try:
                                office.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                office.kill()
                                office.wait()
        report["passed"] = True
    except Exception as error:
        report["passed"] = False
        report["error"] = str(error)
        print(str(error), file=sys.stderr)
    finally:
        try:
            for path in sorted(folder.rglob("*")):
                if path.is_file():
                    artifact = {"path": str(path.relative_to(folder)), "size": path.stat().st_size, "sha256": digest(path), "mode": oct(path.stat().st_mode & 0o7777), "xattrs": xattrs(path)}
                    report["artifacts"].append(artifact)
        except Exception as error:
            report["passed"] = False
            report["artifact_error"] = str(error)
        (output / "results.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        shutil.rmtree(profile_root)
    return 0 if report.get("passed") else 1


if __name__ == "__main__":
    sys.exit(main())

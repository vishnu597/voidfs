#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Build a synthetic Swift Xcode project on APFS or a test mount, keeping build caches in tmp.

  app-matrix-xcode.py --root /private/tmp/control --out xcode-apfs.jsonl

Every run owns a fresh child of --root. Sources, pbxproj and shared scheme live there;
DerivedData, module caches and build products live in a private /private/tmp directory which
is removed after the run. No package dependencies, signing, Xcode GUI interactions, or writes
to the user's DerivedData directory. Common app-matrix.py collection verifies publication.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


PBXPROJ = """// !$*UTF8*$!
{
    archiveVersion = 1;
    classes = {};
    objectVersion = 56;
    objects = {
        A00000000000000000000001 = {isa = PBXBuildFile; fileRef = A00000000000000000000002; };
        A00000000000000000000002 = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = main.swift; sourceTree = "<group>"; };
        A00000000000000000000003 = {isa = PBXFileReference; explicitFileType = "compiled.mach-o.executable"; includeInIndex = 0; path = MatrixFixture; sourceTree = BUILT_PRODUCTS_DIR; };
        A00000000000000000000004 = {isa = PBXGroup; children = (A00000000000000000000005, A00000000000000000000006); sourceTree = "<group>"; };
        A00000000000000000000005 = {isa = PBXGroup; children = (A00000000000000000000002); path = Sources; sourceTree = "<group>"; };
        A00000000000000000000006 = {isa = PBXGroup; children = (A00000000000000000000003); name = Products; sourceTree = "<group>"; };
        A00000000000000000000007 = {isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (A00000000000000000000001); runOnlyForDeploymentPostprocessing = 0; };
        A00000000000000000000008 = {isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; };
        A00000000000000000000009 = {isa = PBXNativeTarget; buildConfigurationList = A00000000000000000000010; buildPhases = (A00000000000000000000007, A00000000000000000000008); buildRules = (); dependencies = (); name = MatrixFixture; productName = MatrixFixture; productReference = A00000000000000000000003; productType = "com.apple.product-type.tool"; };
        A0000000000000000000000A = {isa = PBXProject; attributes = {LastUpgradeCheck = 2750; }; buildConfigurationList = A0000000000000000000000B; compatibilityVersion = "Xcode 14.0"; developmentRegion = en; hasScannedForEncodings = 0; knownRegions = (en, Base); mainGroup = A00000000000000000000004; productRefGroup = A00000000000000000000006; projectDirPath = ""; projectRoot = ""; targets = (A00000000000000000000009); };
        A0000000000000000000000C = {isa = XCBuildConfiguration; buildSettings = {ALWAYS_SEARCH_USER_PATHS = NO; ARCHS = arm64; MACOSX_DEPLOYMENT_TARGET = 13.0; ONLY_ACTIVE_ARCH = YES; SDKROOT = macosx; }; name = Debug; };
        A0000000000000000000000D = {isa = XCBuildConfiguration; buildSettings = {ALWAYS_SEARCH_USER_PATHS = NO; ARCHS = arm64; MACOSX_DEPLOYMENT_TARGET = 13.0; SDKROOT = macosx; }; name = Release; };
        A0000000000000000000000E = {isa = XCBuildConfiguration; buildSettings = {CODE_SIGNING_ALLOWED = NO; CODE_SIGNING_REQUIRED = NO; PRODUCT_NAME = "$(TARGET_NAME)"; SWIFT_OPTIMIZATION_LEVEL = "-Onone"; SWIFT_VERSION = 5.0; }; name = Debug; };
        A0000000000000000000000F = {isa = XCBuildConfiguration; buildSettings = {CODE_SIGNING_ALLOWED = NO; CODE_SIGNING_REQUIRED = NO; PRODUCT_NAME = "$(TARGET_NAME)"; SWIFT_OPTIMIZATION_LEVEL = "-O"; SWIFT_VERSION = 5.0; }; name = Release; };
        A00000000000000000000010 = {isa = XCConfigurationList; buildConfigurations = (A0000000000000000000000E, A0000000000000000000000F); defaultConfigurationIsVisible = 0; defaultConfigurationName = Debug; };
        A0000000000000000000000B = {isa = XCConfigurationList; buildConfigurations = (A0000000000000000000000C, A0000000000000000000000D); defaultConfigurationIsVisible = 0; defaultConfigurationName = Debug; };
    };
    rootObject = A0000000000000000000000A;
}
"""

SCHEME = """<?xml version="1.0" encoding="UTF-8"?>
<Scheme LastUpgradeVersion="2750" version="1.3">
  <BuildAction parallelizeBuildables="YES" buildImplicitDependencies="YES">
    <BuildActionEntries><BuildActionEntry buildForTesting="YES" buildForRunning="YES" buildForProfiling="YES" buildForArchiving="YES" buildForAnalyzing="YES">
      <BuildableReference BuildableIdentifier="primary" BlueprintIdentifier="A00000000000000000000009" BuildableName="MatrixFixture" BlueprintName="MatrixFixture" ReferencedContainer="container:MatrixFixture.xcodeproj"/>
    </BuildActionEntry></BuildActionEntries>
  </BuildAction>
  <LaunchAction buildConfiguration="Debug" selectedDebuggerIdentifier="Xcode.DebuggerFoundation.Debugger.LLDB" selectedLauncherIdentifier="Xcode.IDEFoundation.Launcher.LLDB" launchStyle="0" useCustomWorkingDirectory="NO" ignoresPersistentStateOnLaunch="NO" debugDocumentVersioning="YES" debugServiceExtension="internal" allowLocationSimulation="YES">
    <BuildableProductRunnable runnableDebuggingMode="0"><BuildableReference BuildableIdentifier="primary" BlueprintIdentifier="A00000000000000000000009" BuildableName="MatrixFixture" BlueprintName="MatrixFixture" ReferencedContainer="container:MatrixFixture.xcodeproj"/></BuildableProductRunnable>
  </LaunchAction>
</Scheme>
"""


def source(revision):
    return f'''import Foundation
let revision = {revision}
let values = [3, 5, 8]
print("voidfs Xcode fixture revision \\(revision): \\(values.reduce(0, +))")
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True, help="append-only JSONL evidence")
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    parent = args.root.absolute()
    parent.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="xcode-", dir=parent))
    output = args.out.absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    failures = 0

    def emit(record):
        record["time"] = time.time()
        with output.open("a") as log:
            log.write(json.dumps(record, ensure_ascii=False) + "\n")
        print(json.dumps(record, ensure_ascii=False), flush=True)

    def check(name, passed, **detail):
        nonlocal failures
        failures += not passed
        emit({"kind": "check", "name": name, "ok": bool(passed), **detail})

    def run(name, command, env=None):
        nonlocal failures
        started = time.monotonic()
        completed = subprocess.run([str(x) for x in command], capture_output=True, text=True, timeout=args.timeout, env=env)
        log = output.with_name(output.stem + "-" + name + ".log")
        log.write_text(completed.stdout + completed.stderr)
        emit({"kind": "operation", "name": name, "argv": [str(x) for x in command], "exit_code": completed.returncode,
              "seconds": round(time.monotonic() - started, 4), "stdout": completed.stdout if name.startswith("execute") or name == "version" else "see log", "log": str(log)})
        if completed.returncode:
            failures += 1
            raise RuntimeError(f"{name} exited {completed.returncode}")
        return completed.stdout

    project = root / "MatrixFixture.xcodeproj"
    sources = root / "Sources"
    sources.mkdir()
    project.mkdir()
    scheme = project / "xcshareddata" / "xcschemes"
    scheme.mkdir(parents=True)
    main_swift = sources / "main.swift"
    main_swift.write_text(source(1))
    (project / "project.pbxproj").write_text(PBXPROJ)
    (scheme / "MatrixFixture.xcscheme").write_text(SCHEME)
    emit({"kind": "run", "row": "Xcode command-line project", "root": str(root), "coverage": "xcodebuild reads project and sources from volume; build products/caches are private tmp; no GUI claim"})
    try:
        run("version", ["/usr/bin/xcrun", "xcodebuild", "-version"])
        run("project-plutil", ["/usr/bin/plutil", "-lint", project / "project.pbxproj"])
        with tempfile.TemporaryDirectory(prefix="voidfs-xcode-derived-", dir="/private/tmp") as scratch:
            scratch = Path(scratch)
            derived = scratch / "DerivedData"
            env = dict(os.environ, CLANG_MODULE_CACHE_PATH=str(scratch / "clang-module-cache"), SWIFT_MODULECACHE_PATH=str(scratch / "swift-module-cache"))
            build = ["/usr/bin/xcrun", "xcodebuild", "-project", project, "-scheme", "MatrixFixture", "-configuration", "Debug", "-derivedDataPath", derived,
                     "-destination", "platform=macOS,arch=arm64", "CODE_SIGNING_ALLOWED=NO", "CODE_SIGNING_REQUIRED=NO", "CODE_SIGN_IDENTITY=", "CLANG_MODULE_CACHE_PATH=" + str(scratch / "clang-module-cache"), "SWIFT_MODULECACHE_PATH=" + str(scratch / "swift-module-cache")]
            run("clean-build", [*build, "clean", "build"], env)
            executable = derived / "Build" / "Products" / "Debug" / "MatrixFixture"
            check("first build product exists outside tested volume", executable.is_file(), path=str(executable))
            first = run("execute-first", [executable], env)
            check("first build executes initial source", first == "voidfs Xcode fixture revision 1: 16\n", stdout=first)
            before = hashlib.sha256(main_swift.read_bytes()).hexdigest()
            main_swift.write_text(source(2))
            check("source edit reached tested volume", before != hashlib.sha256(main_swift.read_bytes()).hexdigest())
            run("edited-build", [*build, "build"], env)
            edited = run("execute-edited", [executable], env)
            check("incremental build executes edited source", edited == "voidfs Xcode fixture revision 2: 16\n", stdout=edited)
            emit({"kind": "product", "path": str(executable), "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(), "size": executable.stat().st_size,
                  "derivedData": str(derived), "temporaryBuildProductsRemovedAfterRun": True})
    except Exception as error:
        failures += 1
        emit({"kind": "error", "type": type(error).__name__, "message": str(error)})
    for path in sorted(root.rglob("*")):
        if path.is_file():
            emit({"kind": "artifact", "path": str(path.relative_to(root)), "size": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    emit({"kind": "summary", "row": "Xcode command-line project", "root": str(root), "failures": failures, "result": "fail" if failures else "pass"})
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())

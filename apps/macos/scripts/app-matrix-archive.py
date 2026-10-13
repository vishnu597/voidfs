#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Losslessly archive closed application-matrix logs, with a checksum manifest.

Without --apply this prints a plan and changes nothing. --apply checks lsof for
writers, creates deterministic gzip files, verifies their decoded bytes, records
the manifest, updates only Markdown links in the explicit file map, and removes
the verified originals. Stop all evidence writers before applying the plan.
"""

import argparse
import datetime
import gzip
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import unquote, urlsplit


BLOCK = 1024 * 1024
INLINE_LINK = re.compile(r'(?P<prefix>!?\[[^\n]*?\]\()(?P<target><[^>\n]+>|[^\s)\n]+)(?P<suffix>(?:[ \t]+(?:"[^"\n]*"|\x27[^\x27\n]*\x27))?[ \t]*\))')
REFERENCE_LINK = re.compile(r'^(?P<prefix>[ \t]{0,3}\[[^\]\n]+\]:[ \t]*)(?P<target><[^>\n]+>|[^\s\n]+)(?P<suffix>.*)$')


def fingerprint(path):
    value = path.stat()
    return (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)


def digest_stream(stream):
    digest, count = hashlib.sha256(), 0
    while True:
        block = stream.read(BLOCK)
        if not block:
            return count, digest.hexdigest()
        digest.update(block)
        count += len(block)


def digest_file(path, compressed=False):
    with (gzip.open(path, "rb") if compressed else path.open("rb")) as stream:
        return digest_stream(stream)


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def atomic_bytes(path, content, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".archive-", dir=path.parent)
    temporary = Path(temporary)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        temporary.chmod(mode)
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        temporary.unlink(missing_ok=True)


def assert_no_writers(paths):
    if not paths:
        return
    executable = shutil.which("lsof")
    if executable is None:
        raise RuntimeError("lsof is required to check for active log writers")
    # lsof fields: process ID, descriptor, access (r/w/u), file name. A read-only
    # consumer can keep its descriptor; any writing or ambiguous open blocks us.
    result = subprocess.run([executable, "-Fpnfa", "--", *map(str, paths)],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30)
    if result.returncode not in (0, 1) or result.stderr.strip():
        raise RuntimeError("cannot establish closed logs: " + result.stderr.strip())
    if result.returncode == 1 and not result.stdout:
        return
    access, process, descriptor, opened = None, None, None, []
    for line in result.stdout.splitlines():
        if line.startswith("p"):
            process = line[1:]
        elif line.startswith("f"):
            descriptor, access = line[1:], None
        elif line.startswith("a"):
            access = line[1:]
        elif line.startswith("n") and access != "r":
            opened.append({"pid": process, "descriptor": descriptor, "access": access, "path": line[1:]})
    if opened:
        raise RuntimeError("active log writers: " + json.dumps(opened, ensure_ascii=False))


def relative_file(root, relative):
    path = Path(relative)
    if path.is_absolute() or ".." in path.parts or not path.parts:
        raise ValueError("manifest contains a nonrelative file path")
    return root / path


def link_target(target, document, file_map):
    angled = target.startswith("<") and target.endswith(">")
    value = target[1:-1] if angled else target
    url = urlsplit(value)
    if url.scheme or url.netloc or not url.path:
        return target
    source = Path(unquote(url.path))
    source = (source if source.is_absolute() else document.parent / source).resolve()
    destination = file_map.get(source)
    if destination is None:
        return target
    # Every map entry adds exactly .gz; retain the caller's relative spelling,
    # escaping, query and fragment instead of rewriting unrelated path text.
    value = url.path + ".gz" + (("?" + url.query) if url.query else "") + (("#" + url.fragment) if url.fragment else "")
    return "<" + value + ">" if angled else value


def code_intervals(content):
    intervals, fence, offset, start = [], None, 0, 0
    for line in content.splitlines(keepends=True):
        marker = re.match(r"^[ \t]{0,3}(`{3,}|~{3,})", line)
        if marker:
            value = marker.group(1)
            if fence is None:
                fence, start = value, offset
            elif value[0] == fence[0] and len(value) >= len(fence):
                intervals.append((start, offset + len(line)))
                fence = None
        offset += len(line)
    if fence is not None:
        intervals.append((start, len(content)))

    def protected(position):
        return any(start <= position < end for start, end in intervals)

    offset = 0
    while offset < len(content):
        opening = re.search(r"`+", content[offset:])
        if opening is None:
            break
        start, end = offset + opening.start(), offset + opening.end()
        token = opening.group()
        if protected(start):
            offset = end
            continue
        closing = re.search(r"(?<!`)" + re.escape(token) + r"(?!`)", content[end:])
        if closing is not None and not protected(end + closing.start()):
            finish = end + closing.end()
            intervals.append((start, finish))
            offset = finish
        else:
            offset = end
    return intervals


def update_markdown(content, document, file_map):
    intervals, changes = code_intervals(content), 0

    def replace(match):
        nonlocal changes
        # The label may itself contain code; only literal code around the link
        # or its destination prevents a rewrite. Code spans can cross lines.
        if any(start <= match.start() < end or start <= match.start("target") < end for start, end in intervals):
            return match.group()
        target = link_target(match.group("target"), document, file_map)
        changes += target != match.group("target")
        return match.group("prefix") + target + match.group("suffix")

    # Apply replacements from original offsets, so edits cannot shift code-span
    # protection or the coordinates of another link on the same line.
    replacements = []
    for expression in [INLINE_LINK, re.compile(REFERENCE_LINK.pattern, re.MULTILINE)]:
        for match in expression.finditer(content):
            value = replace(match)
            if value != match.group():
                replacements.append((match.start(), match.end(), value))
    for start, end, value in sorted(replacements, reverse=True):
        content = content[:start] + value + content[end:]
    return content, changes


def archive(raw, manifest, markdown, minimum, apply):
    raw, manifest = raw.resolve(), manifest.absolute()
    if not raw.is_dir():
        raise ValueError("--raw-dir must be an existing directory")
    records = []
    if manifest.exists():
        prior = json.loads(manifest.read_text())
        if prior.get("format") != 1 or prior.get("raw_dir") != str(raw):
            raise ValueError("manifest does not describe this raw directory")
        records = prior["files"]
        for record in records:
            relative_file(raw, record["source"])
            compressed = relative_file(raw, record["archive"])
            if record["archive"] != record["source"] + ".gz" or compressed.is_symlink():
                raise ValueError("manifest contains an unexpected archive path")
            if digest_file(compressed) != (record["archive_bytes"], record["archive_sha256"]) or digest_file(compressed, compressed=True) != (record["source_bytes"], record["source_sha256"]):
                raise ValueError("existing manifest checksum mismatch: " + str(compressed))
    existing = {record["source"]: record for record in records}
    candidates = []
    for source in sorted(raw.rglob("*")):
        if source.is_symlink():
            raise ValueError("raw directory contains a symlink: " + str(source))
        if not source.is_file() or source.suffix == ".gz" or source.name.startswith(".archive-") or source.absolute() == manifest:
            continue
        if source.stat().st_size >= minimum:
            candidates.append(source)
    plan = [{"source": str(source.relative_to(raw)), "archive": str(source.relative_to(raw)) + ".gz", "bytes": source.stat().st_size}
            for source in candidates]
    if not apply:
        return {"applied": False, "raw_dir": str(raw), "minimum_bytes": minimum, "files": plan}
    assert_no_writers(candidates)
    snapshots = {source: fingerprint(source) for source in candidates}
    for source in candidates:
        relative = str(source.relative_to(raw))
        compressed = source.with_name(source.name + ".gz")
        source_meta = source.stat()
        source_size, source_sha = digest_file(source)
        if compressed.exists():
            # Recover safely from an interrupted previous pass, retaining its
            # archives instead of overwriting an unexplained existing artifact.
            if compressed.is_symlink() or digest_file(compressed, compressed=True) != (source_size, source_sha):
                raise ValueError("existing archive does not match original: " + str(compressed))
        else:
            descriptor, temporary = tempfile.mkstemp(prefix=".archive-", dir=source.parent)
            temporary = Path(temporary)
            try:
                with os.fdopen(descriptor, "wb") as output:
                    with gzip.GzipFile(filename="", mode="wb", compresslevel=9, fileobj=output, mtime=0) as packed:
                        with source.open("rb") as original:
                            shutil.copyfileobj(original, packed, BLOCK)
                    output.flush()
                    os.fsync(output.fileno())
                if digest_file(temporary, compressed=True) != (source_size, source_sha):
                    raise RuntimeError("gzip verification failed: " + str(source))
                if fingerprint(source) != snapshots[source]:
                    raise RuntimeError("log changed while archiving: " + str(source))
                assert_no_writers([source])
                temporary.chmod(stat.S_IMODE(source_meta.st_mode))
                os.utime(temporary, ns=(source_meta.st_atime_ns, source_meta.st_mtime_ns))
                os.replace(temporary, compressed)
                sync_directory(compressed.parent)
            finally:
                temporary.unlink(missing_ok=True)
        compressed_size, compressed_sha = digest_file(compressed)
        record = {"source": relative, "archive": relative + ".gz", "source_bytes": source_size, "source_sha256": source_sha,
                  "archive_bytes": compressed_size, "archive_sha256": compressed_sha, "source_mtime_ns": source_meta.st_mtime_ns,
                  "source_mode": format(stat.S_IMODE(source_meta.st_mode), "04o")}
        if relative in existing:
            records.remove(existing[relative])
        records.append(record)
        existing[relative] = record
    file_map = {relative_file(raw, record["source"]): relative_file(raw, record["archive"]) for record in records}
    report = {"format": 1, "raw_dir": str(raw), "created_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "compression": "gzip level9, mtime0, empty filename; exact decoded SHA256 verified", "minimum_bytes": minimum,
              "files": sorted(records, key=lambda value: value["source"])}
    atomic_bytes(manifest, (json.dumps(report, indent=2, ensure_ascii=False) + "\n").encode())
    links = []
    for document in markdown:
        document = document.absolute()
        if document.is_symlink():
            raise ValueError("Markdown file is a symlink: " + str(document))
        snapshot = fingerprint(document)
        content = document.read_bytes().decode("utf-8")
        updated, count = update_markdown(content, document, file_map)
        if count:
            if fingerprint(document) != snapshot:
                raise RuntimeError("Markdown changed while preparing links: " + str(document))
            atomic_bytes(document, updated.encode(), stat.S_IMODE(document.stat().st_mode))
        links.append({"path": str(document), "links_updated": count})
    # The manifest and archives are durable before any original is removed.
    assert_no_writers(candidates)
    for source in candidates:
        if fingerprint(source) != snapshots[source]:
            raise RuntimeError("log changed before removal; originals retained: " + str(source))
        if digest_file(source.with_name(source.name + ".gz"), compressed=True) != (existing[str(source.relative_to(raw))]["source_bytes"], existing[str(source.relative_to(raw))]["source_sha256"]):
            raise RuntimeError("archive changed before removal: " + str(source))
    for source in candidates:
        source.unlink()
    for parent in {source.parent for source in candidates}:
        sync_directory(parent)
    return {"applied": True, "manifest": str(manifest), "files_archived": len(candidates), "source_bytes": sum(existing[str(source.relative_to(raw))]["source_bytes"] for source in candidates),
            "archive_bytes": sum(existing[str(source.relative_to(raw))]["archive_bytes"] for source in candidates), "markdown": links}


def self_test():
    with tempfile.TemporaryDirectory(prefix="voidfs-archive-self-test-", dir="/private/tmp") as directory:
        root = Path(directory)
        raw = root / "raw"
        raw.mkdir()
        payload = (b'{"kind":"check","ok":false,"name":"original failure"}\n' * 2000) + bytes(range(256))
        original = raw / "failed row.jsonl"
        original.write_bytes(payload)
        (raw / "small.jsonl").write_text('{"ok":false}\n')
        document = root / "README.md"
        document.write_bytes(('[failed](<raw/failed row.jsonl>)\n[label `code`](<raw/failed row.jsonl>)\n[reference]: <raw/failed row.jsonl> "failure"\n[small](raw/small.jsonl)\n[web](https://example.com/raw/failed%20row.jsonl)\n`[literal](<raw/failed row.jsonl>)`\n``multiline\n[multiline](<raw/failed row.jsonl>)\ncode``\n```text\n[fenced](<raw/failed row.jsonl>)\n```\n').replace('\n', '\r\n').encode())
        manifest = raw / "archive-manifest.json"
        plan = archive(raw, manifest, [document], 65536, False)
        assert len(plan["files"]) == 1 and original.read_bytes() == payload and not manifest.exists()
        result = archive(raw, manifest, [document], 65536, True)
        packed = raw / "failed row.jsonl.gz"
        assert result["files_archived"] == 1 and not original.exists()
        assert gzip.decompress(packed.read_bytes()) == payload
        saved = json.loads(manifest.read_text())["files"][0]
        assert saved["source_sha256"] == hashlib.sha256(payload).hexdigest()
        assert saved["archive_sha256"] == hashlib.sha256(packed.read_bytes()).hexdigest()
        content = document.read_bytes().decode()
        assert '[failed](<raw/failed row.jsonl.gz>)' in content and '[reference]: <raw/failed row.jsonl.gz>' in content
        assert '[label `code`](<raw/failed row.jsonl.gz>)' in content and '[multiline](<raw/failed row.jsonl>)' in content
        assert '[small](raw/small.jsonl)' in content and '`[literal](<raw/failed row.jsonl>)`' in content
        assert '[fenced](<raw/failed row.jsonl>)' in content and 'https://example.com/raw/failed%20row.jsonl' in content
        assert content.count('\r\n') == content.count('\n'), 'existing Markdown newline bytes remain unchanged'
        assert archive(raw, manifest, [document], 65536, True)["files_archived"] == 0
        # An actual open writer is rejected before archives or links are changed.
        active = raw / "active.txt"
        with active.open("wb") as writer:
            writer.write(b"active\n" * 12000)
            writer.flush()
            try:
                archive(raw, manifest, [document], 65536, True)
            except RuntimeError as error:
                assert "active log writers" in str(error)
            else:
                raise AssertionError("an open writer was not rejected")
            assert active.exists() and not active.with_suffix(".txt.gz").exists()
        print(json.dumps({"self_test": "pass", "checks": ["dry-run leaves files untouched", "gzip byte equality and both SHA256 values", "only mapped Markdown links updated", "small/web/code targets unchanged", "repeat is safe", "actual active writer rejected"]}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--raw-dir", type=Path)
    parser.add_argument("--manifest", type=Path, help="default: RAW_DIR/archive-manifest.json")
    parser.add_argument("--markdown", type=Path, action="append", default=[], help="update mapped links only in this explicit Markdown file; may repeat")
    parser.add_argument("--minimum-bytes", type=int, default=65536)
    parser.add_argument("--apply", action="store_true", help="archive only after all evidence writers have stopped")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.raw_dir is None or args.minimum_bytes < 1:
        parser.error("--raw-dir and a positive --minimum-bytes are required")
    manifest = args.manifest or args.raw_dir / "archive-manifest.json"
    print(json.dumps(archive(args.raw_dir, manifest, args.markdown, args.minimum_bytes, args.apply), indent=2, ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)

#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Check that spec/conformance/cases.json follows the format in README.md.

Checks structure, types, unique ids, variable use before capture, and path syntax.
It does not run the cases. Exits non-zero and lists every problem found.
Needs only the Python 3 standard library.
"""

import json
import re
import sys
from pathlib import Path

CASES = Path(__file__).with_name("cases.json")

METHODS = {"GET", "HEAD", "PUT", "POST", "DELETE"}
BUILTIN_VARS = {"drive", "drive2", "drive3"}
KNOWN_KEYS = {"admin", "read"}
# Pool features (format §3.1) a case may need the server's drives to have.
KNOWN_FEATURES = {"inline-data", "multi-object-versions"}
BODY_FORMS = {"text", "base64", "bytes", "patch", "json"}
MATCHER_MEMBERS = {"equals", "matches", "present", "contains", "not_equals", "count", "capture"}
BODY_MATCHERS = {"text", "base64", "size", "s3_error", "json", "xml"}
ID_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
VAR_RE = re.compile(r"(?<!\$)\$\{([A-Za-z_][A-Za-z0-9_]*)\}")
JSON_PATH_RE = re.compile(r"^\$((\.[A-Za-z_][A-Za-z0-9_]*)|(\[-?\d+\])|(\['[^']*'\]))*$")
XML_PATH_RE = re.compile(r"^[A-Za-z_][\w.-]*(/[A-Za-z_][\w.-]*)*$")
SPEC_REF_RE = re.compile(r"^(protocol|format) §\d+(\.\d+)*$")


class Checker:
    def __init__(self):
        self.errors = []

    def err(self, where, msg):
        self.errors.append(f"{where}: {msg}")

    def vars_in(self, value):
        if isinstance(value, str):
            return set(VAR_RE.findall(value))
        if isinstance(value, list):
            return set().union(*(self.vars_in(v) for v in value)) if value else set()
        if isinstance(value, dict):
            return set().union(*(self.vars_in(v) for v in value.values())) if value else set()
        return set()

    def use(self, where, value, defined):
        for name in sorted(self.vars_in(value) - defined):
            self.err(where, f"variable ${{{name}}} is used before it is captured")

    def check_str_map(self, where, value):
        if not isinstance(value, dict) or not all(
            isinstance(k, str) and isinstance(v, str) for k, v in value.items()
        ):
            self.err(where, "must be an object of string to string")
            return False
        return True

    def check_body(self, where, body, defined):
        if not isinstance(body, dict) or len(body) != 1 or next(iter(body)) not in BODY_FORMS:
            self.err(where, f"must be an object with exactly one of {sorted(BODY_FORMS)}")
            return
        form, value = next(iter(body.items()))
        if form in ("text", "base64") and not isinstance(value, str):
            self.err(where, f"{form} must be a string")
        if form == "text":
            self.use(where, value, defined)
        if form == "bytes":
            if not (isinstance(value, dict) and set(value) == {"seed", "size"}
                    and all(isinstance(value[k], int) and value[k] >= 0 for k in value)):
                self.err(where, "bytes must be {seed, size} with non-negative integers")
        if form == "patch":
            if not isinstance(value, list) or not 1 <= len(value) <= 10_000:
                self.err(where, "patch must be a list of 1 to 10,000 edits")
            else:
                for i, edit in enumerate(value):
                    if not (isinstance(edit, list) and len(edit) == 2 and isinstance(edit[0], int)
                            and edit[0] >= 0 and isinstance(edit[1], str)):
                        self.err(f"{where}[{i}]", "an edit must be [offset, text]")

    def check_matcher(self, where, matcher, defined, captures, allow_path=None):
        if not isinstance(matcher, dict) or not matcher:
            self.err(where, "matcher must be a non-empty object")
            return
        members = set(matcher) - ({"path"} if allow_path else set())
        unknown = members - MATCHER_MEMBERS
        if unknown:
            self.err(where, f"unknown matcher members {sorted(unknown)}")
        if not members:
            self.err(where, "matcher has no condition")
        if allow_path:
            path = matcher.get("path")
            pattern = JSON_PATH_RE if allow_path == "json" else XML_PATH_RE
            if not isinstance(path, str) or not pattern.match(path):
                self.err(where, f"invalid {allow_path} path {path!r}")
        for member in ("equals", "contains", "not_equals"):
            if member in matcher:
                self.use(where, matcher[member], defined)
        if "matches" in matcher:
            try:
                re.compile(matcher["matches"])
            except (re.error, TypeError) as e:
                self.err(where, f"bad regular expression: {e}")
        if "present" in matcher and not isinstance(matcher["present"], bool):
            self.err(where, "present must be true or false")
        if "count" in matcher and not (isinstance(matcher["count"], int) and matcher["count"] >= 0):
            self.err(where, "count must be a non-negative integer")
        if "capture" in matcher:
            name = matcher["capture"]
            if not isinstance(name, str) or not re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name):
                self.err(where, f"invalid capture name {name!r}")
            elif name in BUILTIN_VARS:
                self.err(where, f"capture would overwrite built-in variable {name}")
            else:
                captures.add(name)

    def check_step(self, where, step, defined, allowed_keys):
        captures = set()
        if not isinstance(step, dict):
            self.err(where, "step must be an object")
            return captures
        unknown = set(step) - {"key", "request", "expect", "name"}
        if unknown:
            self.err(where, f"unknown members {sorted(unknown)}")
        key = step.get("key", "admin")
        if key not in allowed_keys:
            self.err(where, f"key {key!r} is not admin and not listed in requires as key:{key}")

        req = step.get("request")
        if not isinstance(req, dict):
            self.err(where, "request is required")
        else:
            unknown = set(req) - {"method", "path", "query", "headers", "unsigned_headers", "body"}
            if unknown:
                self.err(f"{where}.request", f"unknown members {sorted(unknown)}")
            if req.get("method") not in METHODS:
                self.err(f"{where}.request.method", f"must be one of {sorted(METHODS)}")
            path = req.get("path")
            if not isinstance(path, str) or not path.startswith("/"):
                self.err(f"{where}.request.path", "must be a string starting with /")
            elif re.search(r"[ \u0080-\U0010ffff]", path):
                self.err(f"{where}.request.path", "must be percent-encoded (no spaces or non-ASCII)")
            else:
                self.use(f"{where}.request.path", path, defined)
            for member in ("query", "headers"):
                if member in req and self.check_str_map(f"{where}.request.{member}", req[member]):
                    self.use(f"{where}.request.{member}", req[member], defined)
            if "unsigned_headers" in req:
                names = req["unsigned_headers"]
                header_names = {h.lower() for h in req.get("headers", {})}
                if not isinstance(names, list) or not all(
                    isinstance(n, str) and n.lower() in header_names for n in names
                ):
                    self.err(f"{where}.request.unsigned_headers", "must list names present in headers")
            if "body" in req:
                self.check_body(f"{where}.request.body", req["body"], defined)
            if req.get("method") in ("GET", "HEAD") and "body" in req:
                self.err(f"{where}.request", f"{req['method']} must not have a body")

        exp = step.get("expect")
        if not isinstance(exp, dict):
            self.err(where, "expect is required")
            return captures
        unknown = set(exp) - {"status", "headers", "body"}
        if unknown:
            self.err(f"{where}.expect", f"unknown members {sorted(unknown)}")
        status = exp.get("status")
        statuses = status if isinstance(status, list) else [status]
        if not statuses or not all(isinstance(s, int) and 100 <= s <= 599 for s in statuses):
            self.err(f"{where}.expect.status", "must be an HTTP status or a list of them")
        for name, matcher in (exp.get("headers") or {}).items():
            self.check_matcher(f"{where}.expect.headers.{name}", matcher, defined, captures)
        body = exp.get("body")
        if body is not None:
            if not isinstance(body, dict) or not body or set(body) - BODY_MATCHERS:
                self.err(f"{where}.expect.body", f"must use only {sorted(BODY_MATCHERS)}")
            else:
                if "text" in body:
                    self.use(f"{where}.expect.body.text", body["text"], defined)
                if "size" in body and not (isinstance(body["size"], int) and body["size"] >= 0):
                    self.err(f"{where}.expect.body.size", "must be a non-negative integer")
                for kind in ("json", "xml"):
                    for i, m in enumerate(body.get(kind, [])):
                        self.check_matcher(f"{where}.expect.body.{kind}[{i}]", m, defined, captures, kind)
                if req and req.get("method") == "HEAD":
                    self.err(f"{where}.expect.body", "HEAD responses have no body")
        return captures

    def check(self, doc):
        if not isinstance(doc, dict) or doc.get("format") != 1 or doc.get("protocol") != 1:
            self.err("file", "must be an object with format 1 and protocol 1")
            return
        cases = doc.get("cases")
        if not isinstance(cases, list) or not cases:
            self.err("file", "cases must be a non-empty list")
            return
        seen = set()
        for n, case in enumerate(cases):
            cid = case.get("id") if isinstance(case, dict) else None
            where = f"case {cid or n}"
            if not isinstance(case, dict):
                self.err(where, "must be an object")
                continue
            unknown = set(case) - {"id", "title", "spec", "requires", "steps"}
            if unknown:
                self.err(where, f"unknown members {sorted(unknown)}")
            if not isinstance(cid, str) or not ID_RE.match(cid):
                self.err(where, "id must be lowercase words joined by hyphens")
            elif cid in seen:
                self.err(where, "duplicate id")
            seen.add(cid)
            if not isinstance(case.get("title"), str) or not case["title"].strip():
                self.err(where, "title is required")
            spec = case.get("spec")
            if not isinstance(spec, list) or not spec or not all(
                isinstance(s, str) and SPEC_REF_RE.match(s) for s in spec
            ):
                self.err(where, 'spec must list references like "protocol §4.1"')
            requires = case.get("requires", [])
            allowed_keys = {"admin"}
            for r in requires if isinstance(requires, list) else [None]:
                if isinstance(r, str) and r.startswith("key:") and r[4:] in KNOWN_KEYS:
                    allowed_keys.add(r[4:])
                elif isinstance(r, str) and r.startswith("feature:") and r[8:] in KNOWN_FEATURES:
                    pass
                else:
                    self.err(where, f"unknown requirement {r!r}")
            steps = case.get("steps")
            if not isinstance(steps, list) or not steps:
                self.err(where, "steps must be a non-empty list")
                continue
            defined = set(BUILTIN_VARS)
            for i, step in enumerate(steps):
                defined |= self.check_step(f"{where} step {i}", step, defined, allowed_keys)


def main():
    try:
        doc = json.loads(CASES.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        print(f"{CASES}: {e}", file=sys.stderr)
        return 1
    checker = Checker()
    checker.check(doc)
    if checker.errors:
        for line in checker.errors:
            print(line, file=sys.stderr)
        print(f"{len(checker.errors)} problem(s) in {CASES.name}", file=sys.stderr)
        return 1
    steps = sum(len(c["steps"]) for c in doc["cases"])
    print(f"{CASES.name}: {len(doc['cases'])} cases, {steps} steps, OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())

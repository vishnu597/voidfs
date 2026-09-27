# Contributing to voidfs

Thanks for helping. This page covers the two rules that apply to every change, and the extra
process for changes to the specs.

## 1. Sign off your commits (DCO)

voidfs uses the [Developer Certificate of Origin 1.1](DCO) instead of a contributor license
agreement. By signing off a commit you certify the statements in that file: in short, that you
have the right to submit the work under the project's license.

Add the sign-off with `-s`:

```bash
git commit -s -m "gateway: reject patch bodies with trailing bytes"
```

That appends a line with your name and email, which must match the commit author:

```text
Signed-off-by: Jane Doe <jane@example.com>
```

To sign off commits you already made on a branch:

```bash
git rebase --signoff main
```

Pull requests with unsigned commits cannot be merged. No real name is required; a consistent
identity you can be contacted at is.

## 2. License of contributions

All contributions are made under the [Apache License 2.0](LICENSE), the same license as the
project. New source files carry this header:

```text
// SPDX-License-Identifier: Apache-2.0
```

## 3. Changes to the specs

The files in [`spec/`](spec/) are the contract between voidfs servers, clients and the data in
users' buckets. They change more carefully than code.

| Change | Process |
|---|---|
| Typo, wording, an example, a clarification that changes no behavior | A normal pull request |
| Anything a client, server or stored drive could observe: a new operation, header, error, field, limit or on-bucket object | An [RFC](rfcs/) first, then a pull request that implements it |

Every behavior change to the protocol or the format must come with:

1. the spec text, using the [conventions](spec/README.md#conventions);
2. at least one [conformance case](spec/conformance/) that exercises it;
3. an entry in [`spec/CHANGELOG.md`](spec/CHANGELOG.md).

The conformance cases must stay valid:

```bash
python3 spec/conformance/validate.py
```

## 4. Reporting security issues

Do not open a public issue. See [SECURITY.md](SECURITY.md).

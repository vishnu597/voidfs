# voidfs specifications

| Spec | Defines | Status |
|---|---|---|
| [protocol.md](protocol.md) | The wire protocol: the S3 subset voidfs serves and the `x-voidfs-*` extensions | Draft 1 |
| [format.md](format.md) | The on-bucket format: how drives, versions and content are laid out in a bucket | Draft 1 |
| [conformance/](conformance/) | Language-neutral test cases that any voidfs server must pass | Draft 1 |
| [CHANGELOG.md](CHANGELOG.md) | Every change to the above | |

**Draft** means the text may still change incompatibly. A spec becomes **Stable** when the
Phase 1 server passes the whole conformance suite on Amazon S3, Cloudflare R2 and MinIO. After
that, changes follow the compatibility rules in each spec's versioning section.

## Two contracts

- The **protocol** is between programs and a voidfs server. It changes when the API changes.
- The **format** is between a voidfs server (or any reader) and the bytes in a user's bucket. It
  is the stricter of the two: data written today must be readable by every later reader, and
  anyone holding the bucket and this spec must be able to read a drive without voidfs.

## Conventions

- The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT, RECOMMENDED,
  NOT RECOMMENDED, MAY and OPTIONAL are to be interpreted as described in
  [BCP 14](https://www.rfc-editor.org/info/bcp14) ([RFC 2119](https://www.rfc-editor.org/rfc/rfc2119),
  [RFC 8174](https://www.rfc-editor.org/rfc/rfc8174)) when, and only when, they appear in all
  capitals.
- Sections marked *(informative)* explain; they do not add requirements.
- Byte sizes use binary units: 1 KiB = 1024 bytes, 1 MiB = 1024 KiB.
- Timestamps are [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) in UTC, with the `Z`
  suffix and up to microsecond precision.
- Integers in binary encodings are unsigned and big-endian unless stated otherwise.
- `sha256(x)` is the lowercase hexadecimal SHA-256 of the bytes `x` (64 characters).

## Changing a spec

Behavior changes need an [RFC](../rfcs/), a conformance case and a changelog entry. See
[CONTRIBUTING.md](../CONTRIBUTING.md#3-changes-to-the-specs).

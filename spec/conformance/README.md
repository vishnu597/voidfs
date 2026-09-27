# voidfs conformance suite

[`cases.json`](cases.json) holds language-neutral test cases for the [wire protocol](../protocol.md).
Each case is a sequence of HTTP requests and the responses a conforming server must give.

- Every voidfs server runs the suite in CI against each supported backend.
- Every SDK runs it, through its own client, against a live server.
- A third-party server may claim compatibility only if it passes (see
  [TRADEMARKS.md](../../TRADEMARKS.md)).

Check that the file is well formed (it needs only Python 3):

```bash
python3 spec/conformance/validate.py
```

The validator checks structure and variable use. It does not run the cases; the runner arrives
in Phase 1.

---

## File format

```json
{ "format": 1, "protocol": 1, "cases": [ … ] }
```

### Case

| Member | Required | Meaning |
|---|---|---|
| `id` | yes | Unique, lowercase, hyphen-separated |
| `title` | yes | One line describing what is tested |
| `spec` | yes | The sections exercised, for example `["protocol §4.1"]` |
| `requires` | no | Capabilities the runner must have, for example `["key:read"]`. A runner without them reports the case as skipped, never as passed |
| `steps` | yes | Requests, run in order. The case fails at the first step whose expectations are not met |

### Runner environment

- The runner signs every request with AWS Signature Version 4 using the key the step names.
- The key named `admin` is an admin-scoped key for every drive, and every runner has it.
- Other keys are optional and listed in `requires` as `key:<name>`:

  | Key | Scope |
  |---|---|
  | `read` | read, every drive |

- Before a case, the runner picks fresh, unused drive names for the variables `drive`, `drive2`
  and `drive3` (valid S3 bucket names such as `vfc-3k9x2m1q`). It does not create them. After
  the case, it hard-deletes every drive the case created, whether the case passed or not.

### Step

```json
{ "key": "admin",
  "request": { "method": "PUT", "path": "/${drive}/a.txt", "query": {}, "headers": {}, "body": { "text": "hi" } },
  "expect":  { "status": 200, "headers": {}, "body": {} } }
```

**`request`**

| Member | Meaning |
|---|---|
| `method` | `GET`, `HEAD`, `PUT`, `POST` or `DELETE` |
| `path` | Sent exactly as written after variable substitution. It MUST already be percent-encoded. Path-style addressing |
| `query` | Name to value. The runner percent-encodes names and values. A value of `""` sends a bare `name` |
| `headers` | Name to value, sent as written |
| `unsigned_headers` | Names of headers in `headers` to send but leave out of `SignedHeaders` (for testing that servers reject them) |
| `body` | One of the body forms below. No body means an empty body |

**Body forms**

| Form | Bytes sent |
|---|---|
| `{ "text": "…" }` | The UTF-8 string |
| `{ "base64": "…" }` | The decoded bytes |
| `{ "bytes": { "seed": s, "size": n } }` | `n` deterministic bytes: the concatenation of `SHA-256("voidfs-conformance" ‖ u64be(s) ‖ u64be(i))` for `i` = 0, 1, 2, …, truncated to `n` |
| `{ "patch": [[offset, "text"], …] }` | A `VFSP` version 1 patch body ([protocol §4.2](../protocol.md#42-batched-edits-post-drivekeyx-voidfs-patch)) with those edits, in order |
| `{ "json": … }` | The value serialized as JSON. The runner also sends `Content-Type: application/json` unless `headers` sets one |

**`expect`**

| Member | Meaning |
|---|---|
| `status` | An integer, or a list of acceptable integers |
| `headers` | Header name (case-insensitive) to matcher |
| `body` | Body matchers, below. All that are given must hold |

**Matchers** for a header, or for a value found by a path. Every member given must hold.

| Member | Holds when |
|---|---|
| `equals` | The value equals this (after variable substitution). For an XML path, this is a list compared with every match in order |
| `matches` | The value matches this regular expression (ECMAScript syntax, anchored at both ends) |
| `present` | `true`: the value exists. `false`: it does not |
| `contains` | For a list (a JSON array, or every match of an XML path): some element equals this |
| `not_equals` | The value differs from this |
| `count` | The number of XML matches, or the length of a JSON array |
| `capture` | Always holds. Stores the value (the first match, for XML) in the named variable for later steps |

**Body matchers**

| Member | Holds when |
|---|---|
| `text` | The body, as UTF-8, equals this |
| `base64` | The body equals these decoded bytes |
| `size` | The body is this many bytes long |
| `s3_error` | The body is an S3 XML error whose `Code` equals this |
| `json` | A list of `{ "path": …, …matcher }`. The body is JSON, and each matcher holds for the value at its path |
| `xml` | A list of `{ "path": …, …matcher }`. The body is XML, and each matcher holds for the elements at its path |

### Paths

- **JSON:** `$` followed by any of `.name`, `[index]` (from 0; negative counts from the end) and
  `['any name']`. `.length` on an array or string gives its length. A path that leads nowhere
  has no value, so `present: false` holds and every other matcher fails.
- **XML:** element names from the document element down, separated by `/`, ignoring namespaces.
  For example `ListBucketResult/Contents/Key` matches every such element, in document order.
  The value of an element is its text content.

### Variables

`${name}` in a path, query value, header value, `text` body, `equals`, `contains` or
`not_equals` is replaced by the variable's value. A variable must be one of `drive`, `drive2`
and `drive3`, or captured by an earlier step of the same case. `$${` produces a literal `${`.

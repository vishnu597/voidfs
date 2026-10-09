# voidfs RFCs

An RFC is a short proposal for a change that a client, a server or a stored drive could observe:
a new operation, header, error, field, limit or on-bucket object. Editorial fixes to the specs
do not need one. See [CONTRIBUTING.md](../CONTRIBUTING.md#3-changes-to-the-specs).

## Process

1. Copy [`0000-template.md`](0000-template.md) to `NNNN-short-title.md`, using the next free
   number, and open a pull request. Status: **Proposed**.
2. Discussion happens on the pull request. Revise the RFC until the open questions are
   answered.
3. A maintainer marks it **Accepted** (merged) or **Rejected** (merged, so the reasoning is
   kept), or the author withdraws it.
4. An Accepted RFC is implemented by one or more later pull requests. Each changes the spec,
   adds conformance cases and updates [`spec/CHANGELOG.md`](../spec/CHANGELOG.md). The RFC is
   then marked **Implemented** with links to them.

RFCs are never deleted. A later RFC that replaces one says so, and the old one is marked
**Superseded**.

## Index

| RFC | Title | Status |
|---|---|---|
| [0001](0001-metadata-in-the-bucket.md) | Drive metadata as a commit log and checkpoints in the bucket | Accepted |
| [0002](0002-gc-safe-against-writers.md) | Garbage collection that is safe against writers | Implemented |
| [0003](0003-small-content-in-descriptors.md) | Small content inside the content descriptor | Implemented |
| [0004](0004-a-version-for-every-object-it-changes.md) | A version for every object a transaction changes | Implemented |
| [0005](0005-user-metadata-on-edits.md) | User metadata on edits | Implemented |

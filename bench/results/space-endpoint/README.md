# Space's hosted endpoint, from this Mac

*3 October 2026, the last day of the SpaceFS trial. `voidfs-bench`'s 23 small-object scenarios
(those of 1 MiB and less), at a quarter of the operations, against Space's S3 endpoint
(`s3sdk.spacefs.com`, protocol 1, Space 0.2.343) as the harness's "bare" target: the trial drive as
the bucket, the scenarios' objects under `void-probe/bench/`, deleted afterwards. The harness ran on
this Mac over home internet; Space's edge answered from Montreal (`x-s3sdk-edge-colo: YUL`).*

What this measures is Space's service as a plain S3 client sees it. The harness's bare target uses
standard S3 calls only, so the edit rows read the object, change it and put it back, and the move
row copies and deletes 200 objects: Space's own extensions (`x-s3sdk-write`, `x-s3sdk-rename`)
would be faster, and are timed by hand in [step 4 §1.7](../../../docs/step-4-client.md#17-keys-and-drives-observed-3-october).
It is not SpaceFS's setup (a GCP VM in us-east4 and S3 in us-east-1), so it says nothing about their
published ratios.

Two runs, because the first stopped at its 90-minute limit after hanging 12 minutes in
"overwrite 1 MiB": [space-small-objects](space-small-objects.md) (16 rows) and
[space-small-objects-rest](space-small-objects-rest.md) (the other 7). One row has no figure: the
setup of "fanout get 200 × 256 KiB" failed with an HTTP/2 stream error from the edge. The column
"Bucket alone" in those files is Space's endpoint.

| Scenario | Space's endpoint (ms) | R2 directly, 27 September (ms) |
|---|--:|--:|
| get 4 KiB | 48.1 | 128 |
| head | 47.3 | 76.8 |
| list 200 keys | 106 | 176 |
| get 1 MiB | 181 | 166 |
| fanout get 1000 × 4 KiB, 32 / 64 at once | 73.2 / 88.6 | 114 / 121 |
| fanout get 200 × 256 KiB, 32 at once | – (setup failed) | 204 |
| put 4 KiB / overwrite 4 KiB | 115 / 103 | 218 / 213 |
| put 1 MiB / overwrite 1 MiB | 343 / 398 | 292 / 315 |
| fanout put 1000 × 4 KiB, 32 / 64 at once | 239 / 401 | 207 / 211 |
| fanout put 200 × 256 KiB, 32 at once | 290 | 319 |
| edits in 1 MiB, read-modify-write (8 rows) | 666–1,272 | 540–674 |
| move dir 200 × 64 KiB, copy and delete | 13,043 | 8,616 |

The R2 column is the bare bucket of [r2-small-objects](../r2-small-objects.md), another day and a
bucket far from this Mac (its `head` took 77 ms), for orientation only. Space's edge is close, so its
small reads and small puts are quicker than reaching that bucket directly; its larger writes,
fan-out puts, edits and copies are slower.

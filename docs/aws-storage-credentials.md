# AWS storage credentials: diagnosing S3 403s

The live check now passes after correcting the assumed role's bucket ARNs. The
[successful rerun](../bench/results/storage-credentials/aws-check-after-role-fix.txt) began at
`2026-10-05T00:38:01Z` (4 October in Toronto), build `a108f12`. Startup and `probe` read
`voidfs.json` and list the scoped drive (200), while all four forbidden operations return
403 AccessDenied. All four applicable conformance cases pass, the no-credentials case skips,
and the script exits 0. It deleted four test-pool objects and preserved the two pre-existing
objects (two before, two after).

The 4 October 2026 credentials check minted a session, but both requests that must succeed failed:
reading `voidfs.json` and listing that session's drive. The four denied requests alone do not prove
the scope is correct. The server consequently withholds storage credentials, and conformance reports
one pass for that fallback and four skips. This is a failed capability check, not a successful test
of direct reads. Its pool was purged: the before and after counts were both zero.

The approved rerun on 4 October (Toronto), `2026-10-05T00:15:23Z`, confirmed
`HTTP 403 AccessDenied` on all six requests with the new diagnostics. The check script exits
1 because credentials are not offered and the four scoped-read cases skip. It deleted the
four objects in its own pool and preserved the two pre-existing `voidfs-bench/` objects
(two before, two after). The [saved result](../bench/results/storage-credentials/aws-check.txt)
contains no credential values. AWS CLI is not installed on the test machine, so the role's
policies were reviewed from the JSON the user supplied. The user's S3 policy and `.env.aws`
named the intended bucket, but the read role's object and bucket resource ARNs named a different
bucket. The role ARN itself matched. The user corrected both resource ARNs in the role's
permissions policy; the successful rerun above confirms the resulting scoped access.

The log distinguishes two stages. `none could be minted: STS AssumeRole answered ...` means STS
refused the mint. Six S3 request statuses mean the mint succeeded. In the reported run, the role's
trust and caller's ability to assume it worked for that session; investigate the resulting role's
S3 access first.

The server now includes a safe S3 error code with each failed request, for example `HTTP 403
AccessDenied`, `InvalidToken`, or `SignatureDoesNotMatch`. It leaves out error messages and other XML
fields because they can expose request details. This diagnostic change does not relax the startup
check or expand the minted credentials' permissions.

## Check the assumed role's permissions

Open the role named by `VOIDFS_STORAGE_CREDENTIALS_ROLE` in IAM. Its **Permissions** tab needs a
policy for the bucket actually named by `.env.aws`, separate from the role's **Trust relationships**.
Having read/write access on the IAM user that starts voidfs does not give that access to the role.
AWS evaluates the intersection of the role's permissions and voidfs's session policy; the caller's
original S3 grants do not carry into the session.
[AWS documents the evaluation](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_temp_control-access_assumerole.html).

For the benchmark, replace `<bucket>` below with that bucket's name and attach the policy to the
assumed role. This grants the role read access under `voidfs-bench/`; voidfs's session policy narrows
each mint to `voidfs.json`, shared `shards/` and `pages/`, and a single drive's prefix. It grants
listing only under that drive's prefix and grants no writes.

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "s3:GetObject",
      "Resource": "arn:aws:s3:::<bucket>/voidfs-bench/*"
    },
    {
      "Effect": "Allow",
      "Action": "s3:ListBucket",
      "Resource": "arn:aws:s3:::<bucket>",
      "Condition": {
        "StringLike": { "s3:prefix": "voidfs-bench/*" }
      }
    }
  ]
}
```

Check the bucket name in both resources. `GetObject` uses an object ARN with the prefix, whereas
`ListBucket` uses the bucket ARN. The `s3:prefix` condition is an object key prefix: it contains no
bucket name, ARN, `s3://`, or leading slash. Use `StringLike` with `*`, so that the fresh pool and
its synthetic startup-check drive are included. Keep this condition on the listing statement;
object GETs do not supply `s3:prefix`, so attaching the condition to `GetObject` can deny those too.

If the policy matches and `AccessDenied` persists, inspect explicit denies in the bucket policy,
the role's permissions boundary, organization policies, and any VPC endpoint policy. Bucket
conditions that allow only the original IAM user can reject the assumed role. For a role in another
account, check the bucket-side grant too.
[AWS's 403 guide covers these policies](https://docs.aws.amazon.com/AmazonS3/latest/userguide/troubleshoot-403-errors.html).

SSE-KMS can cause an object read denial even when S3 grants match; KMS does not explain a denied
`ListBucket` request. The current minted session grants S3 actions only, so an SSE-KMS deployment
also needs a review of the session policy and key policy before claiming direct-read support.

`InvalidToken`, `ExpiredToken`, or `SignatureDoesNotMatch` instead points to temporary credentials
or signing. Verify the configured S3 region and endpoint, and that the machine's clock is current.
Local tests independently verify all six startup requests' SigV4 signatures, body hashes, session
token, and configured region; they do not establish the policies in the user's account.

## Retry after the IAM review

Run from the checkout, with an env file that contains the bucket settings and role ARN. These
commands do not print env values or a credentials response:

```bash
cargo build --release -p voidfs-server -p voidfs-conformance -p voidfs-bench
bench/scripts/credentials-check.sh .env.aws | tee bench/results/storage-credentials/aws-check.txt
```

This contacts AWS: expect about 150 requests, under 1 MB, and around 30 seconds on a working setup.
For ordinary S3 Standard in us-east-1, our rough estimate is below US$0.01 for requests and transfer,
using AWS's published [GET/PUT rates](https://docs.aws.amazon.com/solutions/latest/data-transfer-hub/cost.html)
and [LIST rates](https://aws.amazon.com/blogs/storage/run-spark-31-faster-and-optimize-compute-costs-with-amazon-s3-express-one-zone-on-amazon-emr/).
Actual charges depend on region and bucket options. The script counts `voidfs-bench/` before and
after and purges only the fresh pool it created. Consult the current
[S3 pricing](https://aws.amazon.com/s3/pricing/) for the account's configuration.

A successful result reads `HTTP 200` for the descriptor and drive listing, denies the other four
requests, and ends `Storage credentials are offered, for 15 minutes`. Conformance then reports
four passes and one skip for the case that tests servers without storage credentials. Record an
AWS pass only after that run; the original 403 report is evidence of the fallback working.

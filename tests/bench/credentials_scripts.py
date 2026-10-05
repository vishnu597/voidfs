#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Check cloud benchmark scripts with fake binaries, without contacting a bucket."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CASES = [
    "storage-credentials-describe-the-drives-storage",
    "storage-credentials-read-the-drive",
    "storage-credentials-reach-no-further",
    "storage-credentials-for-read-keys",
]


class CredentialScripts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="voidfs-script-tests-")
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("VOIDFS_", "BENCH_", "MOCK_"))}
        self.env.update(PATH=f"{self.work}:{os.defpath}", MOCK_JOURNAL=str(self.work / "journal"), MOCK_MODE="offered")
        self.binary("curl", '[[ "$MOCK_MODE" != startup-error ]]')
        self.binary("bench", '''
if [[ "$1" == delay ]]; then
    while :; do sleep 1; done
fi
printf '%s\\n' "$*" >> "$MOCK_JOURNAL"
if [[ "$*" == *--yes* ]]; then echo 'deleted them'; else echo '0 objects under the prefix'; fi
''')
        self.binary("server", '''
if [[ "${MOCK_R2_EXPECTED:-0}" == 1 ]]; then
    [[ "${VOIDFS_S3_REGION:-}" == auto && "${VOIDFS_R2_API_TOKEN:-}" == fake-r2-token ]] || exit 9
fi
if [[ "${@: -1}" == probe ]]; then
    printf 'pool fake-pool\\ncreate-if-absent honoured\\n'
    if [[ "$MOCK_MODE" == unavailable || "$MOCK_MODE" == probe-no-offer ]]; then
        echo 'storage credentials: Storage credentials are NOT offered'
    else
        echo 'storage credentials: Storage credentials are offered, for 15 minutes'
    fi
    if [[ "$MOCK_MODE" == probe-error ]]; then exit 7; fi
    exit 0
fi
if [[ "$MOCK_MODE" == startup-error ]]; then echo 'the server could not start'; exit 8; fi
if [[ "$MOCK_MODE" == unavailable || "$MOCK_MODE" == startup-unavailable ]]; then
    echo 'storage credentials: Storage credentials are NOT offered'
else
    echo 'storage credentials: Storage credentials are offered, for 15 minutes'
fi
while :; do sleep 1; done
''')
        case_lines = "\n".join(f"echo \"$verdict  {case}  1 ms\"" for case in CASES)
        self.binary("conformance", f'''
if [[ "$MOCK_MODE" == unavailable || "$MOCK_MODE" == case-skip ]]; then
    echo 'PASS  storage-credentials-not-offered  1 ms'
    verdict=SKIP
else
    echo 'SKIP  storage-credentials-not-offered offered'
    verdict=PASS
fi
{case_lines}
if [[ "$MOCK_MODE" == conformance-error ]]; then echo 'FAIL  credentials  step 1: failed'; exit 6; fi
''')
        self.binary("bucketread", "echo 'mocked bucket read'")
        self.env.update(
            VOIDFS_BENCH_BIN=str(self.work / "bench"),
            VOIDFS_SERVER_BIN=str(self.work / "server"),
            VOIDFS_CONFORMANCE_BIN=str(self.work / "conformance"),
            VOIDFS_BUCKETREAD_BIN=str(self.work / "bucketread"),
        )
        self.aws = self.work / ".env.aws"
        self.aws.write_text("VOIDFS_S3_BUCKET=fake-bucket\nVOIDFS_S3_REGION=us-east-1\nVOIDFS_STORAGE_CREDENTIALS_ROLE=fake-role\n")
        self.r2 = self.work / ".env.r2"
        self.r2.write_text("VOIDFS_S3_BUCKET=fake-bucket\nVOIDFS_S3_ENDPOINT=https://fake-account.r2.cloudflarestorage.com\nVOIDFS_S3_ACCESS_KEY_ID=fake-parent\nVOIDFS_TOKEN_VALUE=fake-r2-token\n")

    def binary(self, name, body):
        path = self.work / name
        path.write_text(f"#!/usr/bin/env bash\nset -eu\n{body}\n")
        path.chmod(0o755)

    def run_script(self, script, *args):
        return subprocess.run(["bash", str(ROOT / "bench/scripts" / script), *map(str, args)], env=self.env, text=True, capture_output=True, timeout=10)

    def assert_purged_own_pool(self):
        lines = (self.work / "journal").read_text().splitlines()
        deletes = [line for line in lines if "--yes" in line]
        self.assertEqual(len(deletes), 1)
        self.assertRegex(deletes[0], r"^purge --prefix voidfs-bench/credentials-check-[0-9TZ]+/ --yes$")

    def test_offered_credentials_pass_and_purge(self):
        result = self.run_script("credentials-check.sh", self.aws)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_purged_own_pool()

    def test_skipped_capability_fails_and_purges(self):
        for mode in ["unavailable", "startup-unavailable", "case-skip"]:
            with self.subTest(mode=mode):
                (self.work / "journal").unlink(missing_ok=True)
                self.env["MOCK_MODE"] = mode
                result = self.run_script("credentials-check.sh", self.aws)
                self.assertEqual(result.returncode, 1, result.stderr)
                if mode != "startup-unavailable":
                    self.assertIn("must pass against this bucket", result.stderr)
                self.assert_purged_own_pool()
        self.env.update(MOCK_MODE="unavailable", BENCH_AWS="1", BENCH_ENV=str(self.aws))
        result = self.run_script("bucket-read.sh")
        self.assertEqual(result.returncode, 1, result.stderr)

    def test_failures_purge_the_pool(self):
        for mode in ["startup-error", "probe-error", "probe-no-offer", "conformance-error"]:
            with self.subTest(mode=mode):
                (self.work / "journal").unlink(missing_ok=True)
                self.env["MOCK_MODE"] = mode
                result = self.run_script("credentials-check.sh", self.aws)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assert_purged_own_pool()

    def test_r2_alias_defaults_region_without_an_aws_role(self):
        self.env["MOCK_R2_EXPECTED"] = "1"
        result = self.run_script("credentials-check.sh", self.r2)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_purged_own_pool()
        for switch in ["BENCH_R2", "BENCH_AWS"]:
            with self.subTest(switch=switch):
                self.env.pop("BENCH_R2", None)
                self.env.pop("BENCH_AWS", None)
                self.env.update({switch: "1", "BENCH_ENV": str(self.r2)})
                result = self.run_script("bucket-read.sh")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("the R2 bucket", result.stdout)

    def test_aws_requires_its_role(self):
        self.aws.write_text("VOIDFS_S3_BUCKET=fake-bucket\n")
        for script in ["credentials-check.sh", "bucket-read.sh"]:
            with self.subTest(script=script):
                self.env.update(BENCH_AWS="1", BENCH_ENV=str(self.aws))
                args = [self.aws] if script == "credentials-check.sh" else []
                result = self.run_script(script, *args)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("VOIDFS_STORAGE_CREDENTIALS_ROLE", result.stderr)


if __name__ == "__main__":
    unittest.main()

#!/bin/bash
# The full self-tests on a development-signed and an ad hoc build of the app's sources, then the breaks.
set -u
SRC=$1; SP=$2
B=$SP/build; R=$SP/results; mkdir -p $B $R
rm -f $B/voidfs-agent-test $B/voidfs-signed
xcrun swiftc -swift-version 5 -target arm64-apple-macos27.0 -O -o $B/voidfs-agent-test $SRC/App/*.swift $SRC/Shared/*.swift $SRC/Badges/*.swift > $R/build.log 2>&1 || { echo "build failed"; exit 1; }
cp $B/voidfs-agent-test $B/voidfs-signed
codesign -f -s "$IDENTITY" -i dev.voidfs.app $B/voidfs-signed 2>/dev/null
{ echo '$ voidfs agent-selftest'; $B/voidfs-signed agent-selftest; echo "exit $?"; } > $R/agent-selftest.txt 2>&1
{ echo '$ voidfs login-item selftest'; $B/voidfs-signed login-item selftest; echo "exit $?"; } > $R/login-item-selftest.txt 2>&1
{ echo '## ad hoc signed, as swiftc links it'; codesign -dvv $B/voidfs-agent-test 2>&1 | grep -E '^Identifier|^Signature'
  echo '$ voidfs agent-selftest'; $B/voidfs-agent-test agent-selftest | grep -E '^FAIL|self-test'; echo "exit ${PIPESTATUS[0]}"
  echo '$ voidfs login-item selftest'; $B/voidfs-agent-test login-item selftest | grep -E '^FAIL|self-test'; echo "exit ${PIPESTATUS[0]}"; } > $R/unsigned.txt 2>&1
/usr/bin/python3 -I $SP/scripts/breaks.py $SRC $SP/breaks > $R/breaks.txt 2>&1
echo "done"

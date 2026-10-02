#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# poll.sh <file> <max seconds>: prints "<unix time> <size>" each time the file's size changes; stops at 1 GiB or the limit.
f=$1; end=$(perl -MTime::HiRes=time -e "printf '%.3f', time+$2"); last=-1
while :; do
    t=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
    perl -e "exit($t > $end ? 0 : 1)" && break
    s=$(stat -f %z "$f" 2>/dev/null || echo -1)
    [ "$s" != "$last" ] && echo "$t $s" && last=$s
    [ "$s" = 1073741824 ] && break
    sleep 0.05
done

#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# frames.sh <outdir> <seconds> <x,y,w,h>: captures the screen region as fast as it can, each file named by its capture time.
out=$1; dur=$2; rect=$3; mkdir -p "$out"
end=$(perl -MTime::HiRes=time -e "printf '%.3f', time+$dur")
while :; do
    t=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
    perl -e "exit($t > $end ? 0 : 1)" && break
    screencapture -x -R "$rect" -t png "$out/$t.png"
done

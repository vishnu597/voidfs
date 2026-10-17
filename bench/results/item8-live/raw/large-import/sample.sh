#!/bin/bash
# sample.sh <pid> <seconds> <label>: CPU time and RSS of a process each second.
pid=$1; n=$2; label=$3
cpu() { ps -o time= -p $pid | awk -F'[:.]' '{ if (NF==4) print ($1*3600+$2*60+$3)+$4/100; else print ($1*60+$2)+$3/100 }'; }
c0=$(cpu); t0=$(/usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time')
for i in $(seq 1 $n); do sleep 1; echo "$label t=$i rss_kib=$(ps -o rss= -p $pid | tr -d ' ') cpu_s=$(cpu)"; done
c1=$(cpu); t1=$(/usr/bin/perl -MTime::HiRes=time -e 'printf "%.3f", time')
echo "$label average CPU: $(echo "scale=1; 100*($c1-$c0)/($t1-$t0)" | bc)% over $(echo "$t1-$t0" | bc) s"

#!/bin/bash

SAMPLE_COUNT=100
SLEEP_TIME=0.1

for x in $(seq 1 $SAMPLE_COUNT)
    do
        riscv64-unknown-elf-gdb \
            -ex "set pagination 0" \
            -ex "thread apply all bt" \
            -ex "detach" \
            --batch
        sleep $SLEEP_TIME
    done | \
awk '
  BEGIN { s = ""; } 
  /^Thread/ { print s; s = ""; } 
  /^\#/ { if (s != "" ) { s = s "," $4} else { s = $4 } } 
  END { print s }' | \
sort | uniq -c | sort -r -n -k 1,1

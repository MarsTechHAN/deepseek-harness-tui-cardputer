#!/bin/sh
printf 'ESC TEST APP\n\nThis app would run forever.\n'
i=0
while :; do
  i=$((i+1))
  printf '\r  tick %s ' "$i"
  sleep 1
done

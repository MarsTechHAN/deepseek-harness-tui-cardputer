#!/bin/sh
printf 'GOOD APP\n'
[ -n "$1" ] && printf 'arg=%s\n' "$1"
read -r line
[ -n "$line" ] && printf 'stdin=%s\n' "$line"
exit 0

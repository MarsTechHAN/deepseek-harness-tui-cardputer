#!/bin/sh
if [ -t 1 ]; then echo 'MODE: tty'; else echo 'MODE: pipe'; fi
if [ -t 0 ]; then echo 'STDIN: tty'; else echo 'STDIN: pipe'; fi
printf 'SIZE: %sx%s\n' "${COLUMNS:-?}" "${LINES:-?}"
read -r line 2>/dev/null || true
[ -n "$line" ] && echo "LINE: $line"
exit 0

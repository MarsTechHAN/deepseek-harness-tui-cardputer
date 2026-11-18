#!/bin/sh
printf 'ARGS: %s|%s\n' "$1" "$2"
printf 'STATE: %s\n' "$DST_APP_STATE"
printf 'SECRET: %s\n' "${DEEPSEEK_API_KEY:-unset}"
printf 'HOME: %s\n' "${HOME:-}"
sleep 1

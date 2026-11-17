#!/bin/sh
printf '\033[36mCHATTY LINE 1\033[0m\n'
sleep 0.2
printf '\033[33mCHATTY LINE 2\033[0m\n'
printf '\033[2K\rprogress'
sleep 0.2
printf '\033[2K\rdone\n'
sleep 1.5

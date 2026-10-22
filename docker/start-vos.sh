#!/bin/sh
set -eu
photocraft-storage &
storage_pid=$!
nginx -g 'daemon off;' &
nginx_pid=$!
trap 'kill "$storage_pid" "$nginx_pid" 2>/dev/null || true' EXIT INT TERM
while kill -0 "$storage_pid" 2>/dev/null && kill -0 "$nginx_pid" 2>/dev/null; do sleep 1; done
exit 1

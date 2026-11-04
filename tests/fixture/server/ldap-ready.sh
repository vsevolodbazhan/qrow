#!/bin/sh
set -eu

# A PID file can exist before slapd accepts the schema import's local bind.
attempt=0
while [ "$attempt" -lt 100 ]; do
    if ldapwhoami -Y EXTERNAL -Q -H ldapi:/// -o nettimeout=1; then
        exit 0
    fi
    attempt=$((attempt + 1))
    sleep 0.1
done

echo 'OpenLDAP did not accept a local EXTERNAL bind before schema import.' >&2
exit 1

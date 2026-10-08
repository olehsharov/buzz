#!/usr/bin/env bash
# Trust the fork's self-signed code-signing leaf on an ephemeral macOS runner.
#
# Usage: fork-release-trust-signing-cert.sh <keychain> <cert.pem> <identity>
#
# codesign refuses an untrusted self-signed identity ("<name>: no identity
# found"), so the leaf needs an admin-domain codeSign trust setting. macOS
# blocks non-interactive trust changes, and on some hosted images the
# trust-settings commands hang instead of failing (actions/runner-images#11893,
# #12116). Every privileged call is therefore bounded, and any failure prints
# diagnostics before exiting non-zero. Troubleshooting: docs/fork-release.md.
set -euo pipefail

[[ $# -eq 3 ]] || { echo "usage: $0 <keychain> <cert.pem> <identity>" >&2; exit 2; }
kc=$1
pem=$2
identity=$3
limit=${BUZZ_TRUST_TIMEOUT_SECONDS:-120}
right=com.apple.trust-settings.admin

# Kill the command after $limit seconds (exit 142, SIGALRM). perl runs as root
# so the alarm hits the security process itself, not sudo.
bounded_sudo() { sudo perl -e 'alarm shift; exec @ARGV or die "exec $ARGV[0]: $!\n"' "$limit" "$@"; }
bounded() { perl -e 'alarm shift; exec @ARGV or die "exec $ARGV[0]: $!\n"' "$limit" "$@"; }

diagnose() {
  echo "--- sw_vers"
  sw_vers || true
  echo "--- security dump-trust-settings -d"
  bounded security dump-trust-settings -d || true
  echo "--- security find-identity -p codesigning (including untrusted)"
  bounded security find-identity -p codesigning "$kc" || true
}

describe_rc() {
  if [[ $1 -eq 142 ]]; then echo "timed out after ${limit}s"; else echo "exited $1"; fi
}

# Relax the trust-settings right for the add, then put the original rule back.
orig=$(mktemp)
trap 'rm -f "$orig"' EXIT
bounded security authorizationdb read "$right" > "$orig" 2>/dev/null || : > "$orig"
rc=0
bounded_sudo security authorizationdb write "$right" allow || rc=$?
if [[ $rc -ne 0 ]]; then
  echo "::warning::'security authorizationdb write $right allow' $(describe_rc "$rc"); trying add-trusted-cert anyway"
fi

rc=0
bounded_sudo security add-trusted-cert -d -r trustRoot -p codeSign \
  -k /Library/Keychains/System.keychain "$pem" || rc=$?

if [[ -s $orig ]]; then
  bounded_sudo security authorizationdb write "$right" < "$orig" ||
    echo "::warning::could not restore the original $right rule"
fi

if [[ $rc -ne 0 ]]; then
  echo "::error::'security add-trusted-cert' $(describe_rc "$rc"): codesign cannot use the self-signed '$identity' without it (see docs/fork-release.md, Troubleshooting)"
  diagnose
  exit 1
fi

# Capture first: grep -q on a live pipe can SIGPIPE security under pipefail.
valid=$(bounded security find-identity -v -p codesigning "$kc" || true)
if ! grep -qF "\"$identity\"" <<< "$valid"; then
  echo "::error::'$identity' is not a valid codesigning identity after trusting it (see docs/fork-release.md, Troubleshooting)"
  diagnose
  exit 1
fi
echo "'$identity' is trusted for code signing"

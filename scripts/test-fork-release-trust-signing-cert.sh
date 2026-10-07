#!/usr/bin/env bash
# Behavior tests for scripts/fork-release-trust-signing-cert.sh against stubbed
# sudo/security/sw_vers, plus the workflow wiring that bounds it.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
script="$root/scripts/fork-release-trust-signing-cert.sh"
workflow="$root/.github/workflows/fork-release.yml"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
stubs="$tmp/bin"
mkdir -p "$stubs"

cat > "$stubs/sudo" <<'SH'
#!/usr/bin/env bash
exec "$@"
SH
cat > "$stubs/sw_vers" <<'SH'
#!/usr/bin/env bash
echo "ProductVersion:		26.0"
SH
# STUB_AUTHDB / STUB_TRUST: ok | fail | hang. STUB_VALID: yes | no.
cat > "$stubs/security" <<'SH'
#!/usr/bin/env bash
echo "security $*" >> "$STUB_LOG"
act() {
  case $1 in
    ok) return 0 ;;
    fail) echo "SecTrustSettingsSetTrustSettings: The authorization was denied since no user interaction was possible." >&2; return 1 ;;
    hang) exec sleep 30 ;;
  esac
}
case "$1 ${2:-}" in
  "authorizationdb read") echo "<plist>original-rule</plist>" ;;
  "authorizationdb write") if [[ ${4:-} == allow ]]; then act "$STUB_AUTHDB"; else cat > /dev/null; fi ;;
  "add-trusted-cert "*) act "$STUB_TRUST" ;;
  "dump-trust-settings -d") echo "Cert 0: Buzz Local Code Signing" ;;
  "find-identity -v")
    if [[ $STUB_VALID == yes ]]; then echo '  1) 8C58 "Buzz Local Code Signing"'; fi ;;
  "find-identity -p") echo '  1) 8C58 "Buzz Local Code Signing" (CSSMERR_TP_NOT_TRUSTED)' ;;
  *) echo "unexpected security call: $*" >&2; exit 99 ;;
esac
SH
chmod +x "$stubs"/*

fail() { echo "FAIL [$case_name]: $*" >&2; echo "--- output"; cat "$tmp/out" >&2; echo "--- calls"; cat "$tmp/log" >&2; exit 1; }

# run <name> <authdb> <trust> <valid> <expected-exit>
run() {
  case_name=$1
  : > "$tmp/log"
  local start end rc=0
  start=$(date +%s)
  PATH="$stubs:$PATH" STUB_LOG="$tmp/log" STUB_AUTHDB=$2 STUB_TRUST=$3 STUB_VALID=$4 \
    BUZZ_TRUST_TIMEOUT_SECONDS=3 \
    "$script" "$tmp/kc" "$tmp/sign.pem" "Buzz Local Code Signing" > "$tmp/out" 2>&1 || rc=$?
  end=$(date +%s)
  [[ $rc -eq $5 ]] || fail "exit $rc, expected $5"
  (( end - start < 20 )) || fail "took $((end - start))s; a hung trust call was not bounded"
}
has() { grep -qF -- "$1" "$tmp/out" || fail "output lacks: $1"; }
called() { grep -qF -- "$1" "$tmp/log" || fail "security not called with: $1"; }
diagnosed() {
  has "ProductVersion"
  has "Cert 0: Buzz Local Code Signing"
  has "CSSMERR_TP_NOT_TRUSTED"
}
restored_after_add() {
  local add restore
  add=$(grep -n 'add-trusted-cert' "$tmp/log" | head -1 | cut -d: -f1 || true)
  restore=$(grep -n 'authorizationdb write com.apple.trust-settings.admin$' "$tmp/log" | tail -1 | cut -d: -f1 || true)
  [[ -n $add && -n $restore && $restore -gt $add ]] || fail "original trust-settings rule not restored after add-trusted-cert"
}

run happy ok ok yes 0
called "authorizationdb write com.apple.trust-settings.admin allow"
called "add-trusted-cert -d -r trustRoot -p codeSign -k /Library/Keychains/System.keychain $tmp/sign.pem"
restored_after_add
has "is trusted for code signing"
! grep -q "::warning::\|timed out" "$tmp/out" || fail "happy path must not warn or time out"
grep -qF "allow" <(head -2 "$tmp/log" | tail -1) || fail "authorizationdb allow must precede add-trusted-cert"

run trust-hangs ok hang no 1
has "timed out after 3s"
diagnosed
restored_after_add

run trust-denied ok fail no 1
has "'security add-trusted-cert' exited 1"
has "The authorization was denied"
diagnosed
restored_after_add

run authdb-denied-trust-ok fail ok yes 0
has "::warning::'security authorizationdb write com.apple.trust-settings.admin allow' exited 1"

run authdb-hangs-trust-ok hang ok yes 0
has "timed out after 3s; trying add-trusted-cert anyway"

run trusted-but-not-valid ok ok no 1
has "is not a valid codesigning identity"
diagnosed

# The workflow must call the bounded script inside a step-level timeout.
python3 - "$workflow" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
step = re.search(r"(?ms)^      - name: Import signing certificate\n(.*?)(?=^      - (?:name:|uses:))", text)
if not step:
    raise SystemExit("Import signing certificate step missing")
body = step.group(1)
m = re.search(r"^        timeout-minutes: (\d+)$", body, re.M)
if not m or int(m.group(1)) > 10:
    raise SystemExit("Import signing certificate needs a step timeout-minutes <= 10")
if "scripts/fork-release-trust-signing-cert.sh" not in body:
    raise SystemExit("Import signing certificate must trust the cert via the bounded script")
if re.search(r"^\s*sudo security add-trusted-cert", body, re.M):
    raise SystemExit("unbounded add-trusted-cert in the workflow step")
PY

echo "fork-release-trust-signing-cert: all cases passed"

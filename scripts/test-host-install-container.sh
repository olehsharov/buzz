#!/usr/bin/env bash
# Run the Buzz host installer the way a user does — the exact one-line
# snippet Buzz desktop shows, piped with no TTY — in fresh Ubuntu containers
# as the non-root user `ubuntu`, and check the machine ends up ready to run
# Claude agents.
#
#   scripts/test-host-install-container.sh                # 22.04 + 24.04
#   IMAGES="ubuntu:24.04" PLATFORM=linux/amd64 scripts/test-host-install-container.sh
#   HOST_ARTIFACTS=out scripts/test-host-install-container.sh  # real tarballs
#
# A local HTTP server plays the relay's /host/ directory, serving
# install.sh with its base URL filled in exactly as the relay does
# (crates/buzz-relay/src/host_installer.rs). Node, claude-agent-acp and
# Claude Code are installed for real from nodejs.org and npm.
#
# Without HOST_ARTIFACTS (a directory holding sprig-<arch>-unknown-linux-musl
# tarballs, e.g. from `docker buildx build --target host-bundle-files`), the
# Sprig tarball is a stand-in `buzz` that answers `host --help` and
# `host status --json` and records `host up` / `host install-service`
# arguments: pairing needs a live desktop, and the daemon needs systemd.
#
# Cases per image: snippet with no TTY; the same snippet again (idempotent
# upgrade); the snippet with a TTY; the snippet without --base (the relay's
# substituted base); and `| sh`, which must fail with a clear message.
# Requires docker and python3.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGES="${IMAGES:-ubuntu:22.04 ubuntu:24.04}"
PLATFORM="${PLATFORM:-}"
PORT="${PORT:-18765}"
URI='nostrpair://0123abcd?relay=wss%3A%2F%2Fpair.example&secret=s3cr3t&v=1'

WORK="$(mktemp -d)"
SERVER_PID=""
# shellcheck disable=SC2329 # invoked by the EXIT trap
cleanup() {
    [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

BASE="http://host.docker.internal:$PORT/host"
mkdir -p "$WORK/www/host" "$WORK/stub"

# install.sh exactly as the relay serves it to this host.
sed "s|__BUZZ_HOST_BASE__|$BASE|g" "$ROOT/scripts/install-buzz-host.sh" > "$WORK/www/host/install.sh"

if [[ -n "${HOST_ARTIFACTS:-}" ]]; then
    cp "$HOST_ARTIFACTS"/sprig-*-unknown-linux-musl.tar.gz "$WORK/www/host/"
    echo "using real Sprig tarballs from $HOST_ARTIFACTS"
else
    cat > "$WORK/stub/sprig" <<'STUB'
#!/bin/sh
# Stand-in for the static Sprig binary (see test-host-install-container.sh).
state="$HOME/.fake-buzz"
mkdir -p "$state"
[ "$(basename "$0")" = buzz ] || { echo "fake $(basename "$0")"; exit 0; }
[ "${1:-}" = host ] || { echo "fake buzz"; exit 0; }
shift
case "${1:-}" in
    --help) echo "Usage: buzz host <command>" ;;
    status)
        if [ -f "$state/paired" ]; then echo '{"paired": true}'; else echo '{"paired": false}'; fi ;;
    up)
        if [ -t 0 ]; then tty=tty; else tty=notty; fi
        echo "$* [$tty]" >> "$state/calls.log"
        touch "$state/paired"
        echo "fake buzz host: paired and running" ;;
    install-service) echo "install-service" >> "$state/calls.log" ;;
    *) echo "fake buzz host: unexpected $*" >&2; exit 2 ;;
esac
STUB
    chmod 0755 "$WORK/stub/sprig"
    for arch in x86_64 aarch64; do
        tar -czf "$WORK/www/host/sprig-$arch-unknown-linux-musl.tar.gz" -C "$WORK/stub" sprig
    done
    echo "using a stand-in buzz binary (records host up / install-service)"
fi
(
    cd "$WORK/www/host"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -- *.tar.gz > SHA256SUMS
    else
        shasum -a 256 -- *.tar.gz > SHA256SUMS
    fi
)

python3 -m http.server "$PORT" --bind 0.0.0.0 --directory "$WORK/www" >"$WORK/http.log" 2>&1 &
SERVER_PID=$!
sleep 1

# The exact line Buzz desktop shows (desktop/src-tauri/src/commands/hosts.rs).
SNIPPET="curl -fsSL '$BASE/install.sh' | bash -s -- --base '$BASE' --uri '$URI'"
NO_BASE="curl -fsSL '$BASE/install.sh' | bash -s -- --uri '$URI'"
WITH_SH="curl -fsSL '$BASE/install.sh' | sh -s -- --base '$BASE' --uri '$URI'"

# Runs inside the container as root; the user steps run as `ubuntu`.
cat > "$WORK/inside.sh" <<'INSIDE'
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq curl ca-certificates xz-utils >/dev/null
id ubuntu >/dev/null 2>&1 || useradd -m -s /bin/bash ubuntu
as_user() { runuser -l ubuntu -c "$1"; }
fail() { echo "FAIL: $*"; exit 1; }
pass() { echo "PASS: $*"; }

echo "--- $MODE: snippet"
as_user "$SNIPPET" < /dev/null > /tmp/run1.log 2>&1 || { cat /tmp/run1.log; fail "snippet exited non-zero"; }
tail -n 4 /tmp/run1.log
pass "snippet exit 0"

H=/home/ubuntu
for bin in buzz node npm npx claude-agent-acp claude; do
    [ -x "$H/.local/bin/$bin" ] || fail "$H/.local/bin/$bin missing"
done
for bin in node npm npx claude; do
    v="$(as_user "$H/.local/bin/$bin --version" < /dev/null 2>&1 | head -n 1)" || fail "$bin --version"
    echo "    $bin $v"
done
as_user "$H/.local/bin/buzz host --help" < /dev/null >/dev/null || fail "buzz host --help"
head -n 1 "$(readlink -f "$H/.local/bin/claude-agent-acp")" | grep -q node || fail "claude-agent-acp is not a node script"
pass "buzz node npm npx claude-agent-acp claude in ~/.local/bin and run"

p="$(as_user 'bash -lc "command -v claude-agent-acp"' < /dev/null)"
[ "$p" = "$H/.local/bin/claude-agent-acp" ] || fail "login PATH resolves claude-agent-acp to '$p'"
pass "login shell PATH finds ~/.local/bin"

if [ -f "$H/.fake-buzz/calls.log" ]; then
    grep -qF -- "up --yes --uri $URI [notty]" "$H/.fake-buzz/calls.log" || { cat "$H/.fake-buzz/calls.log"; fail "buzz host up not called with the URI"; }
    pass "buzz host up --yes --uri <uri> (no TTY)"
fi

echo "--- $MODE: same snippet again"
as_user "$SNIPPET" < /dev/null > /tmp/run2.log 2>&1 || { cat /tmp/run2.log; fail "rerun exited non-zero"; }
grep -q "already in" /tmp/run2.log || fail "rerun reinstalled Node"
n="$(grep -c 'Added by the Buzz host installer' "$H/.profile")"
[ "$n" = 1 ] || fail "~/.profile has $n PATH blocks"
if [ -f "$H/.fake-buzz/calls.log" ]; then
    [ "$(grep -c -- '--uri' "$H/.fake-buzz/calls.log")" -ge 2 ] || fail "rerun did not run buzz host up"
fi
pass "rerun exit 0, Node kept, one PATH block, daemon re-upped"

echo "--- $MODE: without --base (relay-substituted base)"
as_user "rm -rf ~/.fake-buzz; $NO_BASE" < /dev/null > /tmp/run3.log 2>&1 || { cat /tmp/run3.log; fail "no --base exited non-zero"; }
pass "no --base exit 0"

echo "--- $MODE: piped into sh"
if as_user "$WITH_SH" < /dev/null > /tmp/run4.log 2>&1; then cat /tmp/run4.log; fail "| sh succeeded"; fi
grep -q "needs bash" /tmp/run4.log || { cat /tmp/run4.log; fail "| sh error unclear"; }
grep -q "pipefail" /tmp/run4.log && fail "| sh hit the pipefail error"
sed 's/^/    /' /tmp/run4.log
pass "| sh fails with a clear message"
INSIDE

# With a TTY (-t): the snippet hands the terminal to buzz host up.
cat > "$WORK/inside-tty.sh" <<'INSIDE'
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq curl ca-certificates xz-utils >/dev/null
id ubuntu >/dev/null 2>&1 || useradd -m -s /bin/bash ubuntu
echo "--- $MODE: snippet with a TTY"
# --pty: plain `su -c` calls setsid() and drops the controlling terminal,
# unlike a real ssh login.
su --pty -l ubuntu -c "$SNIPPET" > /tmp/tty.log 2>&1 || { cat /tmp/tty.log; echo "FAIL: tty snippet"; exit 1; }
for bin in buzz node npm npx claude-agent-acp claude; do
    [ -x "/home/ubuntu/.local/bin/$bin" ] || { echo "FAIL: $bin missing"; exit 1; }
done
if [ -f /home/ubuntu/.fake-buzz/calls.log ]; then
    grep -qF -- "--uri $URI [tty]" /home/ubuntu/.fake-buzz/calls.log \
        || { cat /home/ubuntu/.fake-buzz/calls.log; echo "FAIL: up did not get the TTY"; exit 1; }
fi
echo "PASS: snippet with a TTY exit 0, tools installed, buzz host up got the terminal"
INSIDE

status=0
for image in $IMAGES; do
    plat=()
    [[ -n "$PLATFORM" ]] && plat=(--platform "$PLATFORM")
    label="$image${PLATFORM:+ ($PLATFORM)}"
    echo "=== $label: no TTY"
    if ! docker run --rm -i "${plat[@]}" --add-host=host.docker.internal:host-gateway \
        -e MODE="$label" -e SNIPPET="$SNIPPET" -e NO_BASE="$NO_BASE" -e WITH_SH="$WITH_SH" -e URI="$URI" \
        -v "$WORK/inside.sh:/inside.sh:ro" "$image" sh /inside.sh < /dev/null; then
        echo "=== $label: no TTY FAILED"; status=1
    fi
    echo "=== $label: TTY"
    # `docker run -t` gives the container a terminal; it needs none here.
    if ! docker run --rm -t "${plat[@]}" --add-host=host.docker.internal:host-gateway \
        -e MODE="$label" -e SNIPPET="$SNIPPET" -e URI="$URI" \
        -v "$WORK/inside-tty.sh:/inside.sh:ro" "$image" sh /inside.sh < /dev/null | tr -d '\r'; then
        echo "=== $label: TTY FAILED"; status=1
    fi
done
[[ "$status" == 0 ]] && echo "ALL PASSED" || echo "SOME CASES FAILED"
exit "$status"

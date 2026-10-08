#!/usr/bin/env bash
# Install `buzz host` on this machine and pair it with your Buzz desktop.
#
# Every Buzz relay image serves this script and the binaries it installs at
# https://<relay>/host/. Buzz desktop's "Add machine" shows the exact command:
#
#   curl -fsSL 'https://<relay>/host/install.sh' | bash -s -- \
#     --base 'https://<relay>/host' --uri '<pairing uri>'
#
# Options (each also settable through the environment):
#   --base URL       where install.sh, SHA256SUMS and the tarballs live
#                    (BUZZ_HOST_BASE). Required: `curl | bash` cannot know
#                    the URL it was fetched from.
#   --uri URI        pairing URI from Buzz desktop (BUZZ_HOST_URI); prompted
#                    for when omitted
#   --name NAME      machine name shown in Buzz desktop (BUZZ_HOST_NAME,
#                    default: hostname)
#   --install-deps   install Node.js and claude-agent-acp without asking
#   --install-only   install or upgrade the binaries; do not pair or start
#   --prefix DIR     where binaries are installed (BUZZ_HOST_PREFIX,
#                    default ~/.local/bin)
#
# Re-running upgrades the binaries in place. On Linux it installs the static
# Sprig multicall binary (`sprig` plus links: buzz, buzz-host, buzz-acp,
# buzz-agent, buzz-dev-mcp, git-credential-nostr, git-sign-nostr). On macOS
# it links /Applications/Buzz.app's binaries when that build knows
# `buzz host`, and otherwise downloads the macOS build from the relay.
#
# It never passes --relay to `buzz host up`: the pairing URI names the relay
# to pair through, and overriding it would leave pairing waiting forever.

set -euo pipefail

BASE="${BUZZ_HOST_BASE:-}"
URI="${BUZZ_HOST_URI:-}"
NAME="${BUZZ_HOST_NAME:-}"
PREFIX="${BUZZ_HOST_PREFIX:-$HOME/.local/bin}"
INSTALL_DEPS=0
INSTALL_ONLY=0
APP_DIR="/Applications/Buzz.app/Contents/MacOS"
LINKS=(buzz buzz-host buzz-acp buzz-agent buzz-dev-mcp git-credential-nostr git-sign-nostr)

say() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need_value() { [[ $# -ge 2 && -n "$2" ]] || die "$1 needs a value"; }

usage() {
    cat <<'USAGE'
Usage: install.sh --base URL [--uri URI] [--name NAME] [--install-deps]
                  [--install-only] [--prefix DIR]
See the header of this script for details.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --base) need_value "$@"; BASE="$2"; shift 2 ;;
        --uri) need_value "$@"; URI="$2"; shift 2 ;;
        --name) need_value "$@"; NAME="$2"; shift 2 ;;
        --prefix) need_value "$@"; PREFIX="$2"; shift 2 ;;
        --install-deps|--yes|-y) INSTALL_DEPS=1; shift ;;
        --install-only|--no-up) INSTALL_ONLY=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done
BASE="${BASE%/}"

# Prompts read the terminal, so `curl | bash` still works.
has_tty() { { : < /dev/tty; } 2>/dev/null; }
ask() {
    local answer=""
    if [[ "$INSTALL_DEPS" == 1 ]]; then return 0; fi
    if has_tty; then
        printf '%s [y/N] ' "$1" > /dev/tty
        read -r answer < /dev/tty || true
    fi
    [[ "$answer" =~ ^[Yy] ]]
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=aarch64 ;;
    *) die "unsupported CPU architecture: $arch" ;;
esac
case "$os" in
    Linux) artifact="sprig-${arch}-unknown-linux-musl.tar.gz" ;;
    Darwin) artifact="buzz-${arch}-apple-darwin.tar.gz" ;;
    *) die "unsupported OS: $os (Linux and macOS are supported)" ;;
esac
say "Detected $os/$arch"

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"
mkdir -p "$PREFIX"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# macOS: reuse Buzz.app's binaries when that build already knows `buzz host`.
link_app_binaries() {
    [[ "$os" == Darwin && -x "$APP_DIR/buzz" ]] || return 1
    if ! "$APP_DIR/buzz" host --help >/dev/null 2>&1; then
        warn "Buzz.app's buzz has no 'host' command; downloading buzz instead"
        return 1
    fi
    local name linked=0
    for name in "${LINKS[@]}"; do
        if [[ -x "$APP_DIR/$name" ]]; then
            ln -sfn "$APP_DIR/$name" "$PREFIX/$name"
            linked=1
        fi
    done
    [[ "$linked" == 1 ]] || return 1
    say "Linked Buzz.app binaries into $PREFIX"
}

download_and_install() {
    [[ -n "$BASE" ]] || die "pass --base https://<relay>/host (the directory this script came from)"
    local url="$BASE/$artifact" want got
    say "Downloading $url"
    curl -fsSL "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS" \
        || die "could not fetch $BASE/SHA256SUMS"
    want="$(awk -v f="$artifact" '$2 == f || $2 == "*" f {print $1}' "$TMP/SHA256SUMS")"
    if [[ -z "$want" ]]; then
        [[ "$os" == Darwin ]] && die "this relay has no macOS build of buzz; install Buzz.app (with 'buzz host') and re-run"
        die "this relay has no $artifact"
    fi
    curl -fsSL "$url" -o "$TMP/$artifact" || die "download failed: $url"
    got="$(sha256_of "$TMP/$artifact")"
    [[ "$want" == "$got" ]] || die "checksum mismatch for $artifact (want $want, got $got)"
    say "Checksum OK"

    mkdir -p "$TMP/unpack"
    tar -xzf "$TMP/$artifact" -C "$TMP/unpack"
    [[ -f "$TMP/unpack/sprig" ]] || die "$artifact has no sprig binary"
    # Replace atomically so a running daemon keeps its old inode until restart.
    install -m 0755 "$TMP/unpack/sprig" "$PREFIX/.sprig.new"
    mv -f "$PREFIX/.sprig.new" "$PREFIX/sprig"
    local name
    for name in "${LINKS[@]}"; do
        ln -sfn sprig "$PREFIX/$name"
    done
    say "Installed $PREFIX/sprig and linked: ${LINKS[*]}"
}

link_app_binaries || download_and_install
export PATH="$PREFIX:$HOME/.npm-global/bin:$PATH"
"$PREFIX/buzz" host --help >/dev/null 2>&1 || die "$PREFIX/buzz does not support 'buzz host'"
say "buzz host is ready ($PREFIX/buzz)"

if ! grep -qsF "$PREFIX" "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.bash_profile" 2>/dev/null; then
    warn "$PREFIX may not be on your shell PATH; add: export PATH=\"$PREFIX:\$PATH\""
fi

# Agent runtime: Node + the Claude ACP adapter + Claude Code itself.
if ! command -v node >/dev/null 2>&1; then
    warn "Node.js is not installed (needed for claude-agent-acp)"
    if [[ "$os" == Darwin ]] && command -v brew >/dev/null 2>&1 && ask "Install Node with Homebrew?"; then
        brew install node
    elif [[ "$os" == Linux ]] && ask "Install Node 22 into ~/.local/share/node from nodejs.org?"; then
        node_arch=x64; [[ "$arch" == aarch64 ]] && node_arch=arm64
        node_url="https://nodejs.org/dist/latest-v22.x/"
        node_tar="$(curl -fsSL "$node_url" | grep -o "node-v22[0-9.]*-linux-${node_arch}.tar.xz" | head -1)"
        [[ -n "$node_tar" ]] || die "could not find a Node 22 tarball"
        mkdir -p "$HOME/.local/share/node"
        curl -fsSL "$node_url$node_tar" | tar -xJ -C "$HOME/.local/share/node" --strip-components=1
        for b in node npm npx; do ln -sfn "$HOME/.local/share/node/bin/$b" "$PREFIX/$b"; done
    else
        warn "skipping Node; install it (or re-run with --install-deps) before deploying Claude agents"
    fi
fi
if command -v npm >/dev/null 2>&1 && ! command -v claude-agent-acp >/dev/null 2>&1; then
    if ask "Install @agentclientprotocol/claude-agent-acp with npm?"; then
        if npm config get prefix | grep -q "^/usr"; then
            npm config set prefix "$HOME/.npm-global"
        fi
        npm install -g @agentclientprotocol/claude-agent-acp
    else
        warn "claude-agent-acp is not installed; Claude agents will not start (re-run with --install-deps)"
    fi
fi
if command -v node >/dev/null 2>&1 && command -v claude-agent-acp >/dev/null 2>&1; then
    say "Node $(node --version) and claude-agent-acp found"
fi
if command -v claude >/dev/null 2>&1; then
    say "Claude Code found ($(command -v claude))"
else
    warn "Claude Code ('claude') is not on PATH. Install it and run 'claude' once to log in."
fi

if [[ "$INSTALL_ONLY" == 1 ]]; then
    say "Installed. Pair with: buzz host up --uri '<pairing uri from Buzz desktop>'"
    say "Already paired? 'buzz host install-service' restarts the daemon on the new binary."
    exit 0
fi

up=("$PREFIX/buzz" host up)
[[ -n "$URI" ]] && up+=(--uri "$URI")
[[ -n "$NAME" ]] && up+=(--name "$NAME")
[[ -z "$URI" ]] && say "Pairing: in Buzz desktop open Settings → Machines → Add machine."
# stdin may be the curl pipe; pairing prompts read the terminal.
if has_tty; then
    "${up[@]}" < /dev/tty
else
    "${up[@]}"
fi

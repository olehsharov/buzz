#!/usr/bin/env bash
# Install `buzz host` on this machine and pair it with your Buzz desktop.
#
#   curl -fsSL <url-of-this-script> | bash -s -- --relay wss://relay.example
#
# Options (each also settable through the environment):
#   --relay URL        relay to pair through          (BUZZ_RELAY_URL)
#   --sprig-url URL    Sprig tarball to download      (BUZZ_SPRIG_URL)
#   --prefix DIR       where links are created        (BUZZ_HOST_PREFIX, default ~/.local/bin)
#   --sprig-dir DIR    where Sprig is unpacked        (BUZZ_SPRIG_DIR, default ~/.local/share/buzz/sprig)
#   --name NAME        machine name shown in desktop  (BUZZ_HOST_NAME)
#   --yes              install Node tools without asking
#   --no-up            install only; do not pair/start
#
# The Sprig tarball is the static multicall binary built by
# scripts/build-sprig.sh (`sprig-<target>.tar.gz`). The default URL below is
# a placeholder for the rolling release asset; point --sprig-url (or
# BUZZ_SPRIG_URL) at your own build until official assets are published.
# `{target}` in the URL is replaced with the Rust target triple.
#
# On macOS the binaries bundled in /Applications/Buzz.app are linked when
# that build already knows `buzz host`; otherwise Sprig is downloaded.

set -euo pipefail

DEFAULT_SPRIG_URL="https://github.com/block/buzz/releases/latest/download/sprig-{target}.tar.gz"
RELAY="${BUZZ_RELAY_URL:-}"
SPRIG_URL="${BUZZ_SPRIG_URL:-$DEFAULT_SPRIG_URL}"
PREFIX="${BUZZ_HOST_PREFIX:-$HOME/.local/bin}"
SPRIG_DIR="${BUZZ_SPRIG_DIR:-$HOME/.local/share/buzz/sprig}"
NAME="${BUZZ_HOST_NAME:-}"
ASSUME_YES=0
RUN_UP=1
APP_DIR="/Applications/Buzz.app/Contents/MacOS"
LINKS=(buzz buzz-acp buzz-dev-mcp git-credential-nostr)

while [[ $# -gt 0 ]]; do
    case "$1" in
        --relay) RELAY="$2"; shift 2 ;;
        --sprig-url) SPRIG_URL="$2"; shift 2 ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        --sprig-dir) SPRIG_DIR="$2"; shift 2 ;;
        --name) NAME="$2"; shift 2 ;;
        --yes|-y) ASSUME_YES=1; shift ;;
        --no-up) RUN_UP=0; shift ;;
        -h|--help) sed -n "2,22p" "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 1 ;;
    esac
done

say() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Prompts read the terminal, so `curl | bash` still works.
has_tty() { { : < /dev/tty; } 2>/dev/null; }
ask() {
    local answer=""
    if [[ "$ASSUME_YES" == 1 ]]; then return 0; fi
    if has_tty; then
        printf '%s [y/N] ' "$1" > /dev/tty
        read -r answer < /dev/tty || true
    fi
    [[ "$answer" =~ ^[Yy] ]]
}

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=aarch64 ;;
    *) die "unsupported CPU architecture: $arch" ;;
esac
case "$os" in
    Linux) target="${arch}-unknown-linux-musl" ;;
    Darwin) target="${arch}-apple-darwin" ;;
    *) die "unsupported OS: $os (Linux and macOS are supported)" ;;
esac
say "Detected $os/$arch ($target)"

mkdir -p "$PREFIX"

link_app_binaries() {
    [[ "$os" == Darwin && -x "$APP_DIR/buzz" ]] || return 1
    # Older desktop builds ship a `buzz` without the host subcommand.
    "$APP_DIR/buzz" host --help >/dev/null 2>&1 || {
        warn "Buzz.app's buzz has no 'host' command; downloading Sprig instead"
        return 1
    }
    for name in "${LINKS[@]}"; do
        [[ -x "$APP_DIR/$name" ]] && ln -sf "$APP_DIR/$name" "$PREFIX/$name"
    done
    say "Linked Buzz.app binaries into $PREFIX"
}

install_sprig() {
    local url="${SPRIG_URL//\{target\}/$target}"
    local share="$SPRIG_DIR"
    local tmp
    tmp="$(mktemp -d)"
    say "Downloading Sprig from $url"
    curl -fsSL "$url" -o "$tmp/sprig.tar.gz" || die "download failed: $url (set --sprig-url)"
    if curl -fsSL "$url.sha256" -o "$tmp/sprig.tar.gz.sha256" 2>/dev/null; then
        local want got
        want="$(awk '{print $1}' "$tmp/sprig.tar.gz.sha256")"
        if command -v sha256sum >/dev/null 2>&1; then
            got="$(sha256sum "$tmp/sprig.tar.gz" | awk '{print $1}')"
        else
            got="$(shasum -a 256 "$tmp/sprig.tar.gz" | awk '{print $1}')"
        fi
        [[ "$want" == "$got" ]] || die "checksum mismatch for $url"
    else
        warn "no .sha256 next to the tarball; skipping checksum verification"
    fi
    mkdir -p "$share"
    tar -xzf "$tmp/sprig.tar.gz" -C "$share"
    rm -rf "$tmp"
    [[ -x "$share/sprig" ]] || die "tarball has no sprig binary"
    for name in "${LINKS[@]}" buzz-agent buzz-host git-sign-nostr; do
        ln -sf "$share/sprig" "$PREFIX/$name"
    done
    say "Installed Sprig into $share and linked it into $PREFIX"
}

link_app_binaries || install_sprig
export PATH="$PREFIX:$HOME/.npm-global/bin:$PATH"
"$PREFIX/buzz" host --help >/dev/null 2>&1 || die "$PREFIX/buzz does not support 'buzz host'"

if ! grep -qs "$PREFIX" "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc" 2>/dev/null; then
    warn "$PREFIX is not on your shell PATH; add: export PATH=\"$PREFIX:\$PATH\""
fi

# Agent runtime: Node + the Claude ACP adapter + Claude Code itself.
if ! command -v node >/dev/null 2>&1; then
    warn "Node.js is not installed (needed for claude-agent-acp)"
    if [[ "$os" == Darwin ]] && command -v brew >/dev/null 2>&1 && ask "Install Node with Homebrew?"; then
        brew install node
    elif [[ "$os" == Linux ]] && ask "Install Node 22 into ~/.local via the official tarball?"; then
        node_arch=x64; [[ "$arch" == aarch64 ]] && node_arch=arm64
        node_url="https://nodejs.org/dist/latest-v22.x/"
        node_tar="$(curl -fsSL "$node_url" | grep -o "node-v22[0-9.]*-linux-${node_arch}.tar.xz" | head -1)"
        [[ -n "$node_tar" ]] || die "could not find a Node 22 tarball"
        mkdir -p "$HOME/.local/share/node"
        curl -fsSL "$node_url$node_tar" | tar -xJ -C "$HOME/.local/share/node" --strip-components=1
        for b in node npm npx; do ln -sf "$HOME/.local/share/node/bin/$b" "$PREFIX/$b"; done
    else
        warn "skipping Node; install it before deploying Claude agents"
    fi
fi
if command -v npm >/dev/null 2>&1 && ! command -v claude-agent-acp >/dev/null 2>&1; then
    if ask "Install @agentclientprotocol/claude-agent-acp with npm?"; then
        npm config get prefix | grep -q "^/usr" && npm config set prefix "$HOME/.npm-global"
        npm install -g @agentclientprotocol/claude-agent-acp
    else
        warn "claude-agent-acp is not installed; Claude agents will not start"
    fi
fi
if ! command -v claude >/dev/null 2>&1; then
    warn "Claude Code ('claude') is not on PATH. Install it and run 'claude' once to log in."
fi

if [[ "$RUN_UP" == 1 ]]; then
    up=("$PREFIX/buzz" host up)
    [[ -n "$RELAY" ]] && up+=(--relay "$RELAY")
    [[ -n "$NAME" ]] && up+=(--name "$NAME")
    say "Pairing: in Buzz desktop open Settings → Machines → Add machine."
    # stdin may be the curl pipe; pairing prompts read the terminal.
    if has_tty; then
        "${up[@]}" < /dev/tty
    else
        "${up[@]}"
    fi
else
    say "Installed. Run '$PREFIX/buzz host up' to pair."
fi

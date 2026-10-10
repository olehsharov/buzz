#!/usr/bin/env bash
# Install `buzz host` on this machine and pair it with your Buzz desktop.
#
# Every Buzz relay image serves this script and the binaries it installs at
# https://<relay>/host/. Buzz desktop's Settings → Machines → Add machine
# shows the one line to paste:
#
#   curl -fsSL 'https://<relay>/host/install.sh' | bash -s -- \
#     --base 'https://<relay>/host' --uri '<pairing uri>'
#
# Nothing else is needed: no prompts, no PATH edits, no follow-up commands.
# It installs, without asking:
#   * buzz host (the static Sprig binary, or Buzz.app's on macOS)
#   * Node.js 22 into ~/.local/share/node (user-local, no sudo)
#   * the Claude ACP adapter (@agentclientprotocol/claude-agent-acp)
#   * Claude Code (@anthropic-ai/claude-code)
# links buzz, node, npm, npx, claude-agent-acp and claude into ~/.local/bin,
# puts ~/.local/bin on your login PATH, pairs, and starts the daemon as a
# login service. Re-running it upgrades everything and restarts the daemon
# on the new binary; an already paired machine stays paired.
#
# Options (each also settable through the environment):
#   --base URL       where install.sh, SHA256SUMS and the tarballs live
#                    (BUZZ_HOST_BASE; default: the relay that served this
#                    script)
#   --uri URI        pairing URI from Buzz desktop (BUZZ_HOST_URI)
#   --name NAME      machine name shown in Buzz desktop (BUZZ_HOST_NAME,
#                    default: hostname)
#   --no-deps        do not install Node.js, claude-agent-acp or Claude Code
#   --install-only   install or upgrade; do not pair (a paired machine's
#                    daemon is still restarted on the new binary)
#   --prefix DIR     where binaries are linked (BUZZ_HOST_PREFIX,
#                    default ~/.local/bin)
#
# It never passes --relay to `buzz host up`: the pairing URI names the relay
# to pair through, and overriding it would leave pairing waiting forever.

# This block must stay POSIX: `curl … | sh` runs it under dash on Ubuntu.
# The relay replaces the placeholder with its own https://<host>/host.
BUZZ_HOST_SERVED_BASE='__BUZZ_HOST_BASE__'
if [ -z "${BASH_VERSION:-}" ]; then
    # `sh install.sh`: the script is a file, so bash can run it instead.
    if [ -f "$0" ] && head -n 1 -- "$0" 2>/dev/null | grep -q '^#!/usr/bin/env bash' \
        && command -v bash >/dev/null 2>&1; then
        exec bash "$0" "$@"
    fi
    case "$BUZZ_HOST_SERVED_BASE" in
        __BUZZ_HOST_*) _buzz_url='https://<relay>/host/install.sh' ;;
        *) _buzz_url="$BUZZ_HOST_SERVED_BASE/install.sh" ;;
    esac
    echo "error: this installer needs bash, not sh. Paste the command from Buzz desktop (Settings → Machines → Add machine), which pipes into bash:" >&2
    echo "  curl -fsSL '$_buzz_url' | bash -s -- --uri '<pairing uri>'" >&2
    exit 1
fi

# Everything below runs from main(), called on the last line, so bash has
# read the whole script before any command runs: with `curl | bash`, stdin
# is the script itself and a child reading stdin must not swallow it.

set -euo pipefail

NODE_MAJOR=22
NODE_DIR="$HOME/.local/share/node"
NPM_PACKAGES=(@agentclientprotocol/claude-agent-acp @anthropic-ai/claude-code)
APP_DIR="/Applications/Buzz.app/Contents/MacOS"
LINKS=(buzz buzz-host buzz-acp buzz-agent buzz-dev-mcp git-credential-nostr git-sign-nostr)
PROFILE_MARKER="# Added by the Buzz host installer"

say() { printf '==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need_value() { [[ $# -ge 2 && -n "$2" ]] || die "$1 needs a value"; }

usage() {
    cat <<'USAGE'
Usage: install.sh [--base URL] [--uri URI] [--name NAME] [--no-deps]
                  [--install-only] [--prefix DIR]
See the header of this script for details.
USAGE
}

has_tty() { { : < /dev/tty; } 2>/dev/null; }

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# The base this copy of the script was served from, if the relay filled it in.
served_base() {
    case "$BUZZ_HOST_SERVED_BASE" in
        __BUZZ_HOST_*) printf '' ;;
        *) printf '%s' "$BUZZ_HOST_SERVED_BASE" ;;
    esac
}

# macOS: reuse Buzz.app's binaries when that build already knows `buzz host`.
link_app_binaries() {
    [[ "$os" == Darwin && -x "$APP_DIR/buzz" ]] || return 1
    if ! "$APP_DIR/buzz" host --help >/dev/null 2>&1 < /dev/null; then
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
    [[ -n "$BASE" ]] || die "pass --base https://<relay>/host (copy the command from Buzz desktop: Settings → Machines → Add machine)"
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

node_major_of() {
    [[ -x "$1" ]] || return 1
    local version
    version="$("$1" --version 2>/dev/null < /dev/null)" || return 1
    version="${version#v}"
    printf '%s' "${version%%.*}"
}

# Node $NODE_MAJOR from nodejs.org into $NODE_DIR, verified against
# SHASUMS256.txt. Kept when a Node $NODE_MAJOR is already there.
install_node() {
    local current
    current="$(node_major_of "$NODE_DIR/bin/node" || true)"
    if [[ "$current" == "$NODE_MAJOR" ]]; then
        say "Node $("$NODE_DIR/bin/node" --version) already in $NODE_DIR"
        return 0
    fi
    local node_os node_arch dist="https://nodejs.org/dist/latest-v${NODE_MAJOR}.x"
    case "$os" in Linux) node_os=linux ;; Darwin) node_os=darwin ;; esac
    case "$arch" in x86_64) node_arch=x64 ;; aarch64) node_arch=arm64 ;; esac
    curl -fsSL "$dist/SHASUMS256.txt" -o "$TMP/node-sums" \
        || die "could not reach nodejs.org ($dist/SHASUMS256.txt)"
    local line file want got
    line="$(grep -E " node-v${NODE_MAJOR}\.[0-9.]+-${node_os}-${node_arch}\.tar\.gz\$" "$TMP/node-sums" | head -n 1 || true)"
    [[ -n "$line" ]] || die "nodejs.org lists no Node $NODE_MAJOR for ${node_os}-${node_arch}"
    want="${line%% *}"
    file="${line##* }"
    say "Installing Node ($file) into $NODE_DIR"
    curl -fsSL "$dist/$file" -o "$TMP/$file" || die "download failed: $dist/$file"
    got="$(sha256_of "$TMP/$file")"
    [[ "$want" == "$got" ]] || die "checksum mismatch for $file (want $want, got $got)"
    rm -rf "$NODE_DIR.new" "$NODE_DIR.old"
    mkdir -p "$NODE_DIR.new"
    tar -xzf "$TMP/$file" -C "$NODE_DIR.new" --strip-components=1
    [[ -x "$NODE_DIR.new/bin/node" ]] || die "$file has no bin/node"
    [[ -e "$NODE_DIR" ]] && mv "$NODE_DIR" "$NODE_DIR.old"
    mv "$NODE_DIR.new" "$NODE_DIR"
    rm -rf "$NODE_DIR.old"
}

# Link $1 to $2, but never replace a real file someone else put there.
link_tool() {
    local target="$1" link="$2"
    if [[ -e "$link" && ! -L "$link" ]]; then
        warn "$link exists and is not a link; leaving it (Buzz uses $target)"
        return 0
    fi
    ln -sfn "$target" "$link"
}

install_agent_runtime() {
    install_node
    # Later steps (npm, the checks, buzz host's service PATH) must see it.
    export PATH="$PREFIX:$NODE_DIR/bin:$PATH"
    local npm="$NODE_DIR/bin/npm"
    say "Installing ${NPM_PACKAGES[*]} (npm, latest)"
    # --prefix pins global installs to $NODE_DIR whatever ~/.npmrc says.
    npm_config_update_notifier=false \
    "$npm" install --global --prefix "$NODE_DIR" --no-fund --no-audit \
        --loglevel=error "${NPM_PACKAGES[@]/%/@latest}" < /dev/null \
        || die "npm could not install ${NPM_PACKAGES[*]}"
    local bin
    for bin in node npm npx claude-agent-acp claude; do
        [[ -x "$NODE_DIR/bin/$bin" ]] || die "$NODE_DIR/bin/$bin is missing after install"
        link_tool "$NODE_DIR/bin/$bin" "$PREFIX/$bin"
    done
}

# Every tool a Claude agent needs must run from $PREFIX, or the install fails.
verify_agent_runtime() {
    local bin out
    for bin in node npm npx claude; do
        out="$("$PREFIX/$bin" --version 2>&1 < /dev/null)" \
            || die "$PREFIX/$bin does not run: $out"
        say "$bin $(printf '%s' "$out" | head -n 1)"
    done
    # An ACP server waits on stdin; that it resolves and is executable is
    # the check (its shebang runs it with the node above).
    [[ -x "$PREFIX/claude-agent-acp" ]] || die "$PREFIX/claude-agent-acp is missing"
    say "claude-agent-acp $(readlink "$PREFIX/claude-agent-acp" 2>/dev/null || echo "$PREFIX/claude-agent-acp")"
}

# Put $PREFIX on the login PATH once, in every profile the user's shells read.
ensure_login_path() {
    local line profiles=("$HOME/.profile") profile
    # shellcheck disable=SC2016 # $PATH is expanded by the user's shell.
    line='case ":$PATH:" in *":'"$PREFIX"':"*) ;; *) export PATH="'"$PREFIX"':$PATH" ;; esac'
    [[ -f "$HOME/.bashrc" ]] && profiles+=("$HOME/.bashrc")
    [[ -f "$HOME/.bash_profile" ]] && profiles+=("$HOME/.bash_profile")
    if [[ "$os" == Darwin || -f "$HOME/.zshrc" ]]; then
        profiles+=("$HOME/.zprofile")
    fi
    for profile in "${profiles[@]}"; do
        grep -qsF "$PROFILE_MARKER" "$profile" && continue
        printf '\n%s\n%s\n' "$PROFILE_MARKER" "$line" >> "$profile"
        say "Added $PREFIX to PATH in $profile"
    done
}

is_paired() {
    "$PREFIX/buzz" host status --json 2>/dev/null < /dev/null | grep -q '"paired": *true'
}

# stdin may be the curl pipe: hand children the terminal, or nothing.
run_interactive() {
    if has_tty; then "$@" < /dev/tty; else "$@" < /dev/null; fi
}

main() {
    BASE="${BUZZ_HOST_BASE:-}"
    URI="${BUZZ_HOST_URI:-}"
    NAME="${BUZZ_HOST_NAME:-}"
    PREFIX="${BUZZ_HOST_PREFIX:-$HOME/.local/bin}"
    INSTALL_DEPS=1
    INSTALL_ONLY=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --base) need_value "$@"; BASE="$2"; shift 2 ;;
            --uri) need_value "$@"; URI="$2"; shift 2 ;;
            --name) need_value "$@"; NAME="$2"; shift 2 ;;
            --prefix) need_value "$@"; PREFIX="$2"; shift 2 ;;
            --no-deps) INSTALL_DEPS=0; shift ;;
            # Older commands: dependencies are now installed by default.
            --install-deps|--yes|-y) shift ;;
            --install-only|--no-up) INSTALL_ONLY=1; shift ;;
            -h|--help) usage; exit 0 ;;
            *) usage >&2; die "unknown option: $1" ;;
        esac
    done
    [[ -n "$BASE" ]] || BASE="$(served_base)"
    BASE="${BASE%/}"

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

    local tool
    for tool in curl tar grep awk; do
        command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
    done
    mkdir -p "$PREFIX"
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT

    link_app_binaries || download_and_install
    export PATH="$PREFIX:$PATH"
    "$PREFIX/buzz" host --help >/dev/null 2>&1 < /dev/null \
        || die "$PREFIX/buzz does not support 'buzz host'"
    say "buzz host is ready ($PREFIX/buzz)"

    if [[ "$INSTALL_DEPS" == 1 ]]; then
        install_agent_runtime
        verify_agent_runtime
    else
        warn "--no-deps: Claude agents need node, claude-agent-acp and claude on this machine"
    fi
    ensure_login_path

    if [[ "$INSTALL_ONLY" == 1 ]]; then
        if is_paired; then
            say "Restarting the daemon on the new binary"
            "$PREFIX/buzz" host install-service < /dev/null
        else
            say "Installed. To pair, run the command from Buzz desktop (Settings → Machines → Add machine)."
        fi
    else
        if [[ -z "$URI" ]] && ! is_paired && ! has_tty; then
            die "this machine is not paired yet: pass --uri '<pairing uri>' (copy the command from Buzz desktop: Settings → Machines → Add machine)"
        fi
        # The person pasting this compares the pairing code printed below
        # with Buzz desktop and approves there; nothing to answer here.
        local up=("$PREFIX/buzz" host up --yes)
        [[ -n "$URI" ]] && up+=(--uri "$URI")
        [[ -n "$NAME" ]] && up+=(--name "$NAME")
        run_interactive "${up[@]}"
    fi

    if [[ "$INSTALL_DEPS" == 1 ]]; then
        say "Done. Agents that use your Claude subscription need one sign-in on this machine: run 'claude' and log in (not needed when they use ANTHROPIC_BASE_URL + ANTHROPIC_AUTH_TOKEN)."
    fi
}

main "$@"

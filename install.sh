#!/usr/bin/env bash
set -euo pipefail
#
# install.sh — Install SPM binary from GitHub Releases
#
# Usage:
#   curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash
#   curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash -s -- --user
#   curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash -s -- --version v0.3.0
#
# Options:
#   --user          Install to ~/.local/bin instead of /usr/local/bin
#   --version TAG   Install a specific version (default: latest)
#   --help          Show this help
#

set -euo pipefail

BOLD='\033[1m'
RED='\033[31m'
GREEN='\033[32m'
YELLOW='\033[33m'
CYAN='\033[36m'
NC='\033[0m'

info()  { printf "${CYAN}%s${NC}\n" "$*"; }
ok()    { printf "${GREEN}%s${NC}\n" "$*"; }
warn()  { printf "${YELLOW}%s${NC}\n" "$*"; }
err()   { printf "${RED}%s${NC}\n" "$*"; }

usage() {
    sed -n '/^#$/q; /^#/p' "$0" | sed 's/^# //; s/^#//'
    exit 0
}

# ── Detect architecture ──
detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)  echo "x86_64-linux-gnu" ;;
        aarch64|arm64) echo "aarch64-linux-gnu" ;;
        armv7l|armv7)  echo "armv7-linux-gnueabihf" ;;
        i686|i386)     echo "i686-linux-gnu" ;;
        riscv64)       echo "riscv64-linux-gnu" ;;
        *)             err "Unsupported architecture: $arch"; exit 1 ;;
    esac
}

# ── Determine target binary name ──
binary_name() {
    local arch="$1"
    echo "spm-${arch}"
}

# ── Determine install paths ──
install_paths() {
    local user_mode="$1"
    if [ "$user_mode" = "user" ]; then
        BINDIR="${HOME}/.local/bin"
        SPMD_BINDIR="/usr/local/bin"
        SUDO="sudo"
    else
        BINDIR="/usr/local/bin"
        SPMD_BINDIR="/usr/local/bin"
        SUDO=""
    fi
}

# ── Download from GitHub Releases ──
download_release() {
    local version="$1"
    local binary="$2"
    local url

    if [ "$version" = "latest" ]; then
        url="https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/${binary}"
    else
        url="https://github.com/Aloosh101/Samad-Package-Manager/releases/download/${version}/${binary}"
    fi

    info "Downloading: ${url}"
    if command -v curl &>/dev/null; then
        curl -fsSL "$url" -o "/tmp/${binary}"
    elif command -v wget &>/dev/null; then
        wget -q "$url" -O "/tmp/${binary}"
    else
        err "Neither curl nor wget found. Install one and try again."
        exit 1
    fi

    if [ ! -s "/tmp/${binary}" ]; then
        err "Download failed: empty file"
        exit 1
    fi

    chmod +x "/tmp/${binary}"
    echo "/tmp/${binary}"
}

# ── Install ──
install_binary() {
    local binary_path="$1"
    local binary_name="$2"
    local dest="$3"
    local sudo_cmd="$4"

    info "Installing ${binary_name} → ${dest}/${binary_name}"
    $sudo_cmd mkdir -p "$dest"
    $sudo_cmd cp -f "$binary_path" "${dest}/${binary_name}"
    $sudo_cmd chmod 755 "${dest}/${binary_name}"

    if [ "$binary_name" = "spm" ] && [ "$dest" = "/usr/local/bin" ]; then
        # Create /usr/bin symlink for sudo PATH
        if [ ! -L /usr/bin/spm ] || [ "$(readlink /usr/bin/spm)" != "/usr/local/bin/spm" ]; then
            $sudo_cmd ln -sf /usr/local/bin/spm /usr/bin/spm
        fi
    fi

    ok "${binary_name} installed"
}

# ── Main ──
main() {
    local user_mode=""
    local version="latest"

    for arg in "$@"; do
        case "$arg" in
            --help|-h) usage ;;
            --user)    user_mode="user" ;;
            --version=*) version="${arg#*=}" ;;
            --version) ;;
        esac
    done

    # If --user, no sudo needed for spm binary
    if [ -z "$user_mode" ]; then
        if [ "$(id -u)" -ne 0 ]; then
            warn "Root installation requires root. Re-run with: sudo bash install.sh"
            warn "Or use: bash install.sh --user"
            exit 1
        fi
        user_mode="root"
    fi

    # Detect architecture
    local arch
    arch="$(detect_arch)"
    local bin_name
    bin_name="$(binary_name "$arch")"
    info "Detected architecture: ${arch}"

    # Download binary
    local dl_path
    dl_path="$(download_release "$version" "$bin_name")"

    # Install spm
    install_paths "$user_mode"
    install_binary "$dl_path" "spm" "$BINDIR" "$SUDO"

    # Download and install spmd
    local dl_path_d
    dl_path_d="$(download_release "$version" "spmd-${arch}")"
    install_binary "$dl_path_d" "spmd" "$SPMD_BINDIR" "$SUDO"

    # Cleanup
    rm -f "$dl_path" "$dl_path_d"

    # Post-install
    info "Running: spm init"
    $SUDO spm init 2>/dev/null || warn "spm init failed — run manually later"

    echo ""
    ok "SPM $(spm --version 2>/dev/null || echo '') installed successfully!"
    echo "  spm  → ${BINDIR}/spm"
    echo "  spmd → ${SPMD_BINDIR}/spmd"
    echo ""
    echo "Next steps:"
    echo "  spm repo add debian --source deb --mirrors https://deb.debian.org/debian --codename stable --components main"
    echo "  spm update"
    echo "  spm install figlet"
}

main "$@"

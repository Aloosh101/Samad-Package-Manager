#!/usr/bin/env bash
set -euo pipefail
#
# install.sh — Install SPM binary from GitHub Releases
#
# Usage:
#   curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash
#   curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash -s -- --user
#   curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | bash -s -- --version v0.3.1
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
BLUE='\033[34m'
MAGENTA='\033[35m'
DIM='\033[2m'
NC='\033[0m'
ERASE='\033[2K'

step()   { printf "\n${BOLD}${BLUE}▸ Step %s${NC}\n" "$*"; }
info()   { printf "  ${CYAN}%s${NC}\n" "$*"; }
ok()     { printf "  ${GREEN}✔ %s${NC}\n" "$*"; }
warn()   { printf "  ${YELLOW}⚠ %s${NC}\n" "$*"; }
err()    { printf "  ${RED}✘ %s${NC}\n" "$*"; }
header() { printf "\n${BOLD}${MAGENTA}══ %s ══${NC}\n" "$*"; }
detail() { printf "  ${DIM}%s${NC}\n" "$*"; }

usage() {
    sed -n '/^#$/q; /^#/p' "$0" | sed 's/^# //; s/^#//'
    exit 0
}

# ── Spinner ──
spinner() {
    local pid=$1
    local msg=$2
    local spin='|/-\'
    local i=0
    while kill -0 "$pid" 2>/dev/null; do
        printf "  ${CYAN}%c${NC} ${DIM}%s${NC}\r" "${spin:$i:1}" "$msg"
        i=$(( (i+1) % 4 ))
        sleep 0.15
    done
    printf "  ${GREEN}✔${NC} ${DIM}%s${NC}  \n" "$msg"
    wait "$pid"
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

# ── Download with progress ──
download_release() {
    local version="$1"
    local binary="$2"
    local url

    if [ "$version" = "latest" ]; then
        url="https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/${binary}"
    else
        url="https://github.com/Aloosh101/Samad-Package-Manager/releases/download/${version}/${binary}"
    fi

    info "Fetching: ${DIM}${url}${NC}"

    if command -v curl &>/dev/null; then
        curl -fL --progress-bar "$url" -o "/tmp/${binary}" 2>&1 | while IFS= read -r line; do
            if [[ "$line" =~ [0-9]+% ]]; then
                printf "  ${CYAN}⬇${NC} ${DIM}Downloading: %s${NC}\r" "$line"
            fi
        done
        printf "  ${GREEN}✔${NC} ${DIM}Downloaded: ${binary}${NC}  \n"
    elif command -v wget &>/dev/null; then
        info "Downloading with wget... (no progress bar)"
        wget -q --show-progress "$url" -O "/tmp/${binary}" 2>&1
        ok "Downloaded: ${binary}"
    else
        err "Neither curl nor wget found. Install one and try again."
        exit 1
    fi

    if [ ! -s "/tmp/${binary}" ]; then
        err "Download failed — file is empty"
        exit 1
    fi

    local size
    size=$(stat -c%s "/tmp/${binary}" 2>/dev/null || stat -f%z "/tmp/${binary}" 2>/dev/null || echo "?")
    if [ "$size" != "?" ]; then
        size=$(echo "$size" | awk '{printf "%.1f MB", $1/1048576}')
        detail "Size: ${size}"
    fi

    chmod +x "/tmp/${binary}"
    echo "/tmp/${binary}"
}

# ── Verify checksum ──
verify_checksum() {
    local binary_path="$1"
    local binary_name="$2"
    local version="$3"

    local checksums_url
    if [ "$version" = "latest" ]; then
        checksums_url="https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/checksums.txt"
    else
        checksums_url="https://github.com/Aloosh101/Samad-Package-Manager/releases/download/${version}/checksums.txt"
    fi

    if ! command -v sha256sum &>/dev/null; then
        detail "sha256sum not available — skipping verification"
        return
    fi

    local expected
    if curl -fsSL "$checksums_url" -o "/tmp/checksums.txt" 2>/dev/null; then
        expected=$(grep "  ${binary_name}$" "/tmp/checksums.txt" | awk '{print $1}' || true)
        if [ -n "$expected" ]; then
            local actual
            actual=$(sha256sum "$binary_path" | awk '{print $1}')
            if [ "$expected" != "$actual" ]; then
                err "Checksum mismatch! Expected: ${expected}"
                err "Actual:   ${actual}"
                exit 1
            fi
            ok "Checksum verified"
        else
            detail "No checksum found for ${binary_name} — skipping verification"
        fi
        rm -f "/tmp/checksums.txt"
    else
        detail "Could not fetch checksums — skipping verification"
    fi
}

# ── Check if already installed ──
check_existing() {
    local dest="$1"
    local binary="$2"
    if [ -f "${dest}/${binary}" ]; then
        local old_ver
        old_ver=$("${dest}/${binary}" --version 2>/dev/null || echo "unknown")
        detail "Found existing: ${dest}/${binary} (${old_ver})"
    fi
}

# ── Install binary ──
install_binary() {
    local binary_path="$1"
    local binary_name="$2"
    local dest="$3"
    local sudo_cmd="$4"

    info "Installing: ${DIM}${binary_name} → ${dest}/${binary_name}${NC}"
    $sudo_cmd mkdir -p "$dest"
    $sudo_cmd cp -f "$binary_path" "${dest}/${binary_name}"
    $sudo_cmd chmod 755 "${dest}/${binary_name}"

    # Create /usr/bin symlink for sudo PATH (root mode)
    if [ "$binary_name" = "spm" ] && [ "$dest" = "/usr/local/bin" ] && [ -z "$sudo_cmd" ]; then
        if [ ! -L /usr/bin/spm ] || [ "$(readlink /usr/bin/spm)" != "/usr/local/bin/spm" ]; then
            $sudo_cmd ln -sf /usr/local/bin/spm /usr/bin/spm
            detail "Symlinked: /usr/bin/spm → /usr/local/bin/spm"
        fi
    fi

    ok "${binary_name} installed → ${dest}/${binary_name}"
}

# ── Cleanup ──
cleanup() {
    rm -f /tmp/spm-* /tmp/spmd-* /tmp/checksums.txt 2>/dev/null || true
}

# ── Main ──
main() {
    local user_mode=""
    local version="latest"
    local arch

    # Trap for cleanup
    trap cleanup EXIT

    # Parse args
    for arg in "$@"; do
        case "$arg" in
            --help|-h) usage ;;
            --user)    user_mode="user" ;;
            --version=*) version="${arg#*=}" ;;
            --version) ;;
        esac
    done

    # ── Welcome ──
    printf "\n"
    header "SPM Installer — v${version#v}"
    if [ -z "$user_mode" ]; then
        echo "  System-wide install  │  ${CYAN}spm${NC} + ${CYAN}spmd${NC} → /usr/local/bin"
        printf "\n"
        printf "  ${DIM}Use ${NC}--user${DIM} for user install (${NC}~/.local/bin${DIM})${NC}\n"
    else
        echo "  User install         │  ${CYAN}spm${NC} → ~/.local/bin  ·  ${CYAN}spmd${NC} → /usr/local/bin"
    fi
    printf "\n"

    # ── Step 1: Check permissions ──
    step "1 of 5 — Checking environment"
    if [ -z "$user_mode" ]; then
        if [ "$(id -u)" -ne 0 ]; then
            err "Root install requires root privileges."
            err "Re-run: ${BOLD}sudo bash install.sh${NC}"
            err "Or:     ${BOLD}bash install.sh --user${NC}"
            exit 1
        fi
        user_mode="root"
        ok "Running as root"
    else
        ok "Running as user: $(whoami)"
    fi

    # ── Step 2: Detect architecture ──
    step "2 of 5 — Detecting system"
    arch="$(detect_arch)"
    ok "Architecture: ${arch}"

    local os_info
    os_info="$(uname -sr)"
    detail "Kernel: ${os_info}"

    # ── Step 3: Download binaries ──
    step "3 of 5 — Downloading binaries"

    local bin_name="spm-${arch}"
    local bin_name_d="spmd-${arch}"

    # Download spm
    info "Binary: ${bin_name}"
    local dl_path
    dl_path=$(download_release "$version" "$bin_name")
    verify_checksum "$dl_path" "$bin_name" "$version"

    # Download spmd
    info "Binary: ${bin_name_d}"
    local dl_path_d
    dl_path_d=$(download_release "$version" "$bin_name_d")
    verify_checksum "$dl_path_d" "$bin_name_d" "$version"

    # ── Step 4: Install ──
    step "4 of 5 — Installing"
    install_paths "$user_mode"

    check_existing "$BINDIR" "spm"
    install_binary "$dl_path" "spm" "$BINDIR" "$SUDO"

    check_existing "$SPMD_BINDIR" "spmd"
    install_binary "$dl_path_d" "spmd" "$SPMD_BINDIR" "$SUDO"

    # Clean up downloaded files
    cleanup

    # ── Step 5: Post-install ──
    step "5 of 5 — Finalizing"

    if command -v spm &>/dev/null; then
        local ver
        ver=$(spm --version 2>/dev/null || echo "v${version#v}")
        ok "SPM ${ver} is ready"
    else
        detail "Run ${BOLD}hash -r${NC} or restart your shell to use spm"
    fi

    # Run spm init in background
    if $SUDO spm init &>/dev/null; then
        ok "spm init completed"
    else
        detail "spm init skipped (run manually: ${BOLD}spm init${NC})"
    fi

    # ── Summary ──
    printf "\n"
    header "Installation complete"
    echo "  ${GREEN}spm${NC}  → ${BOLD}${BINDIR}/spm${NC}"
    echo "  ${GREEN}spmd${NC} → ${BOLD}${SPMD_BINDIR}/spmd${NC}"
    printf "\n"
    echo "  ${BOLD}Next steps:${NC}"
    echo "    ${CYAN}1.${NC} ${DIM}spm repo add debian --source deb \\"
    echo "         --mirrors https://deb.debian.org/debian \\"
    echo "         --codename stable --components main${NC}"
    echo "    ${CYAN}2.${NC} ${DIM}spm update${NC}"
    echo "    ${CYAN}3.${NC} ${DIM}spm install figlet${NC}"
    printf "\n"
    echo "  ${DIM}Need help?  spm --help${NC}"
    printf "\n"
}

main "$@"

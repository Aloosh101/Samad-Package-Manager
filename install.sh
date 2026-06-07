#!/usr/bin/env bash
set -euo pipefail
#
# install.sh — Install SPM on any Linux system
#
# Usage:
#   ./install.sh                    # interactive (asks root vs user, source, version)
#   ./install.sh --root             # force root installation (interactive for source/version)
#   ./install.sh --user             # force user installation (interactive for source/version)
#   ./install.sh --root --source=apt --version=stable   # fully non-interactive
#   ./install.sh --user --source=dnf --version=newest   # fully non-interactive
#   ./install.sh --help             # show this help
#
# Root mode:
#   spm/spmd → /usr/local/bin/
#   spm      → /usr/bin/spm (symlink for sudo PATH)
#   backends → /usr/libexec/spm/backend/  (isolated from system)
#   database → /var/lib/spm/
#   daemon   → systemd service (spmd)
#   man pages → /usr/local/share/man/man8/
#
# Non-root mode (user has sudo):
#   spm      → ~/.local/bin/spm          (user binary)
#   spmd     → /usr/local/bin/spmd       (daemon — requires sudo)
#   backends → system PATH (no isolation)
#   daemon   → systemd service (installed via sudo)
#   database → /var/lib/spm/ (system-wide)
#   no man pages
#
# Interactive prompts:
#   1. Installation scope (root / user)
#   2. Preferred source  (apt / dnf / native)
#   3. Version preference (stable / newest)
#

SPM_SRC="${SPM_SRC:-$(dirname "$0")/target/debug}"
SPM_BIN="${SPM_SRC}/spm"
SPMD_BIN="${SPM_SRC}/spmd"

# Colours
BOLD='\033[1m'
RED='\033[31m'
GREEN='\033[32m'
YELLOW='\033[33m'
CYAN='\033[36m'
DIM='\033[2m'
NC='\033[0m'

info()  { printf "${CYAN}ℹ${NC} %s\n" "$*"; }
ok()    { printf "${GREEN}✔${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}⚠${NC} %s\n" "$*"; }
err()   { printf "${RED}✘${NC} %s\n" "$*"; }
header(){ printf "\n${BOLD}${CYAN}── %s ──${NC}\n" "$*"; }

usage() {
    sed -n '/^#$/q; /^#/p' "$0" | sed 's/^# //; s/^#//'
    exit 0
}

check_bins() {
    if [ ! -f "$SPM_BIN" ] || [ ! -f "$SPMD_BIN" ]; then
        err "Binaries not found at $SPM_SRC/"
        info "Build first:  cargo build --release"
        info "Or set:       SPM_SRC=/path/to/target/release"
        exit 1
    fi
}

detect_distro() {
    if command -v rpm &>/dev/null; then echo "rpm"
    elif command -v dpkg &>/dev/null; then echo "deb"
    else echo "unknown"; fi
}

install_root() {
    local pref_source="$1"
    local pref_version="$2"
    local distro
    distro=$(detect_distro)

    header "Root installation — system-wide"

    # 1. Copy binaries
    info "Copying spm → /usr/local/bin/spm"
    cp -f "$SPM_BIN" /usr/local/bin/spm
    chmod 755 /usr/local/bin/spm

    info "Copying spmd → /usr/local/bin/spmd"
    cp -f "$SPMD_BIN" /usr/local/bin/spmd
    chmod 755 /usr/local/bin/spmd
    ok "Binaries installed"

    # 1b. Symlink in /usr/bin so sudo finds spm
    if [ ! -L /usr/bin/spm ] || [ "$(readlink /usr/bin/spm)" != "/usr/local/bin/spm" ]; then
        ln -sf /usr/local/bin/spm /usr/bin/spm
        ok "Symlink /usr/bin/spm → /usr/local/bin/spm (for sudo)"
    fi

    # 2. Backends
    info "Installing backends..."
    local bk="${SPM_BACKEND_DIR:-/usr/libexec/spm/backend}"
    mkdir -p "$bk"
    case "$distro" in
        rpm) install_rpm_backends "$bk" ;;
        deb) install_deb_backends "$bk" ;;
        *)   install_rpm_backends "$bk"; install_deb_backends "$bk" ;;
    esac
    ok "Backends installed in $bk"

    # 3. spm init
    info "Running: spm init --fix-backend --from-system"
    /usr/local/bin/spm init --fix-backend --from-system || warn "spm init --from-system failed (non-root? no system package manager?)"
    ok "Database initialized, system packages imported"

    # 4. Apply user preferences
    info "Setting preferences..."
    if [ "$pref_source" = "apt" ]; then
        /usr/local/bin/spm config set preferred_source apt 2>/dev/null || true
    elif [ "$pref_source" = "dnf" ]; then
        /usr/local/bin/spm config set preferred_source dnf 2>/dev/null || true
    fi
    if [ "$pref_version" = "newest" ]; then
        /usr/local/bin/spm config set prefer_newest true 2>/dev/null || true
    fi
    ok "Preferences saved"

    # 5. Daemon service
    info "Running: spm init --install-daemon"
    /usr/local/bin/spm init --install-daemon || warn "spm init --install-daemon failed (no systemd?)"

    # 6. Man pages
    local mandir="/usr/local/share/man/man8"
    if [ -d "$(dirname "$0")/docs/man" ]; then
        mkdir -p "$mandir"
        for man in "$(dirname "$0")/docs/man"/*.8; do
            cp -f "$man" "$mandir/"
            ok "Man page: $(basename "$man")"
        done
    fi

    header "SPM installed system-wide"
    echo "  spm  → /usr/local/bin/spm"
    echo "  spmd → /usr/local/bin/spmd"
    echo "  man  → $mandir"
    echo "  source → $pref_source, version → $pref_version"
    spm --version
}

install_user() {
    local pref_source="$1"
    local pref_version="$2"
    local bindir="${HOME}/.local/bin"

    header "User installation — ~/.local (daemon requires sudo)"

    mkdir -p "$bindir"

    info "Copying spm → $bindir/spm"
    cp -f "$SPM_BIN" "$bindir/spm"
    chmod 755 "$bindir/spm"
    ok "spm installed"

    # Ensure ~/.local/bin is in PATH
    case ":$PATH:" in
        *":$bindir:"*) ;;
        *) warn "$bindir is not in PATH. Add to ~/.bashrc:"
           echo "  export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
    esac

    # Backends — not isolated, use system PATH
    warn "Non-root: backends stay on system PATH (not isolated)"

    # Daemon is required — install system-wide via sudo
    if command -v sudo &>/dev/null; then
        info "Installing spmd daemon (requires sudo)..."
        sudo cp -f "$SPMD_BIN" /usr/local/bin/spmd
        sudo chmod 755 /usr/local/bin/spmd
        sudo /usr/local/bin/spm init --fix-backend --from-system 2>/dev/null || \
            warn "spm init --from-system failed"
        sudo /usr/local/bin/spm init --install-daemon 2>/dev/null || \
            warn "Daemon installation failed (no systemd?). spm requires spmd running."
        # Apply preferences via sudo (config goes to user's home)
        if [ "$pref_source" = "apt" ]; then
            /usr/local/bin/spm config set preferred_source apt 2>/dev/null || true
        elif [ "$pref_source" = "dnf" ]; then
            /usr/local/bin/spm config set preferred_source dnf 2>/dev/null || true
        fi
        if [ "$pref_version" = "newest" ]; then
            /usr/local/bin/spm config set prefer_newest true 2>/dev/null || true
        fi
        ok "Preferences saved"
    else
        warn "sudo not available — cannot install daemon."
        warn "spm requires spmd. Install it manually:"
        warn "  # cp spmd /usr/local/bin/spmd && spmd &"
        exit 1
    fi

    header "SPM installed for current user"
    echo "  spm     → $bindir/spm"
    echo "  daemon  → /usr/local/bin/spmd (systemd service)"
    echo "  source  → $pref_source, version → $pref_version"
    echo ""
    echo "Usage:  spm install <package>"
}

# ── Backend install helpers (duplicated from install-backends.sh for self-containment) ──

install_backend() {
    local name="$1" src="$2" dst="$3/$name"
    if [ ! -f "$src" ] && [ ! -L "$src" ]; then
        echo "  ⚠ $name: not found at $src, skipping"
        return 1
    fi
    mkdir -p "$3"
    rm -f "$dst"
    cp -fL "$src" "$dst"
    chmod 755 "$dst"
    echo "  ✓ $name → $dst"
}

install_rpm_backends() {
    local d="$1"
    echo "  RPM backends:"
    install_backend "dnf"      "/usr/bin/dnf"       "$d" || true
    install_backend "dnf"      "/usr/bin/dnf-3"     "$d" || true
    install_backend "rpm"      "/usr/bin/rpm"       "$d"
    install_backend "rpm2cpio" "/usr/bin/rpm2cpio"  "$d" || true
    install_backend "cpio"     "/usr/bin/cpio"      "$d" || true
    if [ ! -f "$d/dnf" ] && [ -f "$d/dnf-3" ]; then
        ln -sf dnf-3 "$d/dnf"
    fi
    local r2c="$d/rpm2cpio"
    rm -f "$r2c" "$d/rpm2archive"
    if command -v rpm2archive &>/dev/null && rpm2archive --help 2>&1 | grep -q '\-\-format'; then
        cp -fL "$(command -v rpm2archive)" "$d/rpm2archive"
        chmod 755 "$d/rpm2archive"
        printf '%s\n' \
            '#!/usr/bin/env bash' \
            '# Wrapper: rpm2cpio via rpm2archive --format=cpio' \
            'exec rpm2archive --format=cpio "$1"' > "$r2c"
        chmod 755 "$r2c"
        echo "  ✓ rpm2cpio → wrapper (rpm2archive --format=cpio)"
    fi
}

install_deb_backends() {
    local d="$1"
    echo "  DEB backends:"
    install_backend "apt-get"   "/usr/bin/apt-get"   "$d" || true
    install_backend "apt-cache" "/usr/bin/apt-cache" "$d" || true
    install_backend "dpkg-deb"  "/usr/bin/dpkg-deb"  "$d" || true
    install_backend "dpkg"      "/usr/bin/dpkg"      "$d" || true
}

# ── Main ──

main() {
    local mode=""
    local pref_source=""
    local pref_version=""

    for arg in "$@"; do
        case "$arg" in
            --help|-h) usage ;;
            --root)    mode="root" ;;
            --user)    mode="user" ;;
            --source=*) pref_source="${arg#*=}" ;;
            --version=*) pref_version="${arg#*=}" ;;
        esac
    done

    # Check that we can find the built binaries
    check_bins

    if [ -z "$mode" ]; then
        echo ""
        echo "${BOLD}SPM Installer${NC}"
        echo "This will install SPM (Samad Package Manager) on your system."
        echo ""

        # ── Installation scope ──
        echo "${BOLD}Installation scope:${NC}"
        echo "  1) System-wide  — spm + spmd + daemon + man pages (requires root)"
        echo "  2) User install  — spm → ~/.local/bin, daemon system-wide (requires sudo)"
        echo ""
        read -r -p "Choose [1/2] (default: 1): " choice
        case "$choice" in
            2|user) mode="user" ;;
            *)      mode="root" ;;
        esac
    fi

    # ── Distro preference ──
    echo ""
    echo "${BOLD}Preferred package source:${NC}"
    echo "  SPM supports both Debian/APT and RedHat/DNF packages."
    echo "  If the preferred source fails, it auto-falls back to the other."
    echo ""
    echo "  1) Debian/APT — stable, wide selection (default)"
    echo "  2) RedHat/DNF — newer packages, RPM ecosystem"
    echo ""
    read -r -p "Choose [1/2] (default: 1): " src_choice
    case "$src_choice" in
        2|dnf) pref_source="dnf" ;;
        *)     pref_source="apt" ;;
    esac

    # ── Version preference ──
    echo ""
    echo "${BOLD}Version preference:${NC}"
    echo "  1) Stable   — prefer well-tested releases (default)"
    echo "  2) Newest   — bleeding edge, latest versions"
    echo ""
    read -r -p "Choose [1/2] (default: 1): " ver_choice
    case "$ver_choice" in
        2|newest) pref_version="newest" ;;
        *)        pref_version="stable" ;;
    esac

    case "$mode" in
        root)
            if [ "$(id -u)" -ne 0 ]; then
                echo ""
                warn "Root installation requires root privileges."
                echo "Re-run as:  sudo $0 --root"
                echo "Or choose user installation instead."
                exit 1
            fi
            install_root "$pref_source" "$pref_version"
            ;;
        user)
            install_user "$pref_source" "$pref_version"
            ;;
    esac
}

main "$@"

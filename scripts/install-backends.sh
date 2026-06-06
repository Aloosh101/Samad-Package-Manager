#!/usr/bin/env bash
set -euo pipefail
#
# install-backends.sh — Populate /usr/libexec/spm/backend/ from the running system
#
# This script copies the backend binaries that SPM needs from the system's
# package manager into the SPM bundled backend directory.  Run this once
# after installing SPM on a fresh AlmaLinux/Rocky/Debian/Ubuntu system.
#
# After this script completes:
#   spm init --fix-backend   # copies backends into the store
#   rpm -e dnf               # (optional) remove system dnf
#   spm install nginx        # works using store-managed backend
#
# For Samad OS kickstart, this runs automatically in %post.

SPM_BACKEND_DIR="${SPM_BACKEND_DIR:-/usr/libexec/spm/backend}"

# Detect distribution type — RPM first because dpkg can exist on RPM systems
detect_distro() {
    if command -v rpm &>/dev/null; then
        echo "rpm"
    elif command -v dpkg &>/dev/null; then
        echo "deb"
    else
        echo "unknown"
    fi
}

install_backend() {
    local name="$1"
    local src="$2"
    local dst="$SPM_BACKEND_DIR/$name"

    if [ ! -f "$src" ] && [ ! -L "$src" ]; then
        echo "  ⚠ $name: not found at $src, skipping"
        return 1
    fi

    mkdir -p "$SPM_BACKEND_DIR"
    # Remove any existing symlink or file at destination first
    rm -f "$dst"
    cp -fL "$src" "$dst"
    chmod 755 "$dst"
    echo "  ✓ $name → $dst"
}

install_rpm_backends() {
    echo "Installing RPM/DNF backends..."

    install_backend "dnf"       "/usr/bin/dnf"       || true
    install_backend "dnf"       "/usr/bin/dnf-3"     || true  # RHEL9+ / openSUSE alternative
    install_backend "rpm"       "/usr/bin/rpm"
    install_backend "rpm2cpio"  "/usr/bin/rpm2cpio"  || true
    install_backend "cpio"      "/usr/bin/cpio"      || true

    # On AlmaLinux/Rocky/RHEL 9, dnf-3 might be the real binary
    if [ ! -f "$SPM_BACKEND_DIR/dnf" ] && [ -f "$SPM_BACKEND_DIR/dnf-3" ]; then
        ln -sf dnf-3 "$SPM_BACKEND_DIR/dnf"
        echo "  ✓ dnf → symlink to dnf-3"
    fi

    # openSUSE: rpm2cpio is a symlink to rpm2archive — write a proper wrapper
    local r2c="$SPM_BACKEND_DIR/rpm2cpio"
    rm -f "$r2c" "$SPM_BACKEND_DIR/rpm2archive"
    if command -v rpm2archive &>/dev/null && rpm2archive --help 2>&1 | grep -q '\-\-format'; then
        cp -fL "$(command -v rpm2archive)" "$SPM_BACKEND_DIR/rpm2archive"
        chmod 755 "$SPM_BACKEND_DIR/rpm2archive"
        echo "  ✓ rpm2archive → $SPM_BACKEND_DIR/rpm2archive"
        # Wrapper: rpm2cpio <rpm> → cpio on stdout (via rpm2archive --format=cpio)
        printf '%s\n' \
            '#!/usr/bin/env bash' \
            '# Wrapper: rpm2cpio via rpm2archive --format=cpio' \
            'exec rpm2archive --format=cpio "$1"' > "$r2c"
        chmod 755 "$r2c"
        echo "  ✓ rpm2cpio → wrapper (rpm2archive --format=cpio)"
    else
        echo "  ⚠ rpm2cpio: cannot create wrapper (rpm2archive not found/too old)"
    fi

    # Try extra locations for cpio
    if [ ! -f "$SPM_BACKEND_DIR/cpio" ]; then
        install_backend "cpio" "/usr/lib/cpio/cpio" || true
    fi
}

install_deb_backends() {
    echo "Installing DEB/APT backends..."

    install_backend "apt-get"    "/usr/bin/apt-get"
    install_backend "apt-cache"  "/usr/bin/apt-cache"
    install_backend "dpkg-deb"   "/usr/bin/dpkg-deb"
    install_backend "dpkg"       "/usr/bin/dpkg"
}

main() {
    mkdir -p "$SPM_BACKEND_DIR"
    echo "SPM backend directory: $SPM_BACKEND_DIR"

    local distro
    distro=$(detect_distro)
    echo "Detected distribution type: $distro"

    case "$distro" in
        rpm)
            install_rpm_backends
            ;;
        deb)
            install_deb_backends
            ;;
        *)
            echo "Unknown distribution. Trying all backends..."
            install_rpm_backends
            install_deb_backends
            ;;
    esac

    local count
    count=$(find "$SPM_BACKEND_DIR" -type f -o -type l | wc -l)
    echo ""
    echo "Done. $count backend(s) installed in $SPM_BACKEND_DIR"
    echo "Next step: sudo spm init --fix-backend"
}

main

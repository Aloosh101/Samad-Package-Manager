#!/usr/bin/env bash
set -euo pipefail

# ═══════════════════════════════════════════════════════════════════
# install.sh — SPM binary installer
#   curl -fsSL https://github.com/Aloosh101/Samad-Package-Manager/releases/latest/download/install.sh | sudo bash
#
# Options:
#   --user          Install to ~/.local/bin
#   --version TAG   Install specific version (default: latest)
#   --help          Show help
# ═══════════════════════════════════════════════════════════════════

# ── ANSI (use $'...' so these contain actual ESC bytes, not literal \033) ──
BOLD=$'\033[1m'
RED=$'\033[31m'
GREEN=$'\033[32m'
YELLOW=$'\033[33m'
CYAN=$'\033[36m'
BLUE=$'\033[34m'
MAGENTA=$'\033[35m'
DIM=$'\033[2m'
NC=$'\033[0m'

# All helpers write to stderr so command substitution never captures them.
info()  { printf "${DIM}  • %s${NC}\n" "$*" >&2; }
ok()    { printf "  ${GREEN}✔${NC} %s\n" "$*" >&2; }
warn()  { printf "  ${YELLOW}⚠${NC} %s\n" "$*" >&2; }
err()   { printf "  ${RED}✘${NC} %s\n" "$*" >&2; }
detail(){ printf "${DIM}    %s${NC}\n" "$*" >&2; }

block() {
  local title="$1"; shift
  printf "${BLUE}┌─ ${title}${NC}\n" >&2
  for line in "$@"; do
    printf "${BLUE}│${NC}  ${line}\n" >&2
  done
  printf "${BLUE}└─${NC}\n" >&2
}

step() {
  printf "\n${MAGENTA}┃ Step ${1}/5 ┃${NC} ${BOLD}${2}${NC}\n" >&2
  printf "${MAGENTA}┃${NC}\n" >&2
}

# ── Parse arguments ──
USER_MODE=""
VERSION="latest"
for arg in "$@"; do
  case "$arg" in
    --help|-h)
      sed -n '5,11p' "$0" | sed 's/^# \?//'
      exit 0 ;;
    --user)    USER_MODE="user" ;;
    --version=*) VERSION="${arg#*=}" ;;
  esac
done

trap 'rm -f /tmp/spm-* /tmp/spmd-* /tmp/checksums.txt 2>/dev/null; printf "\n" >&2' EXIT

# ── Detect architecture ──
detect_arch() {
  local arch; arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64)  echo "x86_64-linux-gnu" ;;
    aarch64|arm64) echo "aarch64-linux-gnu" ;;
    armv7l|armv7)  echo "armv7-linux-gnueabihf" ;;
    i686|i386)     echo "i686-linux-gnu" ;;
    riscv64)       echo "riscv64-linux-gnu" ;;
    *) err "unsupported architecture: ${arch}"; exit 1 ;;
  esac
}

# ── Detect distro family ──
detect_distro() {
  local id like
  if [ -f /etc/os-release ]; then
    id=$(source /etc/os-release && echo "${ID}")
    like=$(source /etc/os-release && echo "${ID_LIKE}")
  fi
  case "${id}-${like}" in
    *debian*|*ubuntu*|*mint*)      echo "debian" ;;
    *fedora*|*rhel*|*centos*|*)    echo "fedora" ;;
    *opensuse*|*suse*)             echo "suse" ;;
    *arch*)                        echo "arch" ;;
    *)                             echo "other" ;;
  esac
}

# ── Welcome banner ──
printf "\n${MAGENTA}"
printf "  ┌────────────────────────────┐\n"
printf "  │  ${BOLD}SPM — Samad Package Mgr${NC}${MAGENTA}   │\n"
printf "  │  Installer v${VERSION#v}${MAGENTA}            │\n"
printf "  │  ${NC}${DIM}github.com/Aloosh101/...${NC}${MAGENTA}   │\n"
printf "  └────────────────────────────┘${NC}\n" >&2

if [ -z "$USER_MODE" ]; then
  block "System-wide install" \
    "spm + spmd  →  /usr/local/bin" \
    "Use ${BOLD}--user${NC} for ~/.local/bin"
else
  block "User install" \
    "spm  →  ${HOME}/.local/bin" \
    "spmd →  /usr/local/bin  (needs sudo)"
fi

# ═══════════════════════════════════════════════════════════════════
# Step 1 — Environment
# ═══════════════════════════════════════════════════════════════════
step 1 "Environment"

if [ -z "$USER_MODE" ]; then
  if [ "$(id -u)" -ne 0 ]; then
    err "root required for system-wide install"
    err "re-run: ${BOLD}sudo bash install.sh${NC}"
    err "or:    ${BOLD}bash install.sh --user${NC}"
    exit 1
  fi
  ok "root"
else
  ok "user: $(whoami)"
fi

if command -v curl &>/dev/null; then
  ok "curl"
elif command -v wget &>/dev/null; then
  ok "wget"
else
  err "need curl or wget"
  exit 1
fi

# ═══════════════════════════════════════════════════════════════════
# Step 2 — System
# ═══════════════════════════════════════════════════════════════════
step 2 "System"

ARCH="$(detect_arch)"
ok "architecture: ${BOLD}${ARCH}${NC}"

DISTRO_FAMILY="$(detect_distro)"
DISTRO="$( (source /etc/os-release 2>/dev/null && echo "${ID}") || echo "linux")"
KERNEL="$(uname -sr)"
info "distro: ${DISTRO}  ·  kernel: ${KERNEL}"

# ═══════════════════════════════════════════════════════════════════
# Step 3 — Download
# ═══════════════════════════════════════════════════════════════════
step 3 "Download"

GH="https://github.com/Aloosh101/Samad-Package-Manager/releases"
if [ "$VERSION" = "latest" ]; then
  BASE="${GH}/latest/download"
else
  BASE="${GH}/download/${VERSION}"
fi

download() {
  local name="$1"
  local dest="/tmp/${name}"
  local url="${BASE}/${name}"

  detail "source: ${url}"

  local size_hint
  size_hint=$(curl -sI "$url" 2>/dev/null \
    | grep -i '^content-length:' \
    | awk '{printf "%.1f MB", $2/1048576}')
  if [ -n "$size_hint" ]; then
    detail "size: ${CYAN}${size_hint}${NC}"
  fi

  if command -v curl &>/dev/null; then
    curl -fL --progress-bar "$url" -o "$dest"
  else
    wget -q --show-progress "$url" -O "$dest"
  fi

  if [ ! -s "$dest" ]; then
    err "download failed: ${name}"
    exit 1
  fi

  local file_size
  file_size=$(stat -c%s "$dest" 2>/dev/null || stat -f%z "$dest" 2>/dev/null || echo 0)
  if [ "$file_size" -gt 0 ]; then
    local size_display
    size_display=$(echo "$file_size" | awk '{printf "%.1f MB", $1/1048576}')
    ok "${name}  ${DIM}(${size_display})${NC}"
  else
    ok "${name}"
  fi

  chmod +x "$dest"
  printf "%s" "$dest"
}

verify_checksum() {
  local bin_path="$1" bin_name="$2"
  if ! command -v sha256sum &>/dev/null; then
    detail "sha256sum not available — skipping"
    return
  fi
  local checksums_url="${BASE}/checksums.txt"
  local expected
  if curl -fsSL "$checksums_url" -o /tmp/checksums.txt 2>/dev/null; then
    expected=$(awk -v name="  ${bin_name}$" '$0 ~ name {print $1}' /tmp/checksums.txt)
    if [ -n "$expected" ]; then
      local actual
      actual=$(sha256sum "$bin_path" | awk '{print $1}')
      if [ "$expected" != "$actual" ]; then
        err "checksum mismatch for ${bin_name}"
        detail "expected: ${expected}"
        detail "actual:   ${actual}"
        exit 1
      fi
      ok "${bin_name} checksum verified"
    else
      detail "no checksum entry for ${bin_name}"
    fi
  else
    detail "could not fetch checksums"
  fi
  rm -f /tmp/checksums.txt
}

DL_SPM=$(download "spm-${ARCH}")
verify_checksum "$DL_SPM" "spm-${ARCH}"

DL_SPMD=$(download "spmd-${ARCH}")
verify_checksum "$DL_SPMD" "spmd-${ARCH}"

# ═══════════════════════════════════════════════════════════════════
# Step 4 — Install
# ═══════════════════════════════════════════════════════════════════
step 4 "Install"

if [ -z "$USER_MODE" ]; then
  BINDIR="/usr/local/bin"
  SUDO=""
else
  BINDIR="${HOME}/.local/bin"
  SUDO="sudo"
fi

install_bin() {
  local src="$1" name="$2" dest="$3" sudo_cmd="$4"
  if [ -f "${dest}/${name}" ]; then
    local old_ver
    old_ver=$("${dest}/${name}" --version 2>/dev/null || echo "unknown")
    detail "replacing: ${dest}/${name} (${old_ver})"
  fi
  ${sudo_cmd} mkdir -p "$dest"
  ${sudo_cmd} cp -f "$src" "${dest}/${name}"
  ${sudo_cmd} chmod 755 "${dest}/${name}"
  ok "${BOLD}${name}${NC}  →  ${CYAN}${dest}/${name}${NC}"
}

install_bin "$DL_SPM"  "spm"  "$BINDIR" "$SUDO"
install_bin "$DL_SPMD" "spmd" "/usr/local/bin" "$SUDO"

# Symlink for sudo PATH (root mode)
if [ -z "$USER_MODE" ] && [ ! -L /usr/bin/spm ]; then
  ln -sf /usr/local/bin/spm /usr/bin/spm 2>/dev/null \
    && detail "symlink: /usr/bin/spm → /usr/local/bin/spm" \
    || true
fi

# ═══════════════════════════════════════════════════════════════════
# Step 5 — Finalise
# ═══════════════════════════════════════════════════════════════════
step 5 "Finalise"

VER="v${VERSION#v}"
if command -v spm &>/dev/null; then
  VER=$(spm --version 2>/dev/null || echo "$VER")
fi

if ${SUDO} spm init &>/dev/null; then
  ok "spm init"
else
  detail "run ${BOLD}spm init${NC} manually"
fi

# ── Auto-configure standard repos ──
setup_repos() {
  printf "\n${BLUE}┌─ Repository setup${NC}\n" >&2
  printf "${BLUE}│${NC}\n" >&2

  case "$DISTRO_FAMILY" in
    debian)
      printf "${BLUE}│${NC}  Adding Debian repository...\n" >&2
      ${SUDO} spm repo add debian --source deb --mirror http://deb.debian.org/debian >/dev/null 2>&1 \
        && ok "Debian repo added" \
        || detail "Debian repo skipped"
      ;;
    fedora)
      printf "${BLUE}│${NC}  Adding Fedora repository...\n" >&2
      ${SUDO} spm repo add fedora --source rpm --mirror https://mirrors.kernel.org/fedora >/dev/null 2>&1 \
        && ok "Fedora repo added" \
        || detail "Fedora repo skipped"
      ;;
    suse)
      printf "${BLUE}│${NC}  Adding openSUSE repository...\n" >&2
      ${SUDO} spm repo add opensuse --source rpm --mirror https://mirror.opensuse.org >/dev/null 2>&1 \
        && ok "openSUSE repo added" \
        || detail "openSUSE repo skipped"
      ;;
    arch)
      printf "${BLUE}│${NC}  Adding Arch Linux repository...\n" >&2
      ${SUDO} spm repo add arch --source native --mirror https://mirror.archlinux.org >/dev/null 2>&1 \
        && ok "Arch repo added" \
        || detail "Arch repo skipped"
      ;;
    *)
      printf "${BLUE}│${NC}  ${DIM}unknown distro — add repos manually with:${NC}\n" >&2
      printf "${BLUE}│${NC}  ${CYAN}spm repo add <name> --source <deb|rpm|native> --mirror <url>${NC}\n" >&2
      ;;
  esac

  # Ask about closed-source repos
  printf "${BLUE}│${NC}\n" >&2
  printf "${BLUE}│${NC}  ${BOLD}Add closed-source repositories?${NC} ${DIM}(non-free, rpmfusion, etc.)${NC}\n" >&2
  printf "${BLUE}│${NC}  ${DIM}[y/N]${NC} " >&2
  read -r yn </dev/tty || yn="n"
  case "${yn:-n}" in
    y|Y|yes|YES)
      case "$DISTRO_FAMILY" in
        debian)
          ${SUDO} spm repo add debian-nonfree --source deb --mirror http://deb.debian.org/debian >/dev/null 2>&1 \
            && ok "Debian non-free added" \
            || detail "non-free skipped"
          ;;
        fedora)
          ${SUDO} spm repo add rpmfusion-nonfree --source rpm --mirror https://mirrors.rpmfusion.org >/dev/null 2>&1 \
            && ok "RPM Fusion non-free added" \
            || detail "RPM Fusion skipped"
          ;;
      esac
      ;;
  esac

  printf "${BLUE}└─${NC}\n" >&2
}

setup_repos

# ── Summary ──
printf "\n${GREEN}"
printf "  ┌────────────────────────────┐\n"
printf "  │  ${BOLD}SPM ${VER} installed${NC}${GREEN}        │\n"
printf "  └────────────────────────────┘${NC}\n" >&2
printf "\n" >&2
block "Next steps" \
  "${CYAN}spm update${NC}" \
  "${CYAN}spm search <package>${NC}" \
  "${CYAN}spm install <package>${NC}"
printf "\n${DIM}  Need help?  spm --help${NC}\n" >&2
printf "\n" >&2

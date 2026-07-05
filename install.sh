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

BOLD='\033[1m'
RED='\033[31m'
GREEN='\033[32m'
YELLOW='\033[33m'
CYAN='\033[36m'
BLUE='\033[34m'
MAGENTA='\033[35m'
DIM='\033[2m'
NC='\033[0m'

# All formatted output helpers write to stderr (>&2) so they never
# get captured by command substitutions like DL="$(download ...)".
info()  { printf "${DIM}  • %s${NC}\n" "$*" >&2; }
ok()    { printf "  ${GREEN}✔${NC} %s\n" "$*" >&2; }
warn()  { printf "  ${YELLOW}⚠${NC} %s\n" "$*" >&2; }
err()   { printf "  ${RED}✘${NC} %s\n" "$*" >&2; }
detail(){ printf "${DIM}    %s${NC}\n" "$*" >&2; }

block() {
  local title="$1"; shift
  printf "${BOLD}${BLUE}┌─ ${title}${NC}\n" >&2
  for line in "$@"; do
    printf "${BOLD}${BLUE}│${NC}  ${line}\n" >&2
  done
  printf "${BOLD}${BLUE}└─${NC}\n" >&2
}

step() {
  printf "\n${BOLD}${MAGENTA}┃ Step ${1}/5 ┃${NC} ${BOLD}${2}${NC}\n" >&2
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

# ── Welcome banner ──
printf "\n${BOLD}${MAGENTA}" >&2
printf "  ╔══════════════════════════════╗\n" >&2
printf "  ║   SPM — Samad Package Mgr    ║\n" >&2
printf "  ║   Installer v${VERSION#v}${NC}${BOLD}${MAGENTA}            ║\n" >&2
printf "  ║   ${NC}${DIM}github.com/Aloosh101/...${NC}${BOLD}${MAGENTA}   ║\n" >&2
printf "  ╚══════════════════════════════╝${NC}\n" >&2

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
# Step 1 — Environment checks
# ═══════════════════════════════════════════════════════════════════
step 1 "Environment"

if [ -z "$USER_MODE" ]; then
  if [ "$(id -u)" -ne 0 ]; then
    err "root required for system-wide install"
    err "re-run: ${BOLD}sudo bash install.sh${NC}"
    err "or:    ${BOLD}bash install.sh --user${NC}"
    exit 1
  fi
  ok "root privileges confirmed"
else
  ok "user: $(whoami)"
fi

if command -v curl &>/dev/null; then
  ok "curl available"
elif command -v wget &>/dev/null; then
  ok "wget available"
else
  err "need curl or wget — install one and re-run"
  exit 1
fi

# ═══════════════════════════════════════════════════════════════════
# Step 2 — System detection
# ═══════════════════════════════════════════════════════════════════
step 2 "System"

ARCH="$(detect_arch)"
ok "architecture: ${BOLD}${ARCH}${NC}"

DISTRO="$( (source /etc/os-release 2>/dev/null && echo "${ID}") || echo "linux")"
KERNEL="$(uname -sr)"
info "distro: ${DISTRO}  ·  kernel: ${KERNEL}"

# ═══════════════════════════════════════════════════════════════════
# Step 3 — Download binaries
# ═══════════════════════════════════════════════════════════════════
step 3 "Download"

GH="https://github.com/Aloosh101/Samad-Package-Manager/releases"
if [ "$VERSION" = "latest" ]; then
  BASE="${GH}/latest/download"
else
  BASE="${GH}/download/${VERSION}"
fi

# download name → prints dest path to stdout; all diagnostics to stderr
download() {
  local name="$1"
  local dest="/tmp/${name}"
  local url="${BASE}/${name}"

  detail "source: ${DIM}${url}${NC}"

  # Fetch content-length for size hint
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
    ok "${name} downloaded  ${DIM}(${size_display})${NC}"
  else
    ok "${name} downloaded"
  fi

  chmod +x "$dest"
  printf "%s" "$dest"
}

# ── Checksum verification ──
verify_checksum() {
  local bin_path="$1" bin_name="$2"
  if ! command -v sha256sum &>/dev/null; then
    detail "sha256sum not available — skipping verification"
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
      detail "no checksum entry for ${bin_name} — skipping"
    fi
  else
    detail "could not fetch checksums — skipping"
  fi
  rm -f /tmp/checksums.txt
}

BIN_NAME="spm-${ARCH}"
BIN_DNAME="spmd-${ARCH}"

DL_SPM=$(download "$BIN_NAME")
verify_checksum "$DL_SPM" "$BIN_NAME"

DL_SPMD=$(download "$BIN_DNAME")
verify_checksum "$DL_SPMD" "$BIN_DNAME"

# ═══════════════════════════════════════════════════════════════════
# Step 4 — Install binaries
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
    detail "replacing existing: ${dest}/${name} (${old_ver})"
  fi
  ${sudo_cmd} mkdir -p "$dest"
  ${sudo_cmd} cp -f "$src" "${dest}/${name}"
  ${sudo_cmd} chmod 755 "${dest}/${name}"
  ok "${BOLD}${name}${NC} → ${CYAN}${dest}/${name}${NC}"
}

install_bin "$DL_SPM"  "spm"  "$BINDIR" "$SUDO"
install_bin "$DL_SPMD" "spmd" "/usr/local/bin" "$SUDO"

# Symlink /usr/bin/spm for sudo PATH (root mode only)
if [ -z "$USER_MODE" ] && [ ! -L /usr/bin/spm ]; then
  ln -sf /usr/local/bin/spm /usr/bin/spm 2>/dev/null \
    && detail "symlink: /usr/bin/spm → /usr/local/bin/spm" \
    || true
fi

# ═══════════════════════════════════════════════════════════════════
# Step 5 — Finalize
# ═══════════════════════════════════════════════════════════════════
step 5 "Finalize"

VER="v${VERSION#v}"
if command -v spm &>/dev/null; then
  VER=$(spm --version 2>/dev/null || echo "$VER")
fi

if ${SUDO} spm init &>/dev/null; then
  ok "spm init completed"
else
  detail "spm init skipped — run ${BOLD}spm init${NC} manually"
fi

# ═══════════════════════════════════════════════════════════════════
printf "\n${BOLD}${GREEN}" >&2
printf "  ╔══════════════════════════════╗\n" >&2
printf "  ║   SPM ${VER} installed!       ║\n" >&2
printf "  ╚══════════════════════════════╝${NC}\n" >&2
printf "\n" >&2
block "Next steps" \
  "${CYAN}spm repo add debian --source deb --mirrors https://deb.debian.org/debian --codename stable --components main${NC}" \
  "${CYAN}spm update${NC}" \
  "${CYAN}spm install figlet${NC}"
printf "\n" >&2
printf "${DIM}  Need help?  spm --help${NC}\n" >&2
printf "\n" >&2

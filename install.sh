#!/usr/bin/env bash
# vibetty installer: downloads a prebuilt binary from GitHub Releases into ~/.cargo/bin.
#
# Usage:
#   ./install.sh              # latest stable release
#   ./install.sh v0.4.1-rc.1  # a specific release tag (e.g. a prerelease)
#
# Override the install dir with VIBETTY_INSTALL_DIR (default: ~/.cargo/bin).
# Set VIBETTY_FALLBACK_BUILD=1 to build from the local checkout with
# `cargo install --path .` when the download fails (used by the Herdr
# plugin build; off by default for end users).
set -euo pipefail

REPO="second-state/vibetty"
VERSION="${1:-latest}"
INSTALL_DIR="${VIBETTY_INSTALL_DIR:-$HOME/.cargo/bin}"
FALLBACK_BUILD="${VIBETTY_FALLBACK_BUILD:-0}"

# 下载失败时的兜底:开关打开 && 本地是 vibetty checkout(有 Cargo.toml)→ 就地 cargo install。
fallback_build() {
  if [ "$FALLBACK_BUILD" = "1" ] && [ -f Cargo.toml ]; then
    echo "download failed; falling back to cargo install --path ." >&2
    exec cargo install --force --path .
  fi
  return 1
}

# --- detect platform -> release asset name (must match .github/workflows/release.yml) ---
OS="$(uname -s)"
ARCH="$(uname -m)"
ASSET=""
NAME="vibetty"
case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64 | amd64) ASSET="vibetty-linux-x64" ;;
      *)
        echo "error: unsupported Linux arch: $ARCH (prebuilts: x86_64 only)" >&2
        exit 1
        ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64 | aarch64) ASSET="vibetty-macos-arm64" ;;
      *)
        echo "error: unsupported macOS arch: $ARCH (prebuilts: arm64 only)" >&2
        exit 1
        ;;
    esac
    ;;
  MINGW* | MSYS* | CYGWIN*)
    # Windows 走独立的安装方式(PowerShell 等后续再做);Git Bash 下 ln 是 copy,
    # 覆盖正在运行的 exe 会有占用问题,这里不装。
    echo "error: Windows is not supported by install.sh yet — download vibetty-windows-x64.exe from:" >&2
    echo "  https://github.com/${REPO}/releases/latest" >&2
    exit 1
    ;;
  *)
    echo "error: unsupported OS: $OS" >&2
    exit 1
    ;;
esac

# --- download url (releases/latest excludes prereleases; pass a tag for those) ---
if [ "$VERSION" = "latest" ]; then
  URL="https://github.com/${REPO}/releases/latest/download/${ASSET}"
else
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"
fi

command -v curl >/dev/null 2>&1 || {
  echo "error: curl is required" >&2
  exit 1
}

mkdir -p "$INSTALL_DIR"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

echo "Downloading $URL"
if ! curl -fL --progress-bar -o "$TMP" "$URL"; then
  fallback_build || { echo "error: download failed" >&2; exit 1; }
fi
chmod 755 "$TMP"

# 版本号直接问二进制(--version 输出如 "vibetty 0.4.0")。
BINVER="$("$TMP" --version 2>/dev/null | awk '{print $2}')"
if [ -z "$BINVER" ]; then
  echo "error: could not read version from the downloaded binary" >&2
  exit 1
fi

# 带版本号命名(如 vibetty-0.4.0 / vibetty-0.4.0.exe),再软链 vibetty -> 它。
# 升级 = 下新版本 + 改软链;避免直接 cp 覆盖正在运行的二进制导致新文件不可用
# (Unix 下覆盖运行中的文件用 mv/ln 原子替换,Windows 下运行中的 exe 无法覆盖)。
VERSIONED="${NAME%-*}-$BINVER" # NAME 目前恒为 vibetty(.exe),写成这样防御未来扩展

# 已装同一版本且软链完好 → 无事可做,提前退出(刷新软链也无妨,但保持输出干净)。
if [ -x "$INSTALL_DIR/$VERSIONED" ] && [ "$(readlink "$INSTALL_DIR/$NAME" 2>/dev/null)" = "$INSTALL_DIR/$VERSIONED" ]; then
  echo "vibetty $BINVER is already installed at $INSTALL_DIR/$VERSIONED — nothing to do"
  exit 0
fi

mv "$TMP" "$INSTALL_DIR/$VERSIONED"
trap - EXIT
ln -sfn "$INSTALL_DIR/$VERSIONED" "$INSTALL_DIR/$NAME"

echo "Installed: $INSTALL_DIR/$VERSIONED -> $NAME ($BINVER)"

# --- 旧版本清理:扫描 vibetty-<数字>* 的历史版本文件,询问是否删除(默认 N)。
# rm 对正在运行的二进制是安全的(只是解除目录链接,进程握住的 inode 不受影响)。
OLD_FOUND=()
for old in "$INSTALL_DIR/${NAME%-*}"-[0-9]*; do
  [ -f "$old" ] || continue
  [ "$old" = "$INSTALL_DIR/$VERSIONED" ] && continue
  OLD_FOUND+=("$old")
done
if [ ${#OLD_FOUND[@]} -gt 0 ]; then
  echo "Found old version(s):"
  for old in "${OLD_FOUND[@]}"; do echo "  $old"; done
  ans=""
  if [ -t 0 ]; then
    read -r -p "Delete them? [y/N] " ans
  elif [ -t 1 ] && [ -r /dev/tty ]; then
    printf 'Delete old version(s) listed above? [y/N] ' > /dev/tty
    read -r ans < /dev/tty
  else
    ans="n" # 非交互:默认保留
  fi
  case "$ans" in
    [yY]*)
      for old in "${OLD_FOUND[@]}"; do rm -f "$old" && echo "Removed: $old"; done
      ;;
    *) echo "kept — delete manually if no longer needed" ;;
  esac
fi

# --- PATH check: offer to append INSTALL_DIR to the shell rc file ---
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    add_to_rc() {
      local line rc
      line="export PATH=\"$INSTALL_DIR:\$PATH\""
      case "${SHELL:-/bin/sh}" in
        */zsh) rc="$HOME/.zshrc" ;;
        */bash) rc="$HOME/.bashrc" ;;
        */fish)
          echo "fish detected — run this once instead: fish_add_path $INSTALL_DIR"
          return 0
          ;;
        *) rc="$HOME/.profile" ;;
      esac
      if [ -f "$rc" ] && grep -qF "$INSTALL_DIR" "$rc"; then
        echo "note: $rc already references $INSTALL_DIR — 'source $rc' or open a new shell"
        return 0
      fi
      printf '\n# added by vibetty install.sh\n%s\n' "$line" >> "$rc"
      echo "Added to $rc — run 'source $rc' or open a new shell to pick it up"
    }

    prompt="$INSTALL_DIR is not on your PATH. Add it to your shell rc file? [Y/n] "
    ans=""
    if [ -t 0 ]; then
      read -r -p "$prompt" ans
    elif [ -t 1 ] && [ -r /dev/tty ]; then
      # curl | bash: stdin is the script itself, ask on the terminal instead
      printf '%s' "$prompt" > /dev/tty
      read -r ans < /dev/tty
    else
      ans="__noask__" # non-interactive (no tty): don't touch rc files
    fi

    case "$ans" in
      __noask__) echo "note: $INSTALL_DIR is not on your PATH — add it manually" ;;
      [nN]*) echo "skipped — add $INSTALL_DIR to your PATH manually if needed" ;;
      *) add_to_rc ;;
    esac
    ;;
esac

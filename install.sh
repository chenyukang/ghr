#!/bin/sh
set -eu

repo="chenyukang/ghr"
bin_name="ghr"
version="${GHR_VERSION:-${1:-latest}}"
install_dir="${GHR_INSTALL_DIR:-${BIN_DIR:-$HOME/.local/bin}}"

usage() {
  cat <<'EOF'
Install ghr from GitHub release binaries.

Usage:
  curl -fsSL https://raw.githubusercontent.com/chenyukang/ghr/main/install.sh | sh
  curl -fsSL https://raw.githubusercontent.com/chenyukang/ghr/main/install.sh | GHR_VERSION=v0.6.0 sh
  curl -fsSL https://raw.githubusercontent.com/chenyukang/ghr/main/install.sh | GHR_INSTALL_DIR=/usr/local/bin sh

Environment:
  GHR_VERSION      Release tag to install. Defaults to latest.
  GHR_INSTALL_DIR  Install directory. Defaults to ~/.local/bin.
  GITHUB_TOKEN     Optional token for GitHub API/download rate limits.
EOF
}

case "$version" in
  -h|--help|help)
    usage
    exit 0
    ;;
esac

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "ghr install: required command not found: $1" >&2
    exit 1
  fi
}

fetch_to_file() {
  url="$1"
  output="$2"
  if command -v curl >/dev/null 2>&1; then
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$url" -o "$output" || return 1
    else
      curl -fsSL "$url" -o "$output" || return 1
    fi
  elif command -v wget >/dev/null 2>&1; then
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      wget -q --header="Authorization: Bearer $GITHUB_TOKEN" -O "$output" "$url" || return 1
    else
      wget -q -O "$output" "$url" || return 1
    fi
  else
    echo "ghr install: curl or wget is required" >&2
    exit 1
  fi
}

download_to_file() {
  url="$1"
  output="$2"
  if command -v curl >/dev/null 2>&1; then
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      curl -fL --progress-bar -H "Authorization: Bearer $GITHUB_TOKEN" "$url" -o "$output" || return 1
    else
      curl -fL --progress-bar "$url" -o "$output" || return 1
    fi
  elif command -v wget >/dev/null 2>&1; then
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      wget --header="Authorization: Bearer $GITHUB_TOKEN" -O "$output" "$url" || return 1
    else
      wget -O "$output" "$url" || return 1
    fi
  else
    echo "ghr install: curl or wget is required" >&2
    exit 1
  fi
}

fetch_stdout() {
  url="$1"
  tmp_file="$tmp_dir/response.json"
  fetch_to_file "$url" "$tmp_file"
  cat "$tmp_file"
}

detect_target() {
  kernel="$(uname -s)"
  machine="$(uname -m)"

  case "$kernel" in
    Darwin)
      os="apple-darwin"
      ;;
    Linux)
      os="unknown-linux-gnu"
      ;;
    *)
      echo "ghr install: unsupported OS: $kernel" >&2
      exit 1
      ;;
  esac

  case "$machine" in
    x86_64|amd64)
      arch="x86_64"
      ;;
    arm64|aarch64)
      arch="aarch64"
      ;;
    *)
      echo "ghr install: unsupported architecture: $machine" >&2
      exit 1
      ;;
  esac

  printf '%s-%s' "$arch" "$os"
}

sha256_file() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    echo "ghr install: sha256sum or shasum is required to verify the download" >&2
    exit 1
  fi
}

install_binary() {
  source_bin="$1"
  mkdir -p "$install_dir"
  if command -v install >/dev/null 2>&1; then
    install -m 755 "$source_bin" "$install_dir/$bin_name"
  else
    cp "$source_bin" "$install_dir/$bin_name"
    chmod 755 "$install_dir/$bin_name"
  fi
}

tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t ghr-install)"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

need_cmd uname
need_cmd tar

target="$(detect_target)"

if [ "$version" = "latest" ]; then
  release_json="$(fetch_stdout "https://api.github.com/repos/$repo/releases/latest")"
  tag="$(printf '%s\n' "$release_json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  if [ -z "$tag" ]; then
    echo "ghr install: failed to resolve latest release tag" >&2
    exit 1
  fi
else
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
fi

asset="$bin_name-$tag-$target"
archive="$asset.tar.gz"
base_url="https://github.com/$repo/releases/download/$tag"
archive_path="$tmp_dir/$archive"
checksum_path="$tmp_dir/$archive.sha256"

echo "ghr install: downloading $archive"
if ! download_to_file "$base_url/$archive" "$archive_path"; then
  echo "ghr install: release asset not found: $base_url/$archive" >&2
  echo "This release may have been created before prebuilt binaries were added." >&2
  exit 1
fi
if ! fetch_to_file "$base_url/$archive.sha256" "$checksum_path"; then
  echo "ghr install: checksum asset not found: $base_url/$archive.sha256" >&2
  exit 1
fi

expected="$(awk '{print $1}' "$checksum_path" | head -n 1)"
actual="$(sha256_file "$archive_path")"
if [ "$expected" != "$actual" ]; then
  echo "ghr install: checksum mismatch for $archive" >&2
  echo "expected: $expected" >&2
  echo "actual:   $actual" >&2
  exit 1
fi

tar -xzf "$archive_path" -C "$tmp_dir"
source_bin="$tmp_dir/$asset/$bin_name"
if [ ! -x "$source_bin" ]; then
  echo "ghr install: archive did not contain executable $bin_name" >&2
  exit 1
fi

install_binary "$source_bin"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    echo "ghr install: installed to $install_dir, which is not in PATH" >&2
    echo "Add this to your shell profile: export PATH=\"$install_dir:\$PATH\"" >&2
    ;;
esac

echo "ghr install: installed $tag to $install_dir/$bin_name"
echo "Next: gh auth login"
echo "Run:  ghr"

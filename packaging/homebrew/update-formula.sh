#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: $0 <formula-path> <tag> [dist-dir]" >&2
  exit 2
fi

formula_path="$1"
tag="$2"
dist_dir="${3:-dist}"
repo="${GHR_REPO:-chenyukang/ghr}"

if [[ "$tag" != v* ]]; then
  echo "Homebrew formula tag must start with v: $tag" >&2
  exit 1
fi

checksum_for() {
  local target="$1"
  local checksum_file="${dist_dir}/ghr-${tag}-${target}.tar.gz.sha256"

  if [ ! -f "$checksum_file" ]; then
    echo "missing checksum file: $checksum_file" >&2
    exit 1
  fi

  local checksum
  checksum="$(awk '{print $1}' "$checksum_file")"
  if [[ ! "$checksum" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "invalid checksum in $checksum_file: $checksum" >&2
    exit 1
  fi
  printf '%s' "$checksum" | tr 'A-F' 'a-f'
}

macos_arm_sha="$(checksum_for aarch64-apple-darwin)"
macos_intel_sha="$(checksum_for x86_64-apple-darwin)"
linux_arm_sha="$(checksum_for aarch64-unknown-linux-gnu)"
linux_intel_sha="$(checksum_for x86_64-unknown-linux-gnu)"

mkdir -p "$(dirname "$formula_path")"
cat > "$formula_path" <<FORMULA
class GhrCli < Formula
  desc "Fast terminal workspace for GitHub pull requests, issues, and notifications"
  homepage "https://github.com/chenyukang/ghr"
  license "MIT"

  depends_on "gh"

  on_macos do
    on_arm do
      url "https://github.com/${repo}/releases/download/${tag}/ghr-${tag}-aarch64-apple-darwin.tar.gz"
      sha256 "${macos_arm_sha}"
    end

    on_intel do
      url "https://github.com/${repo}/releases/download/${tag}/ghr-${tag}-x86_64-apple-darwin.tar.gz"
      sha256 "${macos_intel_sha}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/${repo}/releases/download/${tag}/ghr-${tag}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${linux_arm_sha}"
    end

    on_intel do
      url "https://github.com/${repo}/releases/download/${tag}/ghr-${tag}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${linux_intel_sha}"
    end
  end

  def install
    bin.install "ghr"
  end

  def caveats
    "Set GHR_GITHUB_TOKEN environment variable, or run \`gh auth login\`, before starting ghr."
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ghr --version")
  end
end
FORMULA

ruby -c "$formula_path" >/dev/null

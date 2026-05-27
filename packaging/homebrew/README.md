# Homebrew packaging

This directory stages the Homebrew tap plan for `ghr`.

## Install command

Once `Formula/ghr-cli.rb` is published to `chenyukang/homebrew-tap`, users can install with:

```bash
brew install chenyukang/tap/ghr-cli
```

The formula name is `ghr-cli` because Homebrew core already has an unrelated `ghr` formula for <https://github.com/tcnksm/ghr>. The installed executable remains `ghr`.

## Tap bootstrap

1. Create the tap repository:

   ```bash
   gh repo create chenyukang/homebrew-tap --public --description "Homebrew tap for chenyukang tools"
   ```

2. Copy the staged formula into the tap:

   ```bash
   mkdir -p Formula
   cp /path/to/ghr/packaging/homebrew/Formula/ghr-cli.rb Formula/ghr-cli.rb
   git add Formula/ghr-cli.rb
   git commit -m "Add ghr-cli formula"
   git push origin main
   ```

3. Verify the public install flow:

   ```bash
   brew install chenyukang/tap/ghr-cli
   ghr --version
   ```

## Release maintenance

The release workflow updates `chenyukang/homebrew-tap` automatically after the GitHub Release assets are published. The workflow requires a repository secret named `HOMEBREW_TAP_TOKEN` with write access to `chenyukang/homebrew-tap`.

Configure the token once:

```bash
gh secret set HOMEBREW_TAP_TOKEN --repo chenyukang/ghr
```

For each `ghr` release:

1. Push the release tag. The `Update Homebrew formula` job generates `Formula/ghr-cli.rb` from the release artifacts, pushes it to `chenyukang/homebrew-tap`, and syncs this staged formula copy.
2. Verify the published formula from a machine with Homebrew:

   ```bash
   brew update
   HOMEBREW_NO_AUTO_UPDATE=1 brew info chenyukang/tap/ghr-cli
   HOMEBREW_NO_AUTO_UPDATE=1 brew fetch chenyukang/tap/ghr-cli
   HOMEBREW_NO_AUTO_UPDATE=1 brew audit --strict --online chenyukang/tap/ghr-cli
   ```

Manual fallback:

```bash
tag=v0.8.1
tmp_dist="$(mktemp -d)"
for target in \
  aarch64-apple-darwin \
  x86_64-apple-darwin \
  aarch64-unknown-linux-gnu \
  x86_64-unknown-linux-gnu
do
  curl -fsSL \
    "https://github.com/chenyukang/ghr/releases/download/${tag}/ghr-${tag}-${target}.tar.gz.sha256" \
    -o "${tmp_dist}/ghr-${tag}-${target}.tar.gz.sha256"
done
packaging/homebrew/update-formula.sh Formula/ghr-cli.rb "$tag" "$tmp_dist"
```

The release workflow already publishes the binary assets consumed by this formula, so the tap should stay thin and avoid building from source.

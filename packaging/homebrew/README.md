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

For each `ghr` release:

1. Update `version`, release URLs, and `sha256` values in `Formula/ghr-cli.rb`.
2. Verify the four prebuilt tarball checksums from the GitHub Release:

   ```bash
   tag=v0.7.8
   for target in \
     aarch64-apple-darwin \
     x86_64-apple-darwin \
     aarch64-unknown-linux-gnu \
     x86_64-unknown-linux-gnu
   do
     curl -fsSL "https://github.com/chenyukang/ghr/releases/download/${tag}/ghr-${tag}-${target}.tar.gz.sha256"
   done
   ```

3. Run local Homebrew checks from a machine with Homebrew:

   ```bash
   brew install --formula ./Formula/ghr-cli.rb
   brew test ghr-cli
   brew audit --strict --online ghr-cli
   ```

The release workflow already publishes the binary assets consumed by this formula, so the tap should stay thin and avoid building from source.

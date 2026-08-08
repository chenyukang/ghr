class GhrCli < Formula
  desc "Fast terminal workspace for GitHub pull requests, issues, and notifications"
  homepage "https://github.com/chenyukang/ghr"
  license "MIT"

  depends_on "gh"

  on_macos do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.0/ghr-v0.9.0-aarch64-apple-darwin.tar.gz"
      sha256 "1ce48f118b9ef1450f2791756bddd6036f0c5e023b7d33ad1c6357939be2b5fa"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.0/ghr-v0.9.0-x86_64-apple-darwin.tar.gz"
      sha256 "e0e62e8e24ad370cfa7b9c4e2fe2bca7a2c3c9eab1ac8fb65ee64dcd0a619bcd"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.0/ghr-v0.9.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "83a876429df10d09bd45caa307a0141caca04be8d7228031f7df42ef21be1845"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.0/ghr-v0.9.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "bda958d412d98ee6ee396956c4b6299145c88f664e64c0a2eeff2a0897b69bc2"
    end
  end

  def install
    bin.install "ghr"
  end

  def caveats
    "Set GHR_GITHUB_TOKEN environment variable, or run `gh auth login`, before starting ghr."
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ghr --version")
  end
end

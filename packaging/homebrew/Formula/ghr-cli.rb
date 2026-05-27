class GhrCli < Formula
  desc "Fast terminal workspace for GitHub pull requests, issues, and notifications"
  homepage "https://github.com/chenyukang/ghr"
  version "0.8.1"
  license "MIT"

  depends_on "gh"

  on_macos do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.1/ghr-v0.8.1-aarch64-apple-darwin.tar.gz"
      sha256 "0f22862d105f614877ab8a45ac38cbc378a172b43a7d989d87e4cebd3c430b6f"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.1/ghr-v0.8.1-x86_64-apple-darwin.tar.gz"
      sha256 "7a3063fbc9162f9b987dad2bd820b4edf54c04456b61bf3940e0a10d31d5c0e7"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.1/ghr-v0.8.1-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "b167fb7bd415c18d351558101322a89cd027ffc283e53d46f97910847d45ad3a"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.1/ghr-v0.8.1-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ed577ef0a7198f8105e1d5e816af028bb591cbe6396d9238fb29526acf8357d1"
    end
  end

  def install
    bin.install "ghr"
  end

  def caveats
    "Run `gh auth login` before starting ghr."
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ghr --version")
  end
end

class GhrCli < Formula
  desc "Fast terminal workspace for GitHub pull requests, issues, and notifications"
  homepage "https://github.com/chenyukang/ghr"
  license "MIT"

  depends_on "gh"

  on_macos do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.2/ghr-v0.8.2-aarch64-apple-darwin.tar.gz"
      sha256 "d579763c5e176b904ecab9f150105b69eee91331361e97ae472cfb004c7b56e2"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.2/ghr-v0.8.2-x86_64-apple-darwin.tar.gz"
      sha256 "4d86174cf36f488403bc36d9461552e3d9d1545d147686d32be8d22eb409ee7e"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.2/ghr-v0.8.2-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "470177a7bae18395c8d2bd70319c7206ee9e47faf019042aec361e71ede8d879"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.8.2/ghr-v0.8.2-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "574ecd59e0965f1be1da8d56c48ae5e5945b39f57fc938e5254a83cdb131b78b"
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

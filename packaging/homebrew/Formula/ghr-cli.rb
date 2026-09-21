class GhrCli < Formula
  desc "Fast terminal workspace for GitHub pull requests, issues, and notifications"
  homepage "https://github.com/chenyukang/ghr"
  license "MIT"

  depends_on "gh"

  on_macos do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.1/ghr-v0.9.1-aarch64-apple-darwin.tar.gz"
      sha256 "58cc276bb384024027f55c67e9190af0491ce6214537043cbc098ec973cfab91"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.1/ghr-v0.9.1-x86_64-apple-darwin.tar.gz"
      sha256 "3523ae6a86e834cdd405c3bb6976dcc41f8121aca12fb2b428961ab8c24a1933"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.1/ghr-v0.9.1-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c358e644483ac60b94f69b78d1fe48c507db856e20567428606eea5968dd84dc"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.9.1/ghr-v0.9.1-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "ce7bb0b82d94cdc25e1abbbdff541b60acc2371db19a935e05a093d19b7fc516"
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

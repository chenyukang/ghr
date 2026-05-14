class GhrCli < Formula
  desc "Fast terminal workspace for GitHub pull requests, issues, and notifications"
  homepage "https://github.com/chenyukang/ghr"
  version "0.7.8"
  license "MIT"

  depends_on "gh"

  on_macos do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.7.8/ghr-v0.7.8-aarch64-apple-darwin.tar.gz"
      sha256 "e1cfa6a27acad0520c5f8d0065f22a228e6653dc6706357a9ab3622900a5d892"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.7.8/ghr-v0.7.8-x86_64-apple-darwin.tar.gz"
      sha256 "95cf45d45f3ac568a5f6ec2b90d96077d3a089ca36744f9958cc64b445386157"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/chenyukang/ghr/releases/download/v0.7.8/ghr-v0.7.8-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "704a03e5a145db4682f3bfde958913d026068899b3e06e48e85ac5976e65e164"
    end

    on_intel do
      url "https://github.com/chenyukang/ghr/releases/download/v0.7.8/ghr-v0.7.8-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "8c1e77506792ebe7f517c73f636aa93bdbe876be68f1d700a152c68d1cc26f9c"
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

class Lazyvial < Formula
  desc "Remap a Vial keyboard from the terminal"
  homepage "https://github.com/imjma/lazyvial"
  url "https://github.com/imjma/lazyvial/archive/refs/tags/v0.1.1.tar.gz"
  sha256 "113b7e7509e076d423e9f7cf0667ee7db5d4f575832739591edbd4b439b7c3b5"
  license "MIT"
  head "https://github.com/imjma/lazyvial.git", branch: "main"

  depends_on "rust" => :build

  on_linux do
    depends_on "pkgconf" => :build
    depends_on "systemd" # libudev, for hidapi's hidraw backend
  end

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_equal "lazyvial #{version}", shell_output("#{bin}/lazyvial --version").strip
    assert_match "unknown argument", shell_output("#{bin}/lazyvial --nope 2>&1", 2)
  end
end

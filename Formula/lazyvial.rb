class Lazyvial < Formula
  desc "Remap a Vial keyboard from the terminal"
  homepage "https://github.com/imjma/lazyvial"
  url "https://github.com/imjma/lazyvial/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "e2180c3dd734232959928f8d1c05ea23d4af4fbf4824c7678a5f4c998e23e224"
  license "GPL-2.0-or-later"
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

class Lazykeymap < Formula
  desc "Remap a Vial or ZMK keyboard, or a ZMK keymap file, from the terminal"
  homepage "https://github.com/imjma/lazykeymap"
  url "https://github.com/imjma/lazykeymap/archive/refs/tags/v0.3.0.tar.gz"
  sha256 "186191dd25b2125c6ea6ffe88e563eb77ce53fe9da36b2c5f03b92fc8844f1dc"
  license "MIT"
  head "https://github.com/imjma/lazykeymap.git", branch: "main"

  depends_on "rust" => :build

  on_linux do
    depends_on "pkgconf" => :build
    depends_on "systemd" # libudev, for hidapi's hidraw backend
  end

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_equal "lazykeymap #{version}", shell_output("#{bin}/lazykeymap --version").strip
    assert_match "unknown argument", shell_output("#{bin}/lazykeymap --nope 2>&1", 2)
  end
end

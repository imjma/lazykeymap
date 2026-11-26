class Lazykeymap < Formula
  desc "Remap a Vial or ZMK keyboard, or a ZMK keymap file, from the terminal"
  homepage "https://github.com/imjma/lazykeymap"
  url "https://github.com/imjma/lazykeymap/archive/refs/tags/v0.3.2.tar.gz"
  sha256 "a27e52a5bbd5c0f1934584abe8b8cfd59eb612bfd77b2ced1d2ae9087a048fe3"
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

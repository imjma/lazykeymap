class Lazykeymap < Formula
  desc "Remap a Vial or ZMK keyboard, or a ZMK keymap file, from the terminal"
  homepage "https://github.com/imjma/lazykeymap"
  url "https://github.com/imjma/lazykeymap/archive/refs/tags/v0.3.1.tar.gz"
  sha256 "d86c371c7612c1c2116fa6a6c0d70db97269c6715e09910ce5cd105628f9714c"
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

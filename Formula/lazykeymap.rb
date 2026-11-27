class Lazykeymap < Formula
  desc "Remap a Vial or ZMK keyboard, or a ZMK keymap file, from the terminal"
  homepage "https://github.com/imjma/lazykeymap"
  url "https://github.com/imjma/lazykeymap/archive/refs/tags/v0.4.0.tar.gz"
  sha256 "34aa3dc8c25387f452032c6e1b9c77462b2e47f07fd406bbfb4d261b78633db5"
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

class Lazykeymap < Formula
  desc "Remap a Vial or ZMK keyboard, or a ZMK keymap file, from the terminal"
  homepage "https://github.com/imjma/lazykeymap"
  url "https://github.com/imjma/lazykeymap/archive/refs/tags/v0.5.0.tar.gz"
  sha256 "2294762788d71436aef78dbfec2484f7e85527aae5d7372b028d0b06156e5e9c"
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

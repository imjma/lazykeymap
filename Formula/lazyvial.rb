class Lazyvial < Formula
  desc "Remap a Vial keyboard from the terminal"
  homepage "https://github.com/imjma/lazyvial"
  url "https://github.com/imjma/lazyvial/archive/refs/tags/v0.1.4.tar.gz"
  sha256 "3ece10c666b26cd22e3ce5ae81b3840ad7587f2cffa6264ca852c87da4a119e1"
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

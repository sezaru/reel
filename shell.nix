# The development environment: `nix-shell --run 'cargo build --release'`. (Installing: the
# flake's package wraps the same run-time pieces.)
{ pkgs ? import <nixpkgs> {} }:
with pkgs;
mkShell {
  packages = [ cargo rustc clippy rustfmt pkg-config libjpeg_turbo fontconfig ffmpeg-headless ];
  # wgpu opens Vulkan at run time; the drivers come from the system.
  # (and the preview window opens Wayland or X11, keyboard maps from xkbcommon)
  LD_LIBRARY_PATH = lib.makeLibraryPath [ vulkan-loader wayland libxkbcommon libx11 libxcursor libxi libxrandr ] + ":/run/opengl-driver/lib";
  # The fallback fonts text may need, whatever the host has (fonts.nix).
  FONTCONFIG_FILE = import ./fonts.nix { inherit pkgs; };
}

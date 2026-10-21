{
  description = "reel: videos from TypeScript scene files, drawn on the GPU";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      runtimeLibs = pkgs: with pkgs; [ vulkan-loader wayland libxkbcommon libx11 libxcursor libxi libxrandr ];
    in
    {
      packages = forAll (pkgs: rec {
        reel = pkgs.rustPlatform.buildRustPackage {
          pname = "reel";
          version = "0.1.0";
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];
          buildInputs = with pkgs; [ libjpeg_turbo fontconfig ];
          # (the GPU's own driver comes from the system: /run/opengl-driver on NixOS)
          postInstall = ''
            wrapProgram $out/bin/reel \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.ffmpeg-headless ]} \
              --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)}:/run/opengl-driver/lib \
              --set-default FONTCONFIG_FILE ${import ./fonts.nix { inherit pkgs; }}
          '';
          meta = {
            description = "Videos from TypeScript scene files, drawn on the GPU";
            mainProgram = "reel";
            license = pkgs.lib.licenses.mit;
          };
        };
        default = reel;
      });

      devShells = forAll (pkgs: {
        default = import ./shell.nix { inherit pkgs; };
      });
    };
}

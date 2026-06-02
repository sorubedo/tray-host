{
  description = "tray-host: headless system tray daemon for use with fuzzel/rofi/dmenu";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
      in
      rec {

        packages.tray-host =
          with pkgs;
          rustPlatform.buildRustPackage rec {
            pname = manifest.name;
            inherit (manifest) version;

            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = [
              installShellFiles
            ];

            postInstall = ''
              installShellCompletion --cmd tray-host \
                --bash <($out/bin/tray-host --completions bash) \
                --zsh <($out/bin/tray-host --completions zsh) \
                --fish <($out/bin/tray-host --completions fish)
            '';

            passthru.updateScript = nix-update-script { };

            meta = {
              description = "Headless StatusNotifierItem host for use with external launchers like fuzzel/rofi/dmenu";
              homepage = "https://github.com/Levizor/tray-tui";
              license = lib.licenses.mit;
              mainProgram = "tray-host";
              maintainers = with lib.maintainers; [ Levizor ];
              platforms = lib.platforms.linux;
            };
          };

        defaultPackage = packages.tray-host;

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            pkgconf
            rust-analyzer
            rustc
            cargo
            cargo-edit
            git
          ];
          RUST_BACKTRACE = "1";
          CARGO_INCREMENTAL = "1";
        };
      }
    );

}

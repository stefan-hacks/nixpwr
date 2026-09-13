{
  description = "powerctl - Linux laptop power diagnostics and policy";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "powerctl";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
        };
      });

      overlays.default = final: prev: {
        powerctl = self.packages.${final.system}.default;
      };

      nixosModules.default = { config, lib, pkgs, ... }:
        with lib; {
          options.services.powerctl.enable = mkEnableOption "powerctl diagnostics and policy service";

          config = mkIf config.services.powerctl.enable {
            environment.systemPackages = [ self.packages.${pkgs.system}.default ];
          };
        };
    };
}

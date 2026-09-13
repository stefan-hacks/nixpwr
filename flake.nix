{
  description = "nixpwr - Linux laptop power diagnostics and policy";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "nixpwr";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
        };
      });

      overlays.default = final: prev: {
        nixpwr = self.packages.${final.system}.default;
      };

      # NixOS module with privileged helper, hardware profiles, and auto-tuning.
      nixosModules.default = { config, lib, pkgs, ... }:
        with lib;
        let
          cfg = config.services.nixpwr;
          nixpwrPkg = self.packages.${pkgs.system}.default;

          # Polkit action definition for the nixpwr helper.
          polkitPolicy = pkgs.writeText "org.nixpwr.policy" ''
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE policyconfig PUBLIC
             "-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN"
             "http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd">
            <policyconfig>
              <action id="org.nixpwr.set-profile">
                <description>Set platform power profile</description>
                <message>Authentication is required to change the platform power profile.</message>
                <defaults>
                  <allow_any>no</allow_any>
                  <allow_inactive>no</allow_inactive>
                  <allow_active>yes</allow_active>
                </defaults>
                <annotate key="org.freedesktop.policykit.exec.path">${nixpwrPkg}/bin/nixpwr</annotate>
                <annotate key="org.freedesktop.policykit.exec.argv1">profile</annotate>
                <annotate key="org.freedesktop.policykit.exec.argv2">set</annotate>
              </action>
            </policyconfig>
          '';
        in {
          options.services.nixpwr = {
            enable = mkEnableOption "nixpwr diagnostics and policy service";

            autoTune = mkOption {
              type = types.bool;
              default = false;
              description = ''
                Whether to apply platform-specific power settings at boot.
                This includes PCIe ASPM, USB autosuspend, and CPU governor hints.
              '';
            };

            platformProfile = mkOption {
              type = types.nullOr (types.enum [ "performance" "balanced" "low-power" ]);
              default = null;
              description = ''
                Default ACPI platform profile to apply at boot.
              '';
            };

            hardwareModule = mkOption {
              type = types.nullOr (types.enum [ "thinkpad" "framework" "xps" "spectre" "generic-intel" "generic-amd" ]);
              default = null;
              description = ''
                Hardware-specific tuning module. If set, autoTune is implied.
              '';
            };
          };

          config = mkIf cfg.enable {
            environment.systemPackages = [ nixpwrPkg ];

            # Polkit policy so non-root users can change profiles.
            environment.etc."polkit-1/actions/org.nixpwr.policy".source = polkitPolicy;

            # If hardwareModule is set, enable auto-tuning automatically.
            services.nixpwr.autoTune = mkIf (cfg.hardwareModule != null) (mkDefault true);

            # Boot-time tuning.
            systemd.services.nixpwr-tune = mkIf cfg.autoTune {
              description = "nixpwr platform power tuning";
              wantedBy = [ "multi-user.target" ];
              serviceConfig = {
                Type = "oneshot";
                RemainAfterExit = true;
              };
              script = optionalString cfg.autoTune ''
                #!/usr/bin/env bash
                set -euo pipefail
                echo "Applying nixpwr power tuning..."

                # PCIe ASPM
                if [ -f /sys/module/pcie_aspm/parameters/policy ]; then
                  echo powersave > /sys/module/pcie_aspm/parameters/policy || true
                fi

                # USB autosuspend
                if [ -f /sys/module/usbcore/parameters/autosuspend ]; then
                  echo -1 > /sys/module/usbcore/parameters/autosuspend || true
                fi

                # Platform profile
                ${optionalString (cfg.platformProfile != null) ''
                  if [ -f /sys/firmware/acpi/platform_profile ]; then
                    echo ${cfg.platformProfile} > /sys/firmware/acpi/platform_profile || true
                  fi
                ''}

                ${optionalString (cfg.hardwareModule == "thinkpad") ''
                  # ThinkPad-specific tuning
                  if [ -d /sys/devices/platform/thinkpad_acpi ]; then
                    echo auto > /sys/bus/usb/devices/usb*/power/control 2>/dev/null || true
                    echo enabled > /sys/devices/system/cpu/intel_pstate/no_turbo 2>/dev/null || true
                  fi
                ''}

                ${optionalString (cfg.hardwareModule == "framework") ''
                  # Framework-specific tuning
                  if [ -d /sys/devices/platform/framework_laptop ]; then
                    echo enabled > /sys/devices/platform/framework_laptop/framework_laptop/ec_leds/power 2>/dev/null || true
                  fi
                ''}

                ${optionalString (cfg.hardwareModule == "xps") ''
                  # Dell XPS-specific tuning
                  if [ -d /sys/devices/platform/dell-smm-hwmon ]; then
                    echo balanced > /sys/firmware/acpi/platform_profile 2>/dev/null || true
                  fi
                ''}

                ${optionalString (cfg.hardwareModule == "spectre") ''
                  # HP Spectre-specific tuning
                  if [ -d /sys/devices/platform/hp-wmi ]; then
                    echo balanced > /sys/firmware/acpi/platform_profile 2>/dev/null || true
                  fi
                ''}

                ${optionalString (cfg.hardwareModule == "generic-intel") ''
                  # Intel generic tuning
                  if [ -d /sys/devices/system/cpu/intel_pstate ]; then
                    echo active > /sys/devices/system/cpu/intel_pstate/status 2>/dev/null || true
                    echo balanced > /sys/firmware/acpi/platform_profile 2>/dev/null || true
                  fi
                ''}

                ${optionalString (cfg.hardwareModule == "generic-amd") ''
                  # AMD generic tuning
                  if [ -d /sys/devices/system/cpu/cpufreq/policy0 ]; then
                    if [ -f /sys/devices/system/cpu/cpufreq/policy0/scaling_driver ]; then
                      driver=$(cat /sys/devices/system/cpu/cpufreq/policy0/scaling_driver)
                      if [ "$driver" = "amd-pstate" ]; then
                        echo active > /sys/devices/system/cpu/amd_pstate/status 2>/dev/null || true
                      fi
                    fi
                    echo balanced > /sys/firmware/acpi/platform_profile 2>/dev/null || true
                  fi
                ''}
              '';
            };
          };
        };
    };
}

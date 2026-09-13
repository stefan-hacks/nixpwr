# nixpwr

`nixpwr` is a NixOS-friendly Linux laptop power diagnostics CLI.

The design goal is **diagnose platform power behaviour rather than blindly tune CPU frequency**.

## Commands

### Quick overview

```bash
nixpwr status              # Battery, CPU driver, platform profile, kernel
nixpwr diagnose            # Find likely causes of excessive battery drain
nixpwr inspect             # Raw kernel power-management facts
nixpwr --json status       # Machine-readable JSON output (global flag)
```

### Platform profile (needs root)

```bash
nixpwr profile get         # Show current profile
sudo nixpwr profile set balanced
sudo nixpwr profile set battery
sudo nixpwr profile set performance
sudo nixpwr profile set ultra-battery
```

### Detailed inspection commands

```bash
nixpwr devices             # /sys runtime-PM device inventory
nixpwr cstates             # CPU package and per-core C-state residency
nixpwr usb                 # USB runtime-PM offenders
nixpwr nvme                # NVMe APST state
nixpwr wifi                # Wi-Fi power-save state
nixpwr display             # Display refresh rate / VRR state
nixpwr systemd             # systemd inhibitors and wakeup sources
nixpwr battery-health      # Battery health / cycle analysis
nixpwr report              # Generate a support bundle report (JSON)
nixpwr report -o bundle.json
nixpwr watch               # Live battery-power sampling
nixpwr watch -i 5 -c 10    # 10 samples every 5 seconds
```

## What it currently inspects

- battery capacity, state, instantaneous power
- battery health (cycles, design/full capacity, voltage)
- energy/charge values
- CPU frequency driver and C-state residency
- ACPI platform profile
- available platform profiles
- Intel/AMD pstate state
- PCIe ASPM policy
- USB autosuspend setting
- `/sys` runtime-PM device inventory
- GPU residency (Intel GT power-gate / AMD runtime status)
- device wakeup sources
- USB runtime-PM offenders
- NVMe APST state
- Wi-Fi power-save state
- display refresh rate / VRR
- systemd inhibitors and wakeup sources
- kernel version and total memory

## NixOS

### Run without installing

You don't need to clone or build anything:

```bash
nix run github:stefan-hacks/nixpwr -- status
nix run github:stefan-hacks/nixpwr -- diagnose
nix run github:stefan-hacks/nixpwr -- profile
```

This fetches the latest `main`, builds the package in the Nix store, and runs it.

### Build locally

```bash
nix build
# result/bin/nixpwr --help
```

### Add to your own flake

Add `nixpwr` as an input in your `flake.nix`:

```nix
{
  inputs.nixpwr.url = "github:stefan-hacks/nixpwr";

  outputs = { self, nixpkgs, nixpwr, ... }:
    let
      system = "x86_64-linux";   # or aarch64-linux
      pkgs = nixpkgs.legacyPackages.${system};
    in {
      nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          {
            environment.systemPackages = [ nixpwr.packages.${system}.default ];
          }
        ];
      };
    };
}
```

Or use the overlay so `nixpwr` appears in `pkgs`:

```nix
{
  inputs.nixpwr.url = "github:stefan-hacks/nixpwr";

  outputs = { self, nixpkgs, nixpwr, ... }:
    let
      system = "x86_64-linux";
    in {
      nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          ({ pkgs, ... }: {
            nixpkgs.overlays = [ nixpwr.overlays.default ];
            environment.systemPackages = [ pkgs.nixpwr ];
          })
        ];
      };
    };
}
```

### Import the NixOS module

```nix
{
  imports = [ inputs.nixpwr.nixosModules.default ];

  services.nixpwr.enable = true;

  # Optional: hardware-specific tuning
  services.nixpwr.hardwareModule = "thinkpad";  # or framework, xps, spectre, generic-intel, generic-amd

  # Optional: auto-apply tuning at boot
  services.nixpwr.autoTune = true;

  # Optional: set default platform profile at boot
  services.nixpwr.platformProfile = "balanced";  # or "performance", "low-power"
}
```

The module also installs a Polkit policy so that regular users can change the platform profile without `sudo` via a future GUI helper.

## Architecture

The intended long-term architecture is:

```text
nixpwr
├── status       snapshot current platform state
├── diagnose     identify likely causes of drain
├── profile      declarative power policy (needs root)
├── inspect      expose kernel/runtime-PM facts
├── devices      runtime-PM device inventory
├── cstates      CPU C-state residency
├── usb          USB runtime-PM offenders
├── nvme         NVMe APST state
├── wifi         Wi-Fi power-save state
├── display      display refresh/VRR
├── systemd      inhibitors and wakeup sources
├── battery-health
├── report       generate support bundle (JSON)
└── watch        live battery-power sampling
```

The important design principle is to keep **measurement separate from policy**. The tool should tell the user what is consuming power before changing anything.

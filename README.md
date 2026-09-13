# nixpwr

`nixpwr` is a NixOS-friendly Linux laptop power diagnostics CLI.

The design goal is **diagnose platform power behaviour rather than blindly tune CPU frequency**.

## Current MVP

```bash
nixpwr status
nixpwr diagnose
nixpwr inspect
nixpwr profile
sudo nixpwr profile set balanced
```

Machine-readable output:

```bash
nixpwr --json status
```

## What it currently inspects

- battery capacity/state
- instantaneous battery power where the kernel exposes it
- energy/charge values
- CPU frequency driver
- ACPI platform profile
- available platform profiles
- Intel/AMD pstate state
- PCIe ASPM policy
- USB autosuspend setting
- kernel version
- total memory

## NixOS

Build:

```bash
nix build
```

Run directly:

```bash
nix run . -- status
nix run . -- diagnose
```

Install into a NixOS configuration:

```nix
environment.systemPackages = [
  inputs.nixpwr.packages.${pkgs.system}.default
];
```

or import the module:

```nix
imports = [
  inputs.nixpwr.nixosModules.default
];

services.nixpwr.enable = true;
```

## Architecture

The intended long-term architecture is:

```text
nixpwr
├── status       snapshot current platform state
├── diagnose     identify likely causes of drain
├── profile      declarative power policy
└── inspect      expose kernel/runtime-PM facts
```

Future versions should add:

- `/sys` runtime-PM device inventory
- CPU package/C-state residency
- GPU residency
- device wakeup sources
- USB runtime-PM offenders
- NVMe APST state
- Wi-Fi power-save state
- display refresh/VRR state
- systemd inhibitor/wakeup analysis
- historical sampling
- `nixpwr watch`
- JSON schema for machine integration
- NixOS hardware-specific policy modules
- a privileged helper instead of requiring the whole CLI to run as root
- battery health/cycle analysis
- `nixpwr report` for support bundles

The important design principle is to keep **measurement separate from policy**. The tool should tell the user what is consuming power before changing anything.

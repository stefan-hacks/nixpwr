use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser, Debug)]
#[command(name = "nixpwr", version, about = "Linux laptop power diagnostics and policy")]
struct Cli {
    #[command(subcommand)]
    command: CommandKind,
    /// Emit machine-readable JSON where supported.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum CommandKind {
    /// Show a concise power/thermal/platform summary.
    Status,
    /// Find likely causes of excessive idle battery drain.
    Diagnose,
    /// Show or change the requested power profile.
    Profile {
        #[command(subcommand)]
        command: Option<ProfileCommand>,
    },
    /// Print useful raw kernel power-management information.
    Inspect,
}

#[derive(Subcommand, Debug)]
enum ProfileCommand {
    Get,
    Set { profile: Profile },
}

#[derive(Clone, Debug, ValueEnum, Serialize)]
enum Profile {
    Performance,
    Balanced,
    Battery,
    UltraBattery,
}

#[derive(Serialize)]
struct Status {
    battery: Option<Battery>,
    cpu_driver: Option<String>,
    platform_profile: Option<String>,
    available_platform_profiles: Vec<String>,
    mem_total_gib: u64,
    kernel: Option<String>,
}

#[derive(Serialize)]
struct Battery {
    capacity_percent: Option<u64>,
    status: Option<String>,
    energy_now_wh: Option<f64>,
    energy_full_wh: Option<f64>,
    power_now_w: Option<f64>,
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| p.exists()).cloned()
}

fn battery() -> Option<Battery> {
    let base = Path::new("/sys/class/power_supply");
    let entries = fs::read_dir(base).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if read(p.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let capacity_percent = read(p.join("capacity")).and_then(|x| x.parse().ok());
        let status = read(p.join("status"));
        let scale = 1_000_000_000.0;
        let energy_now_wh = read(p.join("energy_now")).and_then(|x| x.parse::<f64>().ok()).map(|x| x / scale);
        let energy_full_wh = read(p.join("energy_full")).and_then(|x| x.parse::<f64>().ok()).map(|x| x / scale);
        let power_now_w = read(p.join("power_now")).and_then(|x| x.parse::<f64>().ok()).map(|x| x / scale);

        // Some platforms expose charge_* instead of energy_*.
        let energy_now_wh = energy_now_wh.or_else(|| {
            read(p.join("charge_now")).and_then(|x| x.parse::<f64>().ok()).map(|x| x / scale)
        });
        let energy_full_wh = energy_full_wh.or_else(|| {
            read(p.join("charge_full")).and_then(|x| x.parse::<f64>().ok()).map(|x| x / scale)
        });
        let power_now_w = power_now_w.or_else(|| {
            read(p.join("current_now")).and_then(|x| x.parse::<f64>().ok()).and_then(|ua| {
                read(p.join("voltage_now")).and_then(|x| x.parse::<f64>().ok()).map(|uv| ua * uv / 1e15)
            })
        });

        return Some(Battery { capacity_percent, status, energy_now_wh, energy_full_wh, power_now_w });
    }
    None
}

fn cpu_driver() -> Option<String> {
    let p = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_driver";
    read(p)
}

fn platform_profile() -> Option<(String, Vec<String>)> {
    let p = Path::new("/sys/firmware/acpi/platform_profile");
    if !p.exists() {
        return None;
    }
    let current = read(p.join("")).or_else(|| read("/sys/firmware/acpi/platform_profile"))?;
    let available = read("/sys/firmware/acpi/platform_profile_choices")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    Some((current, available))
}

fn kernel() -> Option<String> {
    Command::new("uname").arg("-r").output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn memory_gib() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory() / 1024 / 1024 / 1024
}

fn status() -> Status {
    let pp = platform_profile();
    Status {
        battery: battery(),
        cpu_driver: cpu_driver(),
        platform_profile: pp.as_ref().map(|x| x.0.clone()),
        available_platform_profiles: pp.map(|x| x.1).unwrap_or_default(),
        mem_total_gib: memory_gib(),
        kernel: kernel(),
    }
}

fn print_status(json: bool) -> Result<()> {
    let s = status();
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!("nixpwr status");
    println!("────────────────────────────────────────");
    if let Some(b) = s.battery {
        println!("Battery       : {}%", b.capacity_percent.map_or("?".into(), |v| v.to_string()));
        println!("State         : {}", b.status.unwrap_or_else(|| "?".into()));
        if let Some(w) = b.power_now_w { println!("Power draw    : {:.2} W", w); }
        if let (Some(now), Some(full)) = (b.energy_now_wh, b.energy_full_wh) {
            println!("Energy        : {:.2} / {:.2} Wh", now, full);
        }
    } else {
        println!("Battery       : not detected");
    }
    println!("CPU driver    : {}", s.cpu_driver.unwrap_or_else(|| "?".into()));
    println!("Platform      : {}", s.platform_profile.unwrap_or_else(|| "unavailable".into()));
    if !s.available_platform_profiles.is_empty() {
        println!("Profiles      : {}", s.available_platform_profiles.join(", "));
    }
    println!("Memory        : {} GiB", s.mem_total_gib);
    println!("Kernel        : {}", s.kernel.unwrap_or_else(|| "?".into()));
    Ok(())
}

fn write_file(path: &str, value: &str) -> Result<()> {
    fs::write(path, value).with_context(|| format!("cannot write {path}; try running as root"))
}

fn set_profile(profile: Profile) -> Result<()> {
    let target = match profile {
        Profile::Performance => "performance",
        Profile::Balanced => "balanced",
        Profile::Battery => "low-power",
        Profile::UltraBattery => "low-power",
    };
    if Path::new("/sys/firmware/acpi/platform_profile").exists() {
        write_file("/sys/firmware/acpi/platform_profile", target)?;
        println!("Platform profile set to {target}");
        if matches!(profile, Profile::UltraBattery) {
            println!("Note: UltraBattery is currently a conservative alias for low-power.");
        }
        return Ok(());
    }
    anyhow::bail!("ACPI platform profile is unavailable on this machine")
}

fn inspect() {
    let paths = [
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_driver",
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
        "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference",
        "/sys/devices/system/cpu/intel_pstate/status",
        "/sys/devices/system/cpu/amd_pstate/status",
        "/sys/firmware/acpi/platform_profile",
        "/sys/firmware/acpi/platform_profile_choices",
    ];
    for p in paths {
        if let Some(v) = read(p) {
            println!("{:<65} {}", p, v);
        }
    }
}

fn diagnose() -> Result<()> {
    let s = status();
    println!("nixpwr diagnose");
    println!("────────────────────────────────────────");
    let mut findings = 0;

    if let Some(b) = &s.battery {
        if let Some(w) = b.power_now_w {
            println!("Current battery power: {:.2} W", w);
            if w > 12.0 {
                println!("⚠ High idle-load candidate: >12 W is unusually high for a lightly loaded laptop.");
                findings += 1;
            } else if w > 8.0 {
                println!("⚠ Elevated power draw: investigate GPU, display, USB and wakeups.");
                findings += 1;
            } else {
                println!("✓ Power draw is not obviously excessive.");
            }
        }
    }

    match s.cpu_driver.as_deref() {
        Some("intel_pstate") | Some("amd_pstate") => println!("✓ Modern CPU frequency driver: {}", s.cpu_driver.unwrap()),
        Some(other) => { println!("⚠ CPU frequency driver: {other}"); findings += 1; }
        None => { println!("⚠ CPU frequency driver could not be detected."); findings += 1; }
    }

    if s.platform_profile.is_none() {
        println!("⚠ No ACPI platform-profile interface detected.");
        println!("  This does not prove power management is broken; firmware may expose another interface.");
        findings += 1;
    }

    let aspm = read("/sys/module/pcie_aspm/parameters/policy");
    if let Some(v) = aspm {
        println!("PCIe ASPM policy: {v}");
        if v.contains("performance") {
            println!("⚠ PCIe ASPM is biased toward performance.");
            findings += 1;
        }
    }

    let autosuspend = read("/sys/module/usbcore/parameters/autosuspend");
    if let Some(v) = autosuspend {
        println!("USB autosuspend timeout: {v}s");
    }

    if Command::new("command").output().is_err() {
        // no-op; keep diagnose dependency-light
    }

    println!();
    if findings == 0 {
        println!("No obvious kernel/platform red flags detected.");
        println!("Next step: inspect wakeups and device runtime-PM state.");
    } else {
        println!("{findings} potential issue(s) found.");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CommandKind::Status => print_status(cli.json),
        CommandKind::Diagnose => diagnose(),
        CommandKind::Inspect => { inspect(); Ok(()) }
        CommandKind::Profile { command } => match command.unwrap_or(ProfileCommand::Get) {
            ProfileCommand::Get => print_status(cli.json),
            ProfileCommand::Set { profile } => set_profile(profile),
        },
    }
}

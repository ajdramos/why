//! Dependency checking module
//! Verifies availability of external command-line tools

use anyhow::Result;
use colored::*;
use rust_i18n::t;

use crate::is_command_available;

/// Check and display all external command dependencies
pub fn check_deps() -> Result<()> {
    println!("{}", t!("deps_header").bold().underline());
    println!();

    // Define all external commands with their category and purpose
    let deps = vec![
        (
            t!("deps_core_system").to_string(),
            vec![
                ("df", t!("deps_disk_usage").to_string(), true),
                ("dmesg", t!("deps_kernel_logs").to_string(), true),
                ("lsblk", t!("deps_block_device").to_string(), true),
                ("pgrep", t!("deps_process_search").to_string(), true),
                ("netstat", t!("deps_network_stats").to_string(), false),
            ],
        ),
        (
            t!("deps_hardware_monitoring").to_string(),
            vec![
                ("sensors", t!("deps_sensors").to_string(), false),
                ("nvidia-smi", t!("deps_nvidia_smi").to_string(), false),
                ("rocm-smi", t!("deps_rocm_smi").to_string(), false),
                ("intel_gpu_top", t!("deps_intel_gpu").to_string(), false),
            ],
        ),
        (
            t!("deps_power_management").to_string(),
            vec![("upower", t!("deps_upower").to_string(), false)],
        ),
        (
            t!("deps_network").to_string(),
            vec![("nmcli", t!("deps_nmcli").to_string(), false)],
        ),
        (
            t!("deps_audio_video").to_string(),
            vec![
                ("pw-metadata", t!("deps_pw_metadata").to_string(), false),
                ("glxinfo", t!("deps_glxinfo").to_string(), false),
                ("vulkaninfo", t!("deps_vulkaninfo").to_string(), false),
            ],
        ),
        (
            t!("deps_system_services").to_string(),
            vec![
                (
                    "systemd-analyze",
                    t!("deps_systemd_analyze").to_string(),
                    false,
                ),
                ("journalctl", t!("deps_journalctl").to_string(), false),
            ],
        ),
        (
            t!("deps_bluetooth").to_string(),
            vec![("bluetoothctl", t!("deps_bluetoothctl").to_string(), false)],
        ),
        (
            t!("deps_gaming").to_string(),
            vec![("prime-run", t!("deps_prime_run").to_string(), false)],
        ),
        (
            t!("deps_containers").to_string(),
            vec![
                ("docker", t!("deps_docker").to_string(), false),
                ("flatpak", t!("deps_flatpak").to_string(), false),
            ],
        ),
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

    println!("{}", t!("deps_summary").bold().underline());
    let percentage = (available as f32 / total as f32 * 100.0) as u32;
    let summary = t!("deps_commands_available")
        .replace("{available}", &available.to_string())
        .replace("{total}", &total.to_string())
        .replace("{percentage}", &percentage.to_string());

    if percentage >= 80 {
        println!("{}", summary.green().bold());
    } else if percentage >= 50 {
        println!("{}", summary.yellow().bold());
    } else {
        println!("{}", summary.red().bold());
    }

    println!();
    println!("{}", t!("deps_legend").dimmed());
    println!("  {} {}", "✓".green(), t!("deps_available"));
    println!("  {} {}", "○".yellow(), t!("deps_missing_optional"));
    println!("  {} {}", "✗".red(), t!("deps_missing_required"));

    Ok(())
}

/// Check for missing critical diagnostic tools
/// Returns a list of (command, i18n_key) tuples for tools that are not available
pub fn check_missing_critical_tools() -> Vec<(&'static str, &'static str)> {
    let critical = vec![
        ("sensors", "tool_purpose_sensors"),
        ("upower", "tool_purpose_upower"),
        ("nmcli", "tool_purpose_nmcli"),
        ("systemd-analyze", "tool_purpose_systemd_analyze"),
        ("nvidia-smi", "tool_purpose_nvidia_smi"),
        ("rocm-smi", "tool_purpose_rocm_smi"),
    ];

    critical
        .into_iter()
        .filter(|(cmd, _)| !is_command_available(cmd))
        .collect()
}

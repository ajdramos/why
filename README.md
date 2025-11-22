# why

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Version](https://img.shields.io/badge/version-1.3.0-green.svg)](https://github.com/ajdramos/why/releases)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![GitHub stars](https://img.shields.io/github/stars/ajdramos/why?style=social)](https://github.com/ajdramos/why/stargazers)

**The Linux troubleshooting command everyone wished existed.**

```bash
why                  # full system diagnosis in <200ms
why slow             # performance analysis: CPU/RAM/disk + top processes
why wifi             # why is Wi-Fi slow / unstable?
why battery          # why is the battery dying so fast?
why fan              # why are the fans screaming?
why gpu              # GPU diagnostics (NVIDIA/AMD/Intel)
why gaming           # gaming performance issues (Steam/Proton)
why hot              # temperature issues
why boot             # why does boot take forever?
why boot-critical    # deep dive into the systemd critical path
why storage          # SMART/Btrfs/ZFS/RAID health summary
why security         # SELinux/AppArmor/firewall posture + listening ports
why rca              # root-cause timeline (OOM, panics, throttling)
why kube-node        # node pressure, kubelet state and failing pods
why check-deps       # verify which diagnostic tools are installed
why --snapshot       # generate forensic snapshot (JSON) for bug reports
why --watch          # live htop-style dashboard with explanations
```


### Installation

```bash
# Build from source (recommended)
git clone https://github.com/ajdramos/why.git
cd why
cargo build --release
sudo cp target/release/why /usr/local/bin/

# Or download pre-built binary (when available)
curl -L https://github.com/ajdramos/why/releases/latest/download/why -o why
chmod +x why
sudo mv why /usr/local/bin/
```

### Features (November 2025 – v1.3)

- **113 real-world rules** covering CPU, RAM, disk, network, Wi-Fi, Bluetooth, battery, fans, temperatures, snaps, Flatpaks, NVIDIA/AMD/Intel GPUs, gaming (Steam/Proton/CS2), Wayland, PipeWire, Firefox hardware acceleration, ZFS, LUKS, boot time, btrfs snapshots, systemd-oomd, baloo, Cloudflare WARP, and more
- **GPU diagnostics** for NVIDIA (nvidia-smi), AMD (rocm-smi + sysfs), and Intel (sysfs kernel hwmon) with vendor-specific tips
- **Gaming performance analysis** including GameMode, MangoHud, Proton version detection, CS2 optimization, works with or without Steam — gaming rules intelligently filtered to avoid noise in general diagnostics
- **Forensic snapshots** (`why --snapshot`) — generates complete JSON report with full system state (always includes GPU metrics) for bug reports and support tickets
- **Dependency checking** (`why check-deps`) lists all external tools and shows what's missing
- **Missing tools warnings** in dashboard alert when critical diagnostic tools aren't installed (lm-sensors, upower, nvidia-smi, etc.)
- **Performance profiling** — track execution time with `WHY_BENCHMARK=1` or `RUST_LOG=debug` (target: <200ms)
- **Locale-safe parsing** — works correctly on systems using Portuguese, German, French, etc. (comma decimals) for all numeric outputs
- **Security hardened** — command injection prevention, safe auto-fix whitelist, path traversal protection, no shell usage
- Zero dependencies outside the Rust std lib + a few crates
- Fully distro-agnostic (systemd or not, apt/dnf/pacman/zypper)
- Sub-200 ms response time
- Live TUI mode (`why --watch`)
- **Internationalisation** — Full i18n support for diagnostic output including snapshots (currently English and Portuguese via `--lang pt`)
- Safe auto-fix for harmless issues (with confirmation and whitelist validation)
- **CI validation** for community rule contributions
- MIT licensed – companies love it

### Example output

```text
╭───── WHY 1.3 ─────╮
│ System: Fedora 41 │ Uptime: 3 days │ Net: 2.1 GB down │ Disk: 87% full
╰──────────────────────╯

🔥 Severity   Diagnosis                                      Solution
🔥 10          Root partition 93% full (btrfs snapshots)     sudo btrfs balance start -dusage=5 /
🔥 9           CPU temperature 94°C                           Check cooling / clean dust
⚠️ 8           87 snap loop devices mounted                   sudo snap remove old revisions
⚠️ 8           NVIDIA GPU running hot (>80°C)                 Check fan curves in nvidia-settings
⚠️ 7           127 packages waiting for update                sudo dnf upgrade
⚠️ 7           Steam overlay eating CPU (>80%)                Disable Steam overlay in settings
ℹ️ 6           Baloo indexer running                          balooctl disable

Tip: Run 'why --update-rules' for the latest community rules.
```

### How it works

For advanced users and system administrators who want to understand what's happening under the hood.

#### Architecture

**why** is built around a declarative **rules engine** that correlates system metrics with known issues:

1. **Metrics collection** (~50ms) — Gathers system state from multiple sources
2. **Rule evaluation** (~100ms) — Evaluates 113+ rules against current metrics
3. **Correlation** (~10ms) — Links related findings (e.g., high CPU + specific process)
4. **Presentation** (~40ms) — Formats output with severity, solution, and context

Total execution time: **<200ms** (tracked via `WHY_BENCHMARK=1` or `RUST_LOG=debug`)

#### Data sources by command

| Command | Data Sources | What it checks |
|---------|--------------|----------------|
| `why` / `why all` | CPU, RAM, disk, network, processes, dmesg, journal | Full system scan: 113 rules evaluated |
| `why wifi` | NetworkManager (nmcli), /proc/net, kernel logs | Signal strength, connection drops, driver issues, regulatory domain |
| `why battery` | UPower, /sys/class/power_supply | Drain rate, charge cycles, health, power profiles |
| `why gpu` | nvidia-smi, rocm-smi, /sys/class/drm, /sys/class/hwmon | GPU vendor, driver version, memory usage, temperature, power state |
| `why gaming` | Steam logs, Proton compat_log.txt, processes (gamemoded, mangohud) | GameMode active, MangoHud, Proton crashes, Vulkan loader, GPU offloading |
| `why fan` / `why hot` | lm-sensors, /sys/class/thermal, /sys/class/hwmon | CPU/GPU temps, fan speeds, throttling |
| `why boot` | systemd-analyze, journalctl | Boot time breakdown, slow services (>5s warning, >15s critical) |
| `why check-deps` | Command availability (which) | Validates external tools: sensors, nvidia-smi, upower, nmcli, etc. |
| `why --snapshot` | All sources above + complete dmesg/journal history | Forensic JSON snapshot for bug reports |

#### Gaming-specific internals

`why gaming` performs specialized checks:

- **Steam detection**: Checks if Steam process is running (pgrep)
- **Proton failure analysis**: Parses `~/.steam/steam/logs/compat_log.txt` for ERROR/crash patterns
- **Vulkan loader**: Tests `vulkaninfo` availability (critical for modern games)
- **GPU offloading**: Detects NVIDIA Optimus (prime-run, NV_PRIME_RENDER_OFFLOAD)
- **Performance tools**: Verifies GameMode daemon, MangoHud presence
- **CS2 optimization**: Checks for `-vulkan +fps_max 0` launch options

**Important**: Gaming rules are filtered by default and only shown when running `why gaming` explicitly to avoid noise in general diagnostics.

#### GPU diagnostics internals

Multi-vendor GPU detection with vendor-specific optimizations:

**NVIDIA (nvidia-smi)**:
- Parses `nvidia-smi --query-gpu=...` for temperature, memory, utilization, power
- Detects driver version, CUDA version, GPU model
- Checks for power throttling, thermal throttling

**AMD (rocm-smi + sysfs)**:
- Primary: `rocm-smi` for telemetry (if available)
- Fallback: `/sys/class/drm/card*/device/hwmon/hwmon*/` for temps/power
- Detects AMDGPU driver, GPU model from sysfs

**Intel (sysfs)**:
- Reads `/sys/class/drm/card*/gt_*` for Intel Arc/Xe metrics
- Temperature from `/sys/class/hwmon/hwmon*/temp*_input`

**Caching**: GPU info cached for 5 seconds in TUI mode to avoid hammering vendor tools.

#### Rules engine format

Rules are declarative TOML (see `rules.toml`):

```toml
[[rule]]
name = "wifi_signal_weak"
trigger = "wifi_connected=true && wifi_signal<50"
message = "Wi-Fi signal weak (<50%)"
solution = "Move closer to router or check for interference"
severity = 7
```

Trigger DSL supports:
- Comparisons: `cpu>80`, `ram<1000`, `wifi_signal>=50`
- Booleans: `wifi_connected=true`, `nvidia_gpu=false`
- Conjunctions: `cpu>80 && ram>90`
- Process checks: `process_running=firefox`, `process_cpu>50 && process_name=steam`
- GPU checks: `gpu_vendor=nvidia`, `gpu_temp>80`, `gpu_memory_util>85`

#### Security features

- **Command injection prevention**: All external commands validated with strict alphanumeric-only input
- **Safe auto-fix whitelist**: Only harmless commands allowed (e.g., `balooctl`, `systemctl restart`), requires user confirmation
- **Path traversal protection**: User home dirs from trusted env vars ($HOME/$USERPROFILE)
- **No shell usage**: Direct command execution without shell intermediaries where possible

#### Performance profiling

Enable performance tracking for debugging:

```bash
WHY_BENCHMARK=1 why all
# Output: ⏱️  Execution time: 178ms

RUST_LOG=debug why all
# Output: ⏱️  Execution time: 182ms
#         ⚠️  Warning: Exceeded 200ms target (210ms)
```

### Contributing

The power of `why` comes from the community.
Found a new common issue? **Add a rule** → open a PR → become immortal.

**See [CONTRIBUTING.md](CONTRIBUTING.md) for the complete guide on writing rules.**

Quick start:
1. Edit `rules.toml` with your new rule
2. Run `cargo test test_rules_toml_is_valid` to validate
3. Open a PR — CI will automatically verify everything

### License

MIT © 2025 

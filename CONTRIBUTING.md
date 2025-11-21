# Contributing to Why

Thanks for your interest in contributing! The power of `why` comes from the community. Every rule you add helps thousands of Linux users diagnose their systems faster.

## 🎯 How to Contribute Rules

The easiest and most impactful way to contribute is by adding new diagnostic rules to `rules.toml`.

### Rule Structure

Each rule in `rules.toml` follows this format:

```toml
[[rule]]
name = "short-descriptive-name"
trigger = "condition"
message = "What's wrong (user-facing)"
solution = "How to fix it"
severity = 8
auto_fix = "command to run (optional)"
```

### Field Requirements

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | ✅ | Unique identifier (lowercase-with-dashes) |
| `trigger` | string | ✅ | Condition that activates this rule (see below) |
| `message` | string | ✅ | User-facing diagnosis (max 200 chars) |
| `solution` | string | ✅ | How to fix the issue (max 500 chars) |
| `severity` | integer | ✅ | Priority 1-10 (10=critical, 5=warning, 1=info) |
| `auto_fix` | string | ❌ | Optional safe command to auto-fix (whitelist only) |

### Trigger Syntax

Triggers use a simple DSL to detect system conditions:

#### CPU & Memory
- `cpu>80` — CPU usage above 80%
- `mem>90` — RAM usage above 90%
- `total_ram<4096` — Total RAM less than 4GB

#### Disk
- `disk>85` — Root partition above 85% full
- `filesystem=btrfs` — Root filesystem is btrfs
- `snap_loops>50` — More than 50 snap loop devices

#### Processes
- `process~chrome` — Process name contains "chrome"
- `process_count>200` — More than 200 processes running

#### Hardware
- `fan>3000` — Fan speed above 3000 RPM
- `temp>80` — Temperature above 80°C
- `gpu_temp>85` — GPU temperature above 85°C
- `gpu_vendor=nvidia` — GPU is NVIDIA

#### Gaming
- `steam_running=true` — Steam is running
- `proton_failures=true` — Proton errors detected
- `vulkan_loader_missing=true` — Vulkan not installed

#### Network
- `wifi_channels>3` — More than 3 Wi-Fi networks on same channel
- `wifi_signal<-70` — Wi-Fi signal weaker than -70 dBm

#### Multiple Conditions
- `cpu>80 && mem>90` — Both conditions must be true
- `gpu_temp>85 && gpu_vendor=nvidia` — NVIDIA GPU running hot

### Severity Guidelines

- **10**: System unusable (disk full, kernel panic imminent)
- **9**: Critical issue (95°C temps, RAM exhausted)
- **8**: Major problem (slow boot, driver issues)
- **7**: Important warning (80%+ disk, outdated packages)
- **6**: Moderate issue (background services eating CPU)
- **5**: Minor problem (unused packages, index bloat)
- **1-4**: Informational tips

### Auto-Fix Whitelist

For security, only these commands are allowed in `auto_fix`:
- `bluetoothctl power off`
- `bluetoothctl power on`
- `snap remove <package>`
- `flatpak uninstall <app>`
- `balooctl disable`
- `systemctl --user stop <service>`
- `docker image prune -f`

If your fix needs a different command, leave `auto_fix` empty and describe the manual steps in `solution`.

## 📝 Example Rules

### Simple Rule

```toml
[[rule]]
name = "high-cpu-usage"
trigger = "cpu>95"
message = "CPU usage critically high (>95%)"
solution = "Check 'why slow' for top consumers"
severity = 9
```

### Rule with Multiple Conditions

```toml
[[rule]]
name = "nvidia-gpu-overheating"
trigger = "gpu_temp>85 && gpu_vendor=nvidia"
message = "NVIDIA GPU running very hot (>85°C)"
solution = "Check fan curves in nvidia-settings or improve case airflow"
severity = 8
```

### Rule with Auto-Fix

```toml
[[rule]]
name = "baloo-indexer-running"
trigger = "process~baloo"
message = "Baloo file indexer consuming resources"
solution = "Disable with balooctl disable"
severity = 6
auto_fix = "balooctl disable"
```

### Gaming Rule

```toml
[[rule]]
name = "steam-overlay-cpu-hog"
trigger = "process~steamwebhelper && cpu>70"
message = "Steam overlay eating CPU (>70%)"
solution = "Disable Steam overlay in Settings > In-Game > Enable Steam Overlay"
severity = 7
```

## 🚀 Submission Process

1. **Fork** this repository
2. **Edit** `rules.toml` and add your rule(s)
3. **Test** locally: `cargo test test_rules_toml_is_valid`
4. **Commit** with clear message: `Add rule for [specific issue]`
5. **Open PR** — CI will automatically validate your changes

### Testing Your Rule Locally

```bash
# Validate syntax and constraints
cargo test test_rules_toml_is_valid

# Test that your rule triggers correctly (optional)
cargo run --release -- all
```

## ✅ CI Validation

When you open a PR, GitHub Actions will automatically:
- ✓ Validate `rules.toml` syntax
- ✓ Check all fields are present
- ✓ Verify severity is 1-10
- ✓ Ensure triggers parse correctly
- ✓ Confirm auto-fix commands are safe

If CI fails, check the error message — it will tell you exactly what's wrong.

## 💡 Rule Ideas

Need inspiration? Here are common issues that need rules:

### Missing Coverage
- AppArmor denials (check dmesg)
- PulseAudio vs PipeWire conflicts
- Wayland-specific issues (XWayland apps, screenshare)
- ZFS scrub errors
- LUKS decryption slow (cryptsetup benchmark)
- Snap/Flatpak runtime issues
- NVIDIA Wayland performance
- AMD GPU power management
- Intel Arc driver issues

### Distribution-Specific
- Fedora: SELinux denials, DNF metadata size
- Ubuntu: held-back packages, snap spam
- Arch: Partial upgrades, AUR package conflicts
- openSUSE: Zypper locks, btrfs snapshots

### Desktop Environment
- GNOME: Extension crashes, mutter memory leaks
- KDE: Baloo indexer, Akonadi database issues
- XFCE: Compositor tearing, panel crashes

## 🤝 Other Ways to Contribute

- **Bug Reports**: Open an issue with `why` output and system info
- **Documentation**: Improve README, add examples
- **Code**: Refactor, optimize, fix bugs
- **Testing**: Try on different distros, report findings

## 📜 Code of Conduct

- Be respectful and constructive
- Focus on technical merit
- Help newcomers learn
- No spam or self-promotion

## 🎓 Learning Resources

- [TOML Spec](https://toml.io/en/)
- [Rust Book](https://doc.rust-lang.org/book/)
- [Linux Performance Tools](https://www.brendangregg.com/linuxperf.html)

---

**Questions?** Open an issue or discussion. We're happy to help! 🚀

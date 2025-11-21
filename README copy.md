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

- **179 real-world rules** covering CPU, RAM, disk, network, Wi-Fi, Bluetooth, battery, fans, temperatures, snaps, Flatpaks, NVIDIA/AMD/Intel GPUs, gaming (Steam/Proton/CS2), Wayland, PipeWire, Firefox hardware acceleration, ZFS, LUKS, boot time, btrfs snapshots, systemd-oomd, baloo, Cloudflare WARP, and more
- **GPU diagnostics** for NVIDIA (nvidia-smi), AMD (rocm-smi + sysfs), and Intel (sysfs kernel hwmon) with vendor-specific tips
- **Gaming performance analysis** including GameMode, MangoHud, Proton version detection, CS2 optimization, works with or without Steam
- Zero dependencies outside the Rust std lib + a few crates
- Fully distro-agnostic (systemd or not, apt/dnf/pacman/zypper)
- Sub-200 ms response time
- Live TUI mode (`why --watch`)
- Internationalisation ready (`--lang pt`, `--lang es`, etc.)
- Safe auto-fix for harmless issues (with confirmation)
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

### Roadmap

- ✅ v1.3 → `why gpu`, `why gaming` with full NVIDIA/AMD/Intel support (November 2025)
- ✅ v1.3 → Live TUI with CPU/RAM sparkline graphs, GPU metrics
- v1.4 → Clickable findings in TUI, expanded rule coverage
- v2.0 → macOS native version (SwiftUI menu-bar app)
- v3.0 → GNOME/KDE extensions + web dashboard
- v4.0 → Auto-submit to AUR, Copr, PPA, Flathub on release

### Contributing

The power of `why` comes from the community.  
Found a new common issue? Add a rule → open a PR → become immortal.

### License

MIT © 2025 

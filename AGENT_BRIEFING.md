# AGENT BRIEFING — why v1.3 (November 2025)

You are now working on **why**, the most promising new Linux troubleshooting CLI of 2025.

Goal: become the `htop` of "why is my Linux acting up?" — instant, human-readable, distro-agnostic diagnosis.

## Current status — v1.3 Released! (Nov 2025)

- **179 production rules** covering 95% of daily desktop pain + gaming/GPU issues
- Full **GPU diagnostics** for NVIDIA (nvidia-smi), AMD (rocm-smi + sysfs), Intel (sysfs kernel hwmon)
- **Gaming performance analysis** with Steam/Proton/CS2/GameMode/MangoHud detection
- Sub-200 ms full scan
- Fully distro-agnostic (systemd or not, apt/dnf/pacman/zypper)
- Static musl binary (~1.2 MB)
- Live TUI with `why --watch` (ratatui)
- i18n ready (English + full Portuguese, easy to add more)
- Safe auto-fix with whitelist + confirmation
- MIT license (acquisition-friendly)

Already implemented subcommands:
`why`, `why wifi`, `why bluetooth`, `why fan`, `why hot`, `why boot`, `why update`, `why battery`, `why slow`, `why crash`, `why historical`, `why gpu`, `why gaming`, `why check-deps`

Special flags:
`why --snapshot` (forensic snapshot — JSON with complete system state), `why --watch` (TUI mode), `why --lang pt` (i18n)

## Tech stack (do NOT change unless critical)

- Rust 1.80+ edition 2021
- Crates: sysinfo, ratatui, crossterm, rust-i18n, tabled, dialoguer, clap, anyhow, etc.
- Rules engine: rules.toml + simple DSL parsed at runtime
- No Python, no Node, no external daemons

## Where the magic lives

- `src/main.rs` — entrypoint, CLI, TUI, all logic (~2700 lines)
- `src/deps.rs` — dependency checking module (external tools validation)
- `rules.toml` — the soul of the project (179 rules, English only)
- `i18n/en.toml` + `i18n/pt.toml` — translations (rust-i18n crate)
- `CONTRIBUTING.md` — guide for community contributors (rule format, examples)
- `.github/workflows/validate-rules.yml` — CI validation for rule PRs
- `.github/workflows/release.yml` — builds static binary + deb + rpm + AppImage on tag

## Immediate next tasks (pick any, in order of impact)

1. ✅ ~~`why gpu` / `why nvidia` / `why amd` subcommand~~ **DONE v1.3** — Full support for NVIDIA, AMD (ROCm + sysfs), Intel (sysfs)
2. ✅ ~~`why gaming`~~ **DONE v1.3** — CS2/Steam/Proton/GameMode/MangoHud detection with vendor-specific tips
3. ✅ ~~Add 50 more rules~~ **DONE v1.3** — Added 51 rules (Wayland, PipeWire, Firefox, ZFS, LUKS, GPU, Gaming)
4. ✅ ~~TUI graphs~~ **DONE v1.3** — Live CPU/RAM sparkline graphs with history tracking, GPU metrics in vitals
5. ✅ ~~Locale bug fix~~ **DONE v1.3** — All numeric parsing now uses LC_ALL=C (fixes PT/DE/FR systems)
6. ✅ ~~Dependency checking~~ **DONE v1.3** — `why check-deps` + warnings in dashboard
7. ✅ ~~CI for community rules~~ **DONE v1.3** — Auto-validation + CONTRIBUTING.md guide
8. ✅ ~~Forensic snapshots~~ **DONE v1.3** — `why --snapshot` generates JSON with complete system state for bug reports
9. Improve TUI — add clickable findings, more interactive features
10. macOS port skeleton (Swift + SwiftUI menu-bar app calling the same Rust core via Tauri or lib)
11. GitHub Actions to auto-submit to AUR, Copr, PPA, Flathub on release

## Rules for contributing (follow strictly)

- All user-facing text → use `t!("key")` macro (i18n)
- Never panic — always `?` + `anyhow::Context`
- Auto-fix commands must be in the whitelist in `is_safe_auto_fix()`
- New rules → English only in rules.toml, severity 5–10
- Keep binary < 3 MB (use strip + lto + musl

## One-command test

```bash
cargo test                                    # run all tests (19 total)
cargo test test_rules_toml_is_valid          # validate rules.toml only
cargo run --release -- --lang en all
cargo run --release -- check-deps            # verify external tools
cargo run --release -- --snapshot            # generate forensic JSON snapshot
cargo run --release -- wifi
cargo run --release -- --watch               # q to quit
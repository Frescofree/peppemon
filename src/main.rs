use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Bar, BarChart, BarGroup, Block, BorderType, Borders, Clear, Gauge, Paragraph, Row,
        Sparkline, Table,
    },
    Frame,
};
use std::{
    collections::VecDeque,
    fs,
    io::{self, stdout},
    time::{Duration, Instant, SystemTime},
};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, RefreshKind, System};

const HISTORY_LEN: usize = 60;
const TICK_RATE: Duration = Duration::from_millis(1000);
const ANIM_TICK: Duration = Duration::from_millis(16);
const MAX_PARTICLES: usize = 100;
const CYCLE_DURATION: Duration = Duration::from_secs(45);
const LIGHTNING_FLASH_FRAMES: u8 = 18;
const LIGHTNING_MIN_INTERVAL_SECS: u64 = 3;
const LIGHTNING_MAX_INTERVAL_SECS: u64 = 8;

// 3-column bitmask font for clock digits (0-9) + colon.
// Each glyph is 5 rows; bits 2,1,0 = left, center, right columns.
// Rendered doubled ("██" per pixel) → 6 chars wide × 5 rows per glyph.
const CLOCK_GLYPHS: [[u8; 5]; 11] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b001, 0b001, 0b001], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
    [0b000, 0b010, 0b000, 0b010, 0b000], // : (colon)
];

// ── Enums ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum ActiveTab {
    Overview,
    Processes,
    CpuDetail,
}

#[derive(Clone, Copy, PartialEq)]
enum SortMode {
    Cpu,
    Memory,
    Pid,
}

#[derive(Clone, Copy, PartialEq)]
enum WeatherEffect {
    Rain,
    Snow,
    Lightning,
    Seasons,
}

#[derive(Clone, Copy, PartialEq)]
enum CycleMode {
    Auto,
    Pinned,
}

#[derive(Clone, Copy, PartialEq)]
enum SeasonMode {
    AutoRotate,
    RealSeason,
    NatureBlend,
}

#[derive(Clone, Copy, PartialEq)]
enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

#[derive(Clone, Copy, PartialEq)]
enum SettingsRow {
    Effect,
    CycleMode,
    SeasonMode,
    Intensity,
    Speed,
}

impl SettingsRow {
    fn next(self) -> Self {
        match self {
            Self::Effect => Self::CycleMode,
            Self::CycleMode => Self::SeasonMode,
            Self::SeasonMode => Self::Intensity,
            Self::Intensity => Self::Speed,
            Self::Speed => Self::Effect,
        }
    }
    fn prev(self) -> Self {
        match self {
            Self::Effect => Self::Speed,
            Self::CycleMode => Self::Effect,
            Self::SeasonMode => Self::CycleMode,
            Self::Intensity => Self::SeasonMode,
            Self::Speed => Self::Intensity,
        }
    }
}

// ── Particle system ───────────────────────────────────────────────────────

struct Particle {
    x: f32,
    y: f32,
    symbol: &'static str,
    fg: Color,
    speed_y: f32,
    drift_x: f32,
    life: u16,
}

struct LightningState {
    active: bool,
    frames_remaining: u8,
    bolt_segments: Vec<(u16, u16)>,
    next_strike: Duration,
    timer: Instant,
}

struct ParticleSystem {
    particles: Vec<Particle>,
    rng: fastrand::Rng,
    effect: WeatherEffect,
    cycle_mode: CycleMode,
    season_mode: SeasonMode,
    intensity: u8,
    speed: u8,
    current_season: Season,
    season_timer: Instant,
    cycle_timer: Instant,
    lightning: LightningState,
    enabled: bool,
    frame_count: u32,
    transition_cooldown: u8,
}

// ── Snapshots ──────────────────────────────────────────────────────────────

struct NetSnapshot {
    rx_bytes: u64,
    tx_bytes: u64,
    time: Instant,
}

struct DiskSnapshot {
    read_bytes: u64,
    write_bytes: u64,
    time: Instant,
}

// ── App ────────────────────────────────────────────────────────────────────

struct App {
    sys: System,
    cpu_history: Vec<VecDeque<u64>>,
    mem_history: VecDeque<u64>,
    net_rx_history: VecDeque<u64>,
    net_tx_history: VecDeque<u64>,
    disk_read_history: VecDeque<u64>,
    disk_write_history: VecDeque<u64>,
    last_net: Option<NetSnapshot>,
    last_disk: Option<DiskSnapshot>,
    disk_read_rate: f64,
    disk_write_rate: f64,
    net_rx_rate: f64,
    net_tx_rate: f64,
    should_quit: bool,
    // v0.2 additions
    active_tab: ActiveTab,
    sort_mode: SortMode,
    filter_mode: bool,
    filter_text: String,
    process_scroll: usize,
    show_help: bool,
    cpu_temp: Option<f64>,
    cpu_freq_avg: Option<f64>,
    // v0.3 background effects
    show_settings: bool,
    settings_row: SettingsRow,
    particles: ParticleSystem,
}

impl App {
    fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything())
                .with_processes(ProcessRefreshKind::everything()),
        );
        let cpu_count = sys.cpus().len().max(1);
        let cpu_history = (0..cpu_count)
            .map(|_| {
                let mut q = VecDeque::with_capacity(HISTORY_LEN);
                q.push_back(0);
                q
            })
            .collect();

        let mut mem_history = VecDeque::with_capacity(HISTORY_LEN);
        mem_history.push_back(0);

        let mut net_rx_history = VecDeque::with_capacity(HISTORY_LEN);
        net_rx_history.push_back(0);
        let mut net_tx_history = VecDeque::with_capacity(HISTORY_LEN);
        net_tx_history.push_back(0);
        let mut disk_read_history = VecDeque::with_capacity(HISTORY_LEN);
        disk_read_history.push_back(0);
        let mut disk_write_history = VecDeque::with_capacity(HISTORY_LEN);
        disk_write_history.push_back(0);

        App {
            sys,
            cpu_history,
            mem_history,
            net_rx_history,
            net_tx_history,
            disk_read_history,
            disk_write_history,
            last_net: None,
            last_disk: None,
            disk_read_rate: 0.0,
            disk_write_rate: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            should_quit: false,
            active_tab: ActiveTab::Overview,
            sort_mode: SortMode::Cpu,
            filter_mode: false,
            filter_text: String::new(),
            process_scroll: 0,
            show_help: false,
            cpu_temp: None,
            cpu_freq_avg: None,
            show_settings: false,
            settings_row: SettingsRow::Effect,
            particles: ParticleSystem::new(),
        }
    }

    fn tick(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        // CPU history
        for (i, cpu) in self.sys.cpus().iter().enumerate() {
            if let Some(hist) = self.cpu_history.get_mut(i) {
                if hist.len() >= HISTORY_LEN {
                    hist.pop_front();
                }
                hist.push_back(cpu.cpu_usage() as u64);
            }
        }

        // Memory history
        let mem_pct = if self.sys.total_memory() > 0 {
            (self.sys.used_memory() as f64 / self.sys.total_memory() as f64 * 100.0) as u64
        } else {
            0
        };
        if self.mem_history.len() >= HISTORY_LEN {
            self.mem_history.pop_front();
        }
        self.mem_history.push_back(mem_pct);

        // Network rates from /proc/net/dev
        self.update_net();

        // Disk I/O rates from /proc/diskstats
        self.update_disk();

        // CPU sensors
        self.cpu_temp = read_cpu_temp();
        self.cpu_freq_avg = read_cpu_freq();
    }

    fn update_net(&mut self) {
        let (rx, tx) = read_net_bytes();
        let now = Instant::now();
        if let Some(prev) = &self.last_net {
            let dt = now.duration_since(prev.time).as_secs_f64();
            if dt > 0.0 {
                self.net_rx_rate = (rx.saturating_sub(prev.rx_bytes)) as f64 / dt;
                self.net_tx_rate = (tx.saturating_sub(prev.tx_bytes)) as f64 / dt;
            }
        }
        if self.net_rx_history.len() >= HISTORY_LEN {
            self.net_rx_history.pop_front();
        }
        if self.net_tx_history.len() >= HISTORY_LEN {
            self.net_tx_history.pop_front();
        }
        self.net_rx_history.push_back(self.net_rx_rate as u64);
        self.net_tx_history.push_back(self.net_tx_rate as u64);

        self.last_net = Some(NetSnapshot {
            rx_bytes: rx,
            tx_bytes: tx,
            time: now,
        });
    }

    fn update_disk(&mut self) {
        let (read_b, write_b) = read_disk_bytes();
        let now = Instant::now();
        if let Some(prev) = &self.last_disk {
            let dt = now.duration_since(prev.time).as_secs_f64();
            if dt > 0.0 {
                self.disk_read_rate = (read_b.saturating_sub(prev.read_bytes)) as f64 / dt;
                self.disk_write_rate = (write_b.saturating_sub(prev.write_bytes)) as f64 / dt;
            }
        }
        if self.disk_read_history.len() >= HISTORY_LEN {
            self.disk_read_history.pop_front();
        }
        if self.disk_write_history.len() >= HISTORY_LEN {
            self.disk_write_history.pop_front();
        }
        self.disk_read_history.push_back(self.disk_read_rate as u64);
        self.disk_write_history
            .push_back(self.disk_write_rate as u64);

        self.last_disk = Some(DiskSnapshot {
            read_bytes: read_b,
            write_bytes: write_b,
            time: now,
        });
    }
}

// ── Sensor readers ─────────────────────────────────────────────────────────
// Linux-primary with cross-platform fallbacks

#[cfg(target_os = "linux")]
fn read_net_bytes() -> (u64, u64) {
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    if let Ok(content) = fs::read_to_string("/proc/net/dev") {
        for line in content.lines().skip(2) {
            let trimmed = line.trim();
            let Some((iface, stats)) = trimmed.split_once(':') else {
                continue;
            };
            if iface.trim() == "lo" {
                continue;
            }
            let parts: Vec<&str> = stats.split_whitespace().collect();
            if parts.len() >= 9 {
                rx_total += parts[0].parse::<u64>().unwrap_or(0);
                tx_total += parts[8].parse::<u64>().unwrap_or(0);
            }
        }
    }
    (rx_total, tx_total)
}

#[cfg(not(target_os = "linux"))]
fn read_net_bytes() -> (u64, u64) {
    // sysinfo Networks could be used here; for now return zero (rates will show 0)
    (0, 0)
}

#[cfg(target_os = "linux")]
fn read_disk_bytes() -> (u64, u64) {
    let mut read_total = 0u64;
    let mut write_total = 0u64;
    if let Ok(content) = fs::read_to_string("/proc/diskstats") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 14 {
                let name = parts[2];
                if name.starts_with("loop") || name.starts_with("dm-") {
                    continue;
                }
                let is_partition = if name.starts_with("nvme") {
                    name.rfind('p').is_some_and(|pos| {
                        pos > 0
                            && !name[pos + 1..].is_empty()
                            && name[pos + 1..].chars().all(|c| c.is_ascii_digit())
                    })
                } else {
                    name.len() > 3 && name[3..].chars().all(|c| c.is_ascii_digit())
                };
                if is_partition {
                    continue;
                }
                read_total += parts[5].parse::<u64>().unwrap_or(0) * 512;
                write_total += parts[9].parse::<u64>().unwrap_or(0) * 512;
            }
        }
    }
    (read_total, write_total)
}

#[cfg(not(target_os = "linux"))]
fn read_disk_bytes() -> (u64, u64) {
    (0, 0)
}

/// Try hwmon (k10temp / coretemp), fall back to thermal_zone0
#[cfg(target_os = "linux")]
fn read_cpu_temp() -> Option<f64> {
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(name) = fs::read_to_string(path.join("name")) {
                let name = name.trim();
                if name == "k10temp" || name == "coretemp" {
                    if let Ok(raw) = fs::read_to_string(path.join("temp1_input")) {
                        if let Ok(millideg) = raw.trim().parse::<f64>() {
                            return Some(millideg / 1000.0);
                        }
                    }
                }
            }
        }
    }
    if let Ok(raw) = fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
        if let Ok(millideg) = raw.trim().parse::<f64>() {
            return Some(millideg / 1000.0);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_temp() -> Option<f64> {
    // No cross-platform temp reader without sysinfo Components; return None
    None
}

/// Average of all cores' scaling_cur_freq (kHz → MHz)
#[cfg(target_os = "linux")]
fn read_cpu_freq() -> Option<f64> {
    let mut total = 0u64;
    let mut count = 0u32;
    if let Ok(entries) = fs::read_dir("/sys/devices/system/cpu") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("cpu")
                && name_str.len() > 3
                && name_str[3..].chars().all(|c| c.is_ascii_digit())
            {
                let freq_path = entry.path().join("cpufreq/scaling_cur_freq");
                if let Ok(raw) = fs::read_to_string(&freq_path) {
                    if let Ok(khz) = raw.trim().parse::<u64>() {
                        total += khz;
                        count += 1;
                    }
                }
            }
        }
    }
    if count > 0 {
        Some(total as f64 / count as f64 / 1000.0)
    } else {
        None
    }
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_freq() -> Option<f64> {
    None
}

fn read_system_info() -> Vec<(String, String)> {
    let mut info = Vec::new();
    // Cross-platform via sysinfo
    info.push((
        "Kernel".into(),
        System::kernel_version().unwrap_or_default(),
    ));
    info.push(("Host".into(), System::host_name().unwrap_or_default()));

    let uptime = System::uptime();
    let hours = uptime / 3600;
    let mins = (uptime % 3600) / 60;
    info.push(("Uptime".into(), format!("{}h {}m", hours, mins)));

    // Linux-specific extras (silently skipped on other OSes)
    #[cfg(target_os = "linux")]
    {
        if let Ok(gov) =
            fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        {
            info.push(("Governor".into(), gov.trim().to_string()));
        }
        if let Ok(s) = fs::read_to_string("/proc/sys/vm/swappiness") {
            info.push(("Swappiness".into(), s.trim().to_string()));
        }
        if let Ok(cc) = fs::read_to_string("/proc/sys/net/ipv4/tcp_congestion_control") {
            info.push(("TCP CC".into(), cc.trim().to_string()));
        }
        if let Ok(la) = fs::read_to_string("/proc/loadavg") {
            let parts: Vec<&str> = la.split_whitespace().collect();
            if parts.len() >= 3 {
                info.push((
                    "Load".into(),
                    format!("{} {} {}", parts[0], parts[1], parts[2]),
                ));
            }
        }
        if let Ok(stat) = fs::read_to_string("/proc/stat") {
            for line in stat.lines() {
                if let Some(rest) = line.strip_prefix("ctxt ") {
                    let val: u64 = rest.trim().parse().unwrap_or(0);
                    info.push(("Ctx Sw".into(), format!("{}", val)));
                    break;
                }
            }
        }
    }
    info
}

// ── Season detection ──────────────────────────────────────────────────────

/// Pure-arithmetic month from epoch using Howard Hinnant's civil_from_days.
fn detect_season() -> Season {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    match m {
        3..=5 => Season::Spring,
        6..=8 => Season::Summer,
        9..=11 => Season::Autumn,
        _ => Season::Winter,
    }
}

// ── Local time ───────────────────────────────────────────────────────────

/// Returns (hour, minute, second) in the system's local timezone.
#[cfg(unix)]
fn local_hm() -> (u8, u8, u8) {
    // Safe FFI: localtime_r writes into our stack buffer and respects TZ.
    extern "C" {
        fn localtime_r(timep: *const i64, result: *mut i32) -> *mut i32;
    }
    let epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut buf = [0i32; 16]; // oversized to cover any struct tm layout
    unsafe {
        localtime_r(&epoch, buf.as_mut_ptr());
    }
    (buf[2] as u8, buf[1] as u8, buf[0] as u8)
}

/// Fallback: UTC arithmetic (no timezone) for non-Unix platforms.
#[cfg(not(unix))]
fn local_hm() -> (u8, u8, u8) {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let day_secs = (secs % 86400) as u32;
    ((day_secs / 3600) as u8, ((day_secs % 3600) / 60) as u8, (day_secs % 60) as u8)
}

// ── Particle system impl ─────────────────────────────────────────────────

impl ParticleSystem {
    fn new() -> Self {
        ParticleSystem {
            particles: Vec::with_capacity(MAX_PARTICLES),
            rng: fastrand::Rng::new(),
            effect: WeatherEffect::Rain,
            cycle_mode: CycleMode::Auto,
            season_mode: SeasonMode::RealSeason,
            intensity: 3,
            speed: 5,
            current_season: detect_season(),
            season_timer: Instant::now(),
            cycle_timer: Instant::now(),
            lightning: LightningState {
                active: false,
                frames_remaining: 0,
                bolt_segments: Vec::new(),
                next_strike: Duration::from_secs(5),
                timer: Instant::now(),
            },
            enabled: true,
            frame_count: 0,
            transition_cooldown: 0,
        }
    }

    fn update(&mut self, width: u16, height: u16, dt: f32) {
        if !self.enabled {
            return;
        }
        self.frame_count = self.frame_count.wrapping_add(1);

        // Auto-cycle effects
        if self.cycle_mode == CycleMode::Auto && self.cycle_timer.elapsed() >= CYCLE_DURATION {
            self.effect = match self.effect {
                WeatherEffect::Rain => WeatherEffect::Snow,
                WeatherEffect::Snow => WeatherEffect::Lightning,
                WeatherEffect::Lightning => WeatherEffect::Seasons,
                WeatherEffect::Seasons => WeatherEffect::Rain,
            };
            self.transition_cooldown = 30;
            self.cycle_timer = Instant::now();
        }

        // Season auto-rotate (every 15s)
        if self.effect == WeatherEffect::Seasons && self.season_mode == SeasonMode::AutoRotate {
            if self.season_timer.elapsed() >= Duration::from_secs(15) {
                self.current_season = match self.current_season {
                    Season::Spring => Season::Summer,
                    Season::Summer => Season::Autumn,
                    Season::Autumn => Season::Winter,
                    Season::Winter => Season::Spring,
                };
                self.season_timer = Instant::now();
            }
        }

        // Speed multiplier: linear ramp from 0.2 (speed=1) to 3.0 (speed=10)
        let speed_mult = 0.2 + (self.speed as f32 - 1.0) * (2.8 / 9.0);

        // Delta-time factor: normalized so 1.0 = old 100ms rate
        let dt_factor = dt * 10.0;

        // Move existing particles
        let w = width as f32;
        let h = height as f32;
        self.particles.retain_mut(|p| {
            p.y += p.speed_y * speed_mult * dt_factor;
            p.x += p.drift_x * speed_mult * dt_factor;
            p.life = p.life.saturating_sub(1);
            p.y < h + 1.0 && p.x >= -1.0 && p.x < w + 1.0 && p.life > 0
        });

        // Transition cooldown: drain old particles before spawning new effect
        if self.transition_cooldown > 0 {
            self.transition_cooldown -= 1;
            return;
        }

        // Spawn throttle: only spawn every 6th frame to keep same density at 6x frame rate
        if self.frame_count % 6 != 0 {
            return;
        }

        // Spawn new particles
        let spawn_count = self.intensity as usize;
        match self.effect {
            WeatherEffect::Rain => self.spawn_rain(width, spawn_count),
            WeatherEffect::Snow => self.spawn_snow(width, spawn_count),
            WeatherEffect::Lightning => {
                self.spawn_rain(width, spawn_count);
                self.update_lightning(width, height);
            }
            WeatherEffect::Seasons => self.spawn_season(width, height, spawn_count),
        }
    }

    fn spawn_rain(&mut self, width: u16, count: usize) {
        for _ in 0..count {
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            let heavy = self.rng.bool();
            let (symbol, fg) = if heavy {
                let syms: &[&str] = &["│", "|"];
                (
                    syms[self.rng.usize(..syms.len())],
                    if self.rng.bool() {
                        Color::Rgb(40, 60, 90) // dim blue
                    } else {
                        Color::Rgb(50, 80, 100) // dim cyan
                    },
                )
            } else {
                ("·", Color::Rgb(50, 50, 50)) // very dim mist
            };
            let has_wind = self.rng.u8(..) < 30;
            self.particles.push(Particle {
                x: self.rng.f32() * width as f32,
                y: -(self.rng.f32() * 3.0),
                symbol: if has_wind && heavy { "/" } else { symbol },
                fg,
                speed_y: if heavy {
                    0.8 + self.rng.f32() * 0.6
                } else {
                    0.5 + self.rng.f32() * 0.3
                },
                drift_x: if has_wind {
                    0.1 + self.rng.f32() * 0.1
                } else {
                    0.0
                },
                life: 1200,
            });
        }
    }

    fn spawn_snow(&mut self, width: u16, count: usize) {
        let fc = self.frame_count;
        for _ in 0..count {
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            let foreground = self.rng.bool();
            let syms: &[&str] = &["*", "·", "•", "."];
            let symbol = syms[self.rng.usize(..syms.len())];
            let fg = if foreground {
                Color::Rgb(120, 120, 130) // soft white
            } else {
                Color::Rgb(70, 70, 80) // dim gray
            };
            let seed = self.rng.f32() * 100.0;
            self.particles.push(Particle {
                x: self.rng.f32() * width as f32,
                y: -(self.rng.f32() * 2.0),
                symbol,
                fg,
                speed_y: if foreground {
                    0.3 + self.rng.f32() * 0.2
                } else {
                    0.15 + self.rng.f32() * 0.15
                },
                drift_x: (seed + fc as f32 * 0.1).sin() * 0.3,
                life: 1800,
            });
        }
    }

    fn update_lightning(&mut self, width: u16, height: u16) {
        if self.lightning.active {
            self.lightning.frames_remaining = self.lightning.frames_remaining.saturating_sub(1);
            if self.lightning.frames_remaining == 0 {
                self.lightning.active = false;
            }
        } else if self.lightning.timer.elapsed() >= self.lightning.next_strike {
            self.lightning.active = true;
            self.lightning.frames_remaining = LIGHTNING_FLASH_FRAMES;
            let bolt_x = self.rng.u16(2..width.saturating_sub(2).max(3));

            self.lightning.bolt_segments.clear();
            let mut x = bolt_x;
            for y in 0..height {
                self.lightning.bolt_segments.push((x, y));
                match self.rng.u8(..3) {
                    0 => x = x.saturating_sub(1),
                    1 => x = (x + 1).min(width.saturating_sub(1)),
                    _ => {}
                }
                // 30% chance of a branch segment
                if self.rng.u8(..10) < 3 {
                    let bx = if self.rng.bool() {
                        x.saturating_sub(1)
                    } else {
                        (x + 1).min(width.saturating_sub(1))
                    };
                    self.lightning.bolt_segments.push((bx, y));
                }
            }

            let range = LIGHTNING_MAX_INTERVAL_SECS - LIGHTNING_MIN_INTERVAL_SECS;
            self.lightning.next_strike =
                Duration::from_secs(LIGHTNING_MIN_INTERVAL_SECS + self.rng.u64(..=range));
            self.lightning.timer = Instant::now();
        }
    }

    fn spawn_season(&mut self, width: u16, height: u16, count: usize) {
        let fc = self.frame_count;
        for _ in 0..count {
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            let season = match self.season_mode {
                SeasonMode::RealSeason => detect_season(),
                SeasonMode::AutoRotate => self.current_season,
                SeasonMode::NatureBlend => match self.rng.u8(..4) {
                    0 => Season::Spring,
                    1 => Season::Summer,
                    2 => Season::Autumn,
                    _ => Season::Winter,
                },
            };
            match season {
                Season::Spring => {
                    let syms: &[&str] = &["*", ".", "·", "'"];
                    let colors: &[Color] = &[
                        Color::Rgb(120, 60, 80),  // muted rose
                        Color::Rgb(140, 80, 100), // soft magenta
                        Color::Rgb(100, 70, 75),  // dusty pink
                    ];
                    let seed = self.rng.f32() * 10.0;
                    self.particles.push(Particle {
                        x: self.rng.f32() * width as f32,
                        y: -(self.rng.f32() * 2.0),
                        symbol: syms[self.rng.usize(..syms.len())],
                        fg: colors[self.rng.usize(..colors.len())],
                        speed_y: 0.15 + self.rng.f32() * 0.2,
                        drift_x: (fc as f32 * 0.08 + seed).sin() * 0.25,
                        life: 1500,
                    });
                }
                Season::Summer => {
                    // Fireflies in the lower 40%, varied warm colors & brightness
                    let syms: &[&str] = &[".", "·", "°", "*"];
                    let colors: &[Color] = &[
                        Color::Rgb(255, 200, 60),  // bright gold
                        Color::Rgb(200, 160, 40),  // warm amber
                        Color::Rgb(255, 180, 50),  // orange-gold
                        Color::Rgb(140, 110, 30),  // dim ember
                        Color::Rgb(180, 140, 35),  // muted amber
                        Color::Rgb(100, 80, 20),   // faint glow
                    ];
                    let h = height as f32;
                    self.particles.push(Particle {
                        x: self.rng.f32() * width as f32,
                        y: h * (0.6 + self.rng.f32() * 0.38),
                        symbol: syms[self.rng.usize(..syms.len())],
                        fg: colors[self.rng.usize(..colors.len())],
                        speed_y: -0.05 + self.rng.f32() * 0.1,
                        drift_x: (self.rng.f32() - 0.5) * 0.3,
                        life: 120 + self.rng.u16(..210),
                    });
                }
                Season::Autumn => {
                    let syms: &[&str] = &["~", "}", "{", "\\", "/", "_"];
                    let colors: &[Color] = &[
                        Color::Rgb(130, 70, 0),  // dim orange
                        Color::Rgb(110, 55, 15), // muted brown
                        Color::Rgb(100, 40, 30), // dark rust
                        Color::Rgb(120, 90, 20), // faded gold
                    ];
                    let seed = self.rng.f32() * 10.0;
                    self.particles.push(Particle {
                        x: self.rng.f32() * width as f32,
                        y: -(self.rng.f32() * 2.0),
                        symbol: syms[self.rng.usize(..syms.len())],
                        fg: colors[self.rng.usize(..colors.len())],
                        speed_y: 0.3 + self.rng.f32() * 0.5,
                        drift_x: (fc as f32 * 0.12 + seed).sin() * 0.5,
                        life: 1200,
                    });
                }
                Season::Winter => {
                    let syms: &[&str] = &["*", ".", "·", "°", "+"];
                    let foreground = self.rng.bool();
                    let fg = if foreground {
                        Color::Rgb(100, 100, 110) // soft white
                    } else if self.rng.bool() {
                        Color::Rgb(70, 85, 95) // dim ice-blue
                    } else {
                        Color::Rgb(55, 55, 60) // faint gray
                    };
                    let seed = self.rng.f32() * 100.0;
                    let near_bottom = self.rng.f32();
                    self.particles.push(Particle {
                        x: self.rng.f32() * width as f32,
                        y: -(self.rng.f32() * 2.0),
                        symbol: syms[self.rng.usize(..syms.len())],
                        fg,
                        speed_y: if foreground {
                            0.25 + self.rng.f32() * 0.2
                        } else {
                            0.1 + self.rng.f32() * 0.15
                        } * if near_bottom > 0.8 { 0.5 } else { 1.0 },
                        drift_x: (seed + fc as f32 * 0.05).sin() * 0.2,
                        life: 1800,
                    });
                }
            }
        }
    }
}

fn format_bytes(bytes: f64) -> String {
    if bytes >= 1_073_741_824.0 {
        format!("{:.1} GB/s", bytes / 1_073_741_824.0)
    } else if bytes >= 1_048_576.0 {
        format!("{:.1} MB/s", bytes / 1_048_576.0)
    } else if bytes >= 1024.0 {
        format!("{:.1} KB/s", bytes / 1024.0)
    } else {
        format!("{:.0} B/s", bytes)
    }
}

fn sort_label(mode: SortMode) -> &'static str {
    match mode {
        SortMode::Cpu => "CPU",
        SortMode::Memory => "Memory",
        SortMode::Pid => "PID",
    }
}

// ── UI dispatch ────────────────────────────────────────────────────────────

fn ui(frame: &mut Frame, app: &App) {
    // Layer 1: widgets first (fill the screen)
    match app.active_tab {
        ActiveTab::Overview => ui_overview(frame, app),
        ActiveTab::Processes => ui_processes_tab(frame, app),
        ActiveTab::CpuDetail => ui_cpu_detail(frame, app),
    }
    // Layer 0.5: clock digits — only into empty cells, behind particles
    if !app.show_help && !app.show_settings {
        render_clock(frame);
    }
    // Layer 0: particles — only into empty cells so data is never obscured
    render_particles(frame, &app.particles);
    // Layer 2: overlays
    if app.show_help {
        render_help_overlay(frame);
    }
    if app.show_settings {
        render_settings_overlay(frame, app);
    }
}

fn render_clock(frame: &mut Frame) {
    let (h, m, s) = local_hm();
    let colon_visible = s % 2 == 0;
    let colon_idx: usize = if colon_visible { 10 } else { usize::MAX };

    // Glyph sequence: H tens, H ones, colon, M tens, M ones
    let digits: [usize; 5] = [
        (h / 10) as usize,
        (h % 10) as usize,
        10, // colon slot
        (m / 10) as usize,
        (m % 10) as usize,
    ];

    // Each glyph: 6 chars wide (3 columns × 2 cells). Gaps: 2 chars between glyphs.
    // Total: 5×6 + 4×2 = 38 columns, 5 rows tall.
    let total_w: u16 = 38;
    let total_h: u16 = 5;

    let buf = frame.buffer_mut();
    let area = *buf.area();
    if area.width < total_w + 4 || area.height < total_h + 4 {
        return; // terminal too small
    }

    let ox = (area.width - total_w) / 2;
    let oy = 1;
    let fg_color = Color::Rgb(70, 80, 130);
    let bg_color = Color::Rgb(22, 24, 40);

    for (gi, &idx) in digits.iter().enumerate() {
        // Skip colon when it should be invisible (blink off)
        if gi == 2 && idx != colon_idx {
            continue;
        }
        if idx >= CLOCK_GLYPHS.len() {
            continue;
        }
        let glyph = &CLOCK_GLYPHS[idx];
        let gx = ox + (gi as u16) * 8; // 6 char glyph + 2 char gap

        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..3u16 {
                if bits & (1 << (2 - col)) != 0 {
                    let cx = gx + col * 2;
                    let cy = oy + row as u16;
                    // Write two cells ("██") for each set pixel
                    for dx in 0..2u16 {
                        let x = cx + dx;
                        let y = cy;
                        if x < area.width && y < area.height {
                            if let Some(cell) = buf.cell_mut((x, y)) {
                                if cell.symbol() == " " {
                                    cell.set_symbol("█");
                                    cell.set_fg(fg_color);
                                } else {
                                    // Glow behind existing widget text
                                    cell.set_bg(bg_color);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_particles(frame: &mut Frame, ps: &ParticleSystem) {
    if !ps.enabled {
        return;
    }
    let buf = frame.buffer_mut();
    let area = *buf.area();

    // Lightning flash: subtle tint only on empty cells
    if ps.lightning.active && ps.effect == WeatherEffect::Lightning {
        let tint = match ps.lightning.frames_remaining {
            13..=18 => Some(Color::Rgb(15, 15, 30)),
            7..=12 => Some(Color::Rgb(30, 30, 55)),
            1..=6 => Some(Color::Rgb(18, 18, 35)),
            _ => None,
        };
        if let Some(bg) = tint {
            for y in area.y..area.y + area.height {
                for x in area.x..area.x + area.width {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        if cell.symbol() == " " {
                            cell.set_bg(bg);
                        }
                    }
                }
            }
        }
        // Draw bolt segments only into empty cells
        if ps.lightning.frames_remaining >= 12 {
            for &(bx, by) in &ps.lightning.bolt_segments {
                if bx < area.width && by < area.height {
                    if let Some(cell) = buf.cell_mut((bx, by)) {
                        if cell.symbol() == " " {
                            let sym = if by % 3 == 0 {
                                "╲"
                            } else if by % 3 == 1 {
                                "│"
                            } else {
                                "╱"
                            };
                            cell.set_symbol(sym);
                            cell.set_fg(Color::Rgb(180, 180, 100));
                        }
                    }
                }
            }
        }
    }

    // Draw particles only into empty cells — garnish, never obscure data
    for p in &ps.particles {
        let px = p.x as u16;
        let py = p.y as u16;
        if px < area.width && py < area.height {
            if let Some(cell) = buf.cell_mut((px, py)) {
                if cell.symbol() == " " {
                    cell.set_symbol(p.symbol);
                    cell.set_fg(p.fg);
                }
            }
        }
    }
}

// ── Overview tab (original layout) ─────────────────────────────────────────

fn ui_overview(frame: &mut Frame, app: &App) {
    let size = frame.area();
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(3),
            Constraint::Fill(2),
            Constraint::Fill(5),
            Constraint::Length(1),
        ])
        .split(size);

    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(main_chunks[0]);

    render_cpu(frame, app, top_chunks[0]);
    render_sysinfo(frame, top_chunks[1]);

    let mid_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(main_chunks[1]);

    render_memory(frame, app, mid_chunks[0]);
    render_network(frame, app, mid_chunks[1]);
    render_disk(frame, app, mid_chunks[2]);

    render_processes(frame, app, main_chunks[2]);
    render_status_bar(frame, app, main_chunks[3]);
}

// ── Processes tab ──────────────────────────────────────────────────────────

fn ui_processes_tab(frame: &mut Frame, app: &App) {
    let size = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1)])
        .split(size);

    render_processes_full(frame, app, chunks[0]);
    render_status_bar(frame, app, chunks[1]);
}

// ── CPU Detail tab ─────────────────────────────────────────────────────────

fn ui_cpu_detail(frame: &mut Frame, app: &App) {
    let size = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(size);

    render_cpu_sparklines(frame, app, chunks[0]);
    render_status_bar(frame, app, chunks[1]);
}

// ── Render functions ───────────────────────────────────────────────────────

fn cpu_gradient(usage: u64) -> Color {
    if usage > 95 {
        Color::Rgb(255, 60, 60)
    } else if usage > 80 {
        Color::Rgb(255, 140, 50)
    } else if usage > 60 {
        Color::Rgb(255, 220, 50)
    } else if usage > 30 {
        Color::Rgb(80, 200, 120)
    } else {
        Color::Rgb(60, 160, 200)
    }
}

fn render_cpu(frame: &mut Frame, app: &App, area: Rect) {
    let cpu_count = app.sys.cpus().len();
    let bars: Vec<Bar> = app
        .sys
        .cpus()
        .iter()
        .enumerate()
        .map(|(i, cpu)| {
            let usage = cpu.cpu_usage() as u64;
            let color = cpu_gradient(usage);
            Bar::default()
                .value(usage)
                .label(Line::from(format!("C{}", i)))
                .style(Style::default().fg(color))
                .text_value(format!("{}%", usage))
        })
        .collect();

    let avg: f32 =
        app.sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / cpu_count.max(1) as f32;

    let title = match (app.cpu_temp, app.cpu_freq_avg) {
        (Some(t), Some(f)) => format!(" CPU (avg: {:.0}%)  {:.0}°C  {:.0} MHz ", avg, t, f),
        (Some(t), None) => format!(" CPU (avg: {:.0}%)  {:.0}°C ", avg, t),
        (None, Some(f)) => format!(" CPU (avg: {:.0}%)  {:.0} MHz ", avg, f),
        (None, None) => format!(" CPU Usage (avg: {:.0}%) ", avg),
    };

    let inner_w = area.width.saturating_sub(2);
    let bar_w = if cpu_count > 0 {
        ((inner_w + 1) / cpu_count as u16).saturating_sub(1).max(3)
    } else {
        5
    };

    let chart = BarChart::default()
        .block(
            Block::default()
                .title(title)
                .title_bottom(Line::from(format!(" {} cores ", cpu_count)).right_aligned())
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Rgb(100, 120, 220))),
        )
        .data(BarGroup::default().bars(&bars))
        .bar_width(bar_w)
        .bar_gap(1)
        .max(100);

    frame.render_widget(chart, area);
}

fn render_sysinfo(frame: &mut Frame, area: Rect) {
    let info = read_system_info();
    let rows: Vec<Row> = info
        .iter()
        .map(|(k, v)| {
            Row::new(vec![
                Span::styled(k.as_str(), Style::default().fg(Color::Rgb(180, 100, 255))),
                Span::raw(v.as_str()),
            ])
        })
        .collect();

    let table = Table::new(rows, [Constraint::Length(12), Constraint::Min(20)]).block(
        Block::default()
            .title(" System Info ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(180, 100, 255))),
    );

    frame.render_widget(table, area);
}

fn render_memory(frame: &mut Frame, app: &App, area: Rect) {
    let total = app.sys.total_memory();
    let used = app.sys.used_memory();
    let swap_total = app.sys.total_swap();
    let swap_used = app.sys.used_swap();

    let mem_pct = if total > 0 {
        used as f64 / total as f64
    } else {
        0.0
    };
    let swap_pct = if swap_total > 0 {
        swap_used as f64 / swap_total as f64
    } else {
        0.0
    };

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .margin(1)
        .split(area);

    let block = Block::default()
        .title(" Memory ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(140, 160, 255)));
    frame.render_widget(block, area);

    let mem_label = Paragraph::new(format!(
        "RAM: {:.1}/{:.1} GB",
        used as f64 / 1_073_741_824.0,
        total as f64 / 1_073_741_824.0
    ))
    .style(Style::default().fg(Color::White));
    frame.render_widget(mem_label, inner[0]);

    let mem_gauge = Gauge::default()
        .gauge_style(
            Style::default()
                .fg(if mem_pct > 0.85 {
                    Color::Rgb(255, 100, 100)
                } else {
                    Color::Rgb(140, 160, 255)
                })
                .bg(Color::Rgb(30, 30, 50)),
        )
        .ratio(mem_pct.min(1.0))
        .label(format!("{:.0}%", mem_pct * 100.0));
    frame.render_widget(mem_gauge, inner[1]);

    let swap_label = Paragraph::new(format!(
        "Swap: {:.1}/{:.1} GB",
        swap_used as f64 / 1_073_741_824.0,
        swap_total as f64 / 1_073_741_824.0
    ))
    .style(Style::default().fg(Color::White));
    frame.render_widget(swap_label, inner[2]);

    let swap_gauge = Gauge::default()
        .gauge_style(
            Style::default()
                .fg(if swap_pct > 0.5 {
                    Color::Rgb(255, 100, 100)
                } else {
                    Color::Rgb(180, 100, 255)
                })
                .bg(Color::Rgb(30, 30, 50)),
        )
        .ratio(swap_pct.min(1.0))
        .label(format!("{:.0}%", swap_pct * 100.0));
    frame.render_widget(swap_gauge, inner[3]);

    let data: Vec<u64> = app.mem_history.iter().copied().collect();
    let spark = Sparkline::default()
        .data(&data)
        .max(100)
        .style(Style::default().fg(Color::Rgb(140, 160, 255)));
    frame.render_widget(spark, inner[4]);
}

fn render_network(frame: &mut Frame, app: &App, area: Rect) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .margin(1)
        .split(area);

    let block = Block::default()
        .title(" Network ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(100, 120, 220)));
    frame.render_widget(block, area);

    let net_info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("RX: ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw(format_bytes(app.net_rx_rate)),
        ]),
        Line::from(vec![
            Span::styled("TX: ", Style::default().fg(Color::Rgb(180, 100, 255))),
            Span::raw(format_bytes(app.net_tx_rate)),
        ]),
    ]);
    frame.render_widget(net_info, inner[0]);

    let rx_data: Vec<u64> = app.net_rx_history.iter().copied().collect();
    let spark_rx = Sparkline::default()
        .data(&rx_data)
        .style(Style::default().fg(Color::Rgb(140, 160, 255)));
    frame.render_widget(spark_rx, inner[1]);

    let tx_data: Vec<u64> = app.net_tx_history.iter().copied().collect();
    let spark_tx = Sparkline::default()
        .data(&tx_data)
        .style(Style::default().fg(Color::Rgb(180, 100, 255)));
    frame.render_widget(spark_tx, inner[2]);
}

fn render_disk(frame: &mut Frame, app: &App, area: Rect) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .margin(1)
        .split(area);

    let block = Block::default()
        .title(" Disk I/O ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(180, 100, 255)));
    frame.render_widget(block, area);

    let disk_info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Read:  ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw(format_bytes(app.disk_read_rate)),
        ]),
        Line::from(vec![
            Span::styled("Write: ", Style::default().fg(Color::Rgb(180, 100, 255))),
            Span::raw(format_bytes(app.disk_write_rate)),
        ]),
    ]);
    frame.render_widget(disk_info, inner[0]);

    let read_data: Vec<u64> = app.disk_read_history.iter().copied().collect();
    let spark_read = Sparkline::default()
        .data(&read_data)
        .style(Style::default().fg(Color::Rgb(140, 160, 255)));
    frame.render_widget(spark_read, inner[1]);

    let write_data: Vec<u64> = app.disk_write_history.iter().copied().collect();
    let spark_write = Sparkline::default()
        .data(&write_data)
        .style(Style::default().fg(Color::Rgb(180, 100, 255)));
    frame.render_widget(spark_write, inner[2]);
}

/// Overview tab: top 15 processes, respects sort mode + filter
fn render_processes(frame: &mut Frame, app: &App, area: Rect) {
    let mut procs: Vec<_> = app
        .sys
        .processes()
        .values()
        .map(|p| {
            (
                p.pid(),
                p.name().to_string_lossy().to_string(),
                p.cpu_usage(),
                p.memory(),
            )
        })
        .collect();

    if !app.filter_text.is_empty() {
        let filter = app.filter_text.to_lowercase();
        procs.retain(|(_, name, _, _)| name.to_lowercase().contains(&filter));
    }

    match app.sort_mode {
        SortMode::Cpu => {
            procs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
        }
        SortMode::Memory => procs.sort_by(|a, b| b.3.cmp(&a.3)),
        SortMode::Pid => procs.sort_by(|a, b| a.0.as_u32().cmp(&b.0.as_u32())),
    }
    let max_rows = area.height.saturating_sub(4) as usize;
    procs.truncate(max_rows);

    let rows: Vec<Row> = procs
        .iter()
        .enumerate()
        .map(|(i, (pid, name, cpu, mem))| {
            let cpu_color = if *cpu > 80.0 {
                Color::Red
            } else if *cpu > 40.0 {
                Color::Yellow
            } else {
                Color::White
            };
            let row = Row::new(vec![
                Span::styled(format!("{}", pid), Style::default().fg(Color::DarkGray)),
                Span::raw(if name.chars().count() > 20 {
                    format!("{}...", name.chars().take(17).collect::<String>())
                } else {
                    name.clone()
                }),
                Span::styled(format!("{:.1}%", cpu), Style::default().fg(cpu_color)),
                Span::raw(format!("{:.1} MB", *mem as f64 / 1_048_576.0)),
            ]);
            if i % 2 == 1 {
                row.style(Style::default().bg(Color::Rgb(22, 24, 40)))
            } else {
                row
            }
        })
        .collect();

    let header = Row::new(vec!["PID", "Process", "CPU", "Memory"])
        .style(
            Style::default()
                .fg(Color::Rgb(220, 220, 235))
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    let title = format!(" Top Processes (by {}) ", sort_label(app.sort_mode));

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(title)
            .title_bottom(Line::from(" Tab: full view ").right_aligned())
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(100, 120, 220))),
    );

    frame.render_widget(table, area);
}

/// Processes tab: full scrollable list with filter bar
fn render_processes_full(frame: &mut Frame, app: &App, area: Rect) {
    let mut procs: Vec<_> = app
        .sys
        .processes()
        .values()
        .map(|p| {
            (
                p.pid(),
                p.name().to_string_lossy().to_string(),
                p.cpu_usage(),
                p.memory(),
            )
        })
        .collect();

    if !app.filter_text.is_empty() {
        let filter = app.filter_text.to_lowercase();
        procs.retain(|(_, name, _, _)| name.to_lowercase().contains(&filter));
    }

    match app.sort_mode {
        SortMode::Cpu => {
            procs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
        }
        SortMode::Memory => procs.sort_by(|a, b| b.3.cmp(&a.3)),
        SortMode::Pid => procs.sort_by(|a, b| a.0.as_u32().cmp(&b.0.as_u32())),
    }

    // Split area for table + optional filter bar
    let (table_area, filter_area) = if app.filter_mode {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(1)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };

    // Compute visible window: header(1) + margin(1) + borders(2) = 4 overhead
    let visible_height = table_area.height.saturating_sub(4) as usize;
    let max_scroll = procs.len().saturating_sub(visible_height);
    let scroll = app.process_scroll.min(max_scroll);
    let end = procs.len().min(scroll + visible_height);
    let visible_procs = if scroll < procs.len() {
        &procs[scroll..end]
    } else {
        &[]
    };

    let rows: Vec<Row> = visible_procs
        .iter()
        .enumerate()
        .map(|(i, (pid, name, cpu, mem))| {
            let cpu_color = if *cpu > 80.0 {
                Color::Red
            } else if *cpu > 40.0 {
                Color::Yellow
            } else {
                Color::White
            };
            let row = Row::new(vec![
                Span::styled(format!("{}", pid), Style::default().fg(Color::DarkGray)),
                Span::raw(if name.chars().count() > 30 {
                    format!("{}...", name.chars().take(27).collect::<String>())
                } else {
                    name.clone()
                }),
                Span::styled(format!("{:.1}%", cpu), Style::default().fg(cpu_color)),
                Span::raw(format!("{:.1} MB", *mem as f64 / 1_048_576.0)),
            ]);
            if i % 2 == 1 {
                row.style(Style::default().bg(Color::Rgb(22, 24, 40)))
            } else {
                row
            }
        })
        .collect();

    let header = Row::new(vec!["PID", "Process", "CPU", "Memory"])
        .style(
            Style::default()
                .fg(Color::Rgb(220, 220, 235))
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    let title = format!(
        " Processes — sort: {} [{}/{}] ",
        sort_label(app.sort_mode),
        if procs.is_empty() { 0 } else { scroll + 1 },
        procs.len()
    );

    let scroll_label = format!(" {}/{} ", scroll + 1, procs.len());

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(title)
            .title_bottom(Line::from(scroll_label).right_aligned())
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(100, 120, 220))),
    );

    frame.render_widget(table, table_area);

    if let Some(fa) = filter_area {
        let filter_line = Line::from(vec![
            Span::styled(
                " / ",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw(format!(" {}", app.filter_text)),
            Span::styled(
                "\u{2588}",
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(filter_line), fa);
    }
}

/// CPU Detail tab: per-core sparklines with two-column layout when needed
fn render_cpu_sparklines(frame: &mut Frame, app: &App, area: Rect) {
    let cpu_count = app.cpu_history.len();
    if cpu_count == 0 {
        return;
    }

    let title = match (app.cpu_temp, app.cpu_freq_avg) {
        (Some(t), Some(f)) => format!(" CPU Detail  {:.0}°C  {:.0} MHz ", t, f),
        (Some(t), None) => format!(" CPU Detail  {:.0}°C ", t),
        (None, Some(f)) => format!(" CPU Detail  {:.0} MHz ", f),
        (None, None) => " CPU Detail ".to_string(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(100, 120, 220)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let available_rows = inner.height as usize;
    let use_two_cols = cpu_count > available_rows;

    if use_two_cols {
        // Split into two columns
        let col_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        let half = (cpu_count + 1) / 2;
        for (col_idx, col_area) in col_chunks.iter().enumerate() {
            let start = col_idx * half;
            let end = (start + half).min(cpu_count);
            let col_count = end - start;
            let mut constraints: Vec<Constraint> =
                (0..col_count).map(|_| Constraint::Length(1)).collect();
            constraints.push(Constraint::Min(0));
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(*col_area);

            for (ri, i) in (start..end).enumerate() {
                if ri >= rows.len().saturating_sub(1) {
                    break;
                }
                let data: Vec<u64> = app.cpu_history[i].iter().copied().collect();
                let current = data.last().copied().unwrap_or(0);
                let color = cpu_gradient(current);

                let row_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(12), Constraint::Min(1)])
                    .split(rows[ri]);

                let label = Paragraph::new(format!(" Core {:>2} {:>3}%", i, current))
                    .style(Style::default().fg(color));
                frame.render_widget(label, row_chunks[0]);

                let spark = Sparkline::default()
                    .data(&data)
                    .max(100)
                    .style(Style::default().fg(color));
                frame.render_widget(spark, row_chunks[1]);
            }
        }
    } else {
        // Single column: each core gets 1 row
        let mut constraints: Vec<Constraint> =
            (0..cpu_count).map(|_| Constraint::Length(1)).collect();
        constraints.push(Constraint::Min(0));
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        for (i, hist) in app.cpu_history.iter().enumerate() {
            if i >= rows.len().saturating_sub(1) {
                break;
            }
            let data: Vec<u64> = hist.iter().copied().collect();
            let current = data.last().copied().unwrap_or(0);
            let color = cpu_gradient(current);

            let row_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(12), Constraint::Min(1)])
                .split(rows[i]);

            let label = Paragraph::new(format!(" Core {:>2} {:>3}%", i, current))
                .style(Style::default().fg(color));
            frame.render_widget(label, row_chunks[0]);

            let spark = Sparkline::default()
                .data(&data)
                .max(100)
                .style(Style::default().fg(color));
            frame.render_widget(spark, row_chunks[1]);
        }
    }
}

/// Help overlay: centered popup
fn render_help_overlay(frame: &mut Frame) {
    let area = frame.area();
    let popup_w = 50u16.min(area.width.saturating_sub(4));
    let popup_h = 22u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_w)) / 2;
    let y = (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup);

    let text = vec![
        Line::from(Span::styled(
            " Peppemon Keybindings",
            Style::default()
                .fg(Color::Rgb(180, 100, 255))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tab      ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Cycle tabs"),
        ]),
        Line::from(vec![
            Span::styled("  q        ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Quit"),
        ]),
        Line::from(vec![
            Span::styled("  ?        ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Toggle this help"),
        ]),
        Line::from(vec![
            Span::styled("  /        ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Filter processes"),
        ]),
        Line::from(vec![
            Span::styled("  Esc      ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Close filter / quit"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Sort",
            Style::default()
                .fg(Color::Rgb(180, 100, 255))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  c        ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Sort by CPU"),
        ]),
        Line::from(vec![
            Span::styled("  m        ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Sort by Memory"),
        ]),
        Line::from(vec![
            Span::styled("  p        ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Sort by PID"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Navigation",
            Style::default()
                .fg(Color::Rgb(180, 100, 255))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  Up/Down  ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Scroll process list"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Background",
            Style::default()
                .fg(Color::Rgb(180, 100, 255))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  b        ", Style::default().fg(Color::Rgb(140, 160, 255))),
            Span::raw("Background effects settings"),
        ]),
    ];

    let help = Paragraph::new(text).block(
        Block::default()
            .title(" Help ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(180, 100, 255))),
    );
    frame.render_widget(help, popup);
}

/// Settings overlay: centered popup for background effect controls
fn render_settings_overlay(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let popup_w = 54u16.min(area.width.saturating_sub(4));
    let popup_h = 12u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_w)) / 2;
    let y = (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup);

    let effect_name = match app.particles.effect {
        WeatherEffect::Rain => "Rain",
        WeatherEffect::Snow => "Snow",
        WeatherEffect::Lightning => "Lightning",
        WeatherEffect::Seasons => "Seasons",
    };
    let cycle_name = match app.particles.cycle_mode {
        CycleMode::Auto => "Auto-cycle",
        CycleMode::Pinned => "Pinned",
    };
    let season_name = match app.particles.season_mode {
        SeasonMode::AutoRotate => "Auto-rotate",
        SeasonMode::RealSeason => "Real season",
        SeasonMode::NatureBlend => "Nature blend",
    };
    let int = app.particles.intensity as usize;
    let spd = app.particles.speed as usize;
    let intensity_bar = format!(
        "{}{} ({}/5)",
        "\u{2588}".repeat(int),
        "\u{2591}".repeat(5 - int),
        int
    );
    let speed_bar = format!(
        "{}{} ({}/10)",
        "\u{2588}".repeat(spd),
        "\u{2591}".repeat(10 - spd),
        spd
    );

    let labels = ["Effect", "Cycle Mode", "Season Mode", "Intensity", "Speed"];
    let values = [
        format!("\u{25c2} {} \u{25b8}", effect_name),
        format!("\u{25c2} {} \u{25b8}", cycle_name),
        format!("\u{25c2} {} \u{25b8}", season_name),
        format!("\u{25c2} {} \u{25b8}", intensity_bar),
        format!("\u{25c2} {} \u{25b8}", speed_bar),
    ];
    let all_rows = [
        SettingsRow::Effect,
        SettingsRow::CycleMode,
        SettingsRow::SeasonMode,
        SettingsRow::Intensity,
        SettingsRow::Speed,
    ];

    let mut lines = vec![
        Line::from(Span::styled(
            " Background Effects",
            Style::default()
                .fg(Color::Rgb(180, 100, 255))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for (i, (label, value)) in labels.iter().zip(values.iter()).enumerate() {
        let selected = all_rows[i] == app.settings_row;
        let (indicator, style) = if selected {
            (
                "\u{25b6} ",
                Style::default().fg(Color::Rgb(140, 160, 255)),
            )
        } else {
            ("  ", Style::default().fg(Color::Rgb(220, 220, 235)))
        };
        lines.push(Line::from(vec![
            Span::styled(indicator, style),
            Span::styled(format!("{:<14}", label), style),
            Span::styled(value.as_str(), style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  \u{2190}/\u{2192} change  \u{2191}/\u{2193} navigate  Esc close",
        Style::default().fg(Color::Rgb(100, 105, 130)),
    )));

    let settings = Paragraph::new(lines).block(
        Block::default()
            .title(" Settings ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(180, 100, 255))),
    );
    frame.render_widget(settings, popup);
}

fn settings_change(ps: &mut ParticleSystem, row: SettingsRow, right: bool) {
    match row {
        SettingsRow::Effect => {
            ps.effect = if right {
                match ps.effect {
                    WeatherEffect::Rain => WeatherEffect::Snow,
                    WeatherEffect::Snow => WeatherEffect::Lightning,
                    WeatherEffect::Lightning => WeatherEffect::Seasons,
                    WeatherEffect::Seasons => WeatherEffect::Rain,
                }
            } else {
                match ps.effect {
                    WeatherEffect::Rain => WeatherEffect::Seasons,
                    WeatherEffect::Snow => WeatherEffect::Rain,
                    WeatherEffect::Lightning => WeatherEffect::Snow,
                    WeatherEffect::Seasons => WeatherEffect::Lightning,
                }
            };
            ps.particles.clear();
            ps.transition_cooldown = 30;
            ps.cycle_timer = Instant::now();
        }
        SettingsRow::CycleMode => {
            ps.cycle_mode = match ps.cycle_mode {
                CycleMode::Auto => CycleMode::Pinned,
                CycleMode::Pinned => CycleMode::Auto,
            };
            ps.cycle_timer = Instant::now();
        }
        SettingsRow::SeasonMode => {
            ps.season_mode = if right {
                match ps.season_mode {
                    SeasonMode::AutoRotate => SeasonMode::RealSeason,
                    SeasonMode::RealSeason => SeasonMode::NatureBlend,
                    SeasonMode::NatureBlend => SeasonMode::AutoRotate,
                }
            } else {
                match ps.season_mode {
                    SeasonMode::AutoRotate => SeasonMode::NatureBlend,
                    SeasonMode::RealSeason => SeasonMode::AutoRotate,
                    SeasonMode::NatureBlend => SeasonMode::RealSeason,
                }
            };
            ps.season_timer = Instant::now();
        }
        SettingsRow::Intensity => {
            if right {
                ps.intensity = (ps.intensity + 1).min(5);
            } else {
                ps.intensity = ps.intensity.saturating_sub(1).max(1);
            }
        }
        SettingsRow::Speed => {
            if right {
                ps.speed = (ps.speed + 1).min(10);
            } else {
                ps.speed = ps.speed.saturating_sub(1).max(1);
            }
        }
    }
}

/// Status bar: tab name, sort mode, help hint (or filter input)
fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    if app.filter_mode {
        let line = Line::from(vec![
            Span::styled(
                " / ",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw(format!(" {}", app.filter_text)),
            Span::styled(
                "\u{2588}",
                Style::default().fg(Color::White).bg(Color::DarkGray),
            ),
            Span::styled("  Esc: cancel  Enter: apply", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    } else {
        let tab_name = match app.active_tab {
            ActiveTab::Overview => "Overview",
            ActiveTab::Processes => "Processes",
            ActiveTab::CpuDetail => "CPU Detail",
        };
        let status = Paragraph::new(Line::from(vec![
            Span::styled(
                " peppemon ",
                Style::default()
                    .fg(Color::Rgb(220, 220, 235))
                    .bg(Color::Rgb(100, 120, 220)),
            ),
            Span::raw("  "),
            Span::styled(
                format!(" {} ", tab_name),
                Style::default()
                    .fg(Color::Rgb(220, 220, 235))
                    .bg(Color::Rgb(180, 100, 255)),
            ),
            Span::raw(format!("  sort: {}  ", sort_label(app.sort_mode))),
            Span::styled(
                format!(" {} cpus ", app.sys.cpus().len()),
                Style::default().fg(Color::Rgb(100, 105, 130)),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    " {} ",
                    match app.particles.effect {
                        WeatherEffect::Rain => "Rain",
                        WeatherEffect::Snow => "Snow",
                        WeatherEffect::Lightning => "Lightning",
                        WeatherEffect::Seasons => "Seasons",
                    }
                ),
                Style::default()
                    .fg(Color::Rgb(220, 220, 235))
                    .bg(Color::Rgb(60, 70, 140)),
            ),
            Span::styled(
                "  ?: help  b: effects ",
                Style::default().fg(Color::Rgb(100, 105, 130)),
            ),
        ]));
        frame.render_widget(status, area);
    }
}

// ── Main ───────────────────────────────────────────────────────────────────

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = ratatui::init();

    let mut app = App::new();

    // Initial data collection (need two samples for CPU %)
    app.sys.refresh_cpu_usage();
    std::thread::sleep(Duration::from_millis(200));
    app.tick();

    let mut last_tick = Instant::now();
    let mut last_anim = Instant::now();

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Dual-tick: wake for whichever fires next
        let until_data = TICK_RATE.saturating_sub(last_tick.elapsed());
        let until_anim = ANIM_TICK.saturating_sub(last_anim.elapsed());
        let timeout = until_data.min(until_anim);

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.filter_mode {
                        match key.code {
                            KeyCode::Esc => {
                                app.filter_mode = false;
                                app.filter_text.clear();
                                app.process_scroll = 0;
                            }
                            KeyCode::Enter => {
                                app.filter_mode = false;
                            }
                            KeyCode::Backspace => {
                                app.filter_text.pop();
                                app.process_scroll = 0;
                            }
                            KeyCode::Char(c) => {
                                app.filter_text.push(c);
                                app.process_scroll = 0;
                            }
                            _ => {}
                        }
                    } else if app.show_settings {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('b') => app.show_settings = false,
                            KeyCode::Up => app.settings_row = app.settings_row.prev(),
                            KeyCode::Down => app.settings_row = app.settings_row.next(),
                            KeyCode::Left => {
                                settings_change(&mut app.particles, app.settings_row, false)
                            }
                            KeyCode::Right => {
                                settings_change(&mut app.particles, app.settings_row, true)
                            }
                            _ => {}
                        }
                    } else if app.show_help {
                        // Any key dismisses help
                        app.show_help = false;
                    } else {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                            KeyCode::Tab => {
                                app.active_tab = match app.active_tab {
                                    ActiveTab::Overview => ActiveTab::Processes,
                                    ActiveTab::Processes => ActiveTab::CpuDetail,
                                    ActiveTab::CpuDetail => ActiveTab::Overview,
                                };
                                app.process_scroll = 0;
                            }
                            KeyCode::Char('c') => app.sort_mode = SortMode::Cpu,
                            KeyCode::Char('m') => app.sort_mode = SortMode::Memory,
                            KeyCode::Char('p') => app.sort_mode = SortMode::Pid,
                            KeyCode::Char('/') => {
                                app.filter_mode = true;
                                app.filter_text.clear();
                            }
                            KeyCode::Char('?') => app.show_help = !app.show_help,
                            KeyCode::Char('b') => app.show_settings = !app.show_settings,
                            KeyCode::Up => {
                                app.process_scroll = app.process_scroll.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                app.process_scroll = app.process_scroll.saturating_add(1);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Animation tick (30 FPS)
        if last_anim.elapsed() >= ANIM_TICK {
            let dt = last_anim.elapsed().as_secs_f32().min(0.15);
            let size = terminal.size()?;
            app.particles.update(size.width, size.height, dt);
            last_anim = Instant::now();
        }

        // Data tick (1 Hz)
        if last_tick.elapsed() >= TICK_RATE {
            app.tick();
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    ratatui::restore();

    Ok(())
}

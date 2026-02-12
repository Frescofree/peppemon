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
        Bar, BarChart, BarGroup, Block, Borders, Clear, Gauge, Paragraph, Row, Sparkline, Table,
    },
    Frame,
};
use std::{
    collections::VecDeque,
    fs,
    io::{self, stdout},
    time::{Duration, Instant},
};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, RefreshKind, System};

const HISTORY_LEN: usize = 60;
const TICK_RATE: Duration = Duration::from_millis(1000);

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

/// Try hwmon (k10temp / coretemp), fall back to thermal_zone0
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

/// Average of all cores' scaling_cur_freq (kHz → MHz)
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

fn read_system_info() -> Vec<(String, String)> {
    let mut info = Vec::new();
    info.push((
        "Kernel".into(),
        System::kernel_version().unwrap_or_default(),
    ));
    info.push(("Host".into(), System::host_name().unwrap_or_default()));

    let uptime = System::uptime();
    let hours = uptime / 3600;
    let mins = (uptime % 3600) / 60;
    info.push(("Uptime".into(), format!("{}h {}m", hours, mins)));

    if let Ok(gov) = fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor") {
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
    info
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
    match app.active_tab {
        ActiveTab::Overview => ui_overview(frame, app),
        ActiveTab::Processes => ui_processes_tab(frame, app),
        ActiveTab::CpuDetail => ui_cpu_detail(frame, app),
    }
    if app.show_help {
        render_help_overlay(frame);
    }
}

// ── Overview tab (original layout) ─────────────────────────────────────────

fn ui_overview(frame: &mut Frame, app: &App) {
    let size = frame.area();
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Min(8),
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

fn render_cpu(frame: &mut Frame, app: &App, area: Rect) {
    let cpu_count = app.sys.cpus().len();
    let bars: Vec<Bar> = app
        .sys
        .cpus()
        .iter()
        .enumerate()
        .map(|(i, cpu)| {
            let usage = cpu.cpu_usage() as u64;
            let color = if usage > 90 {
                Color::Red
            } else if usage > 60 {
                Color::Yellow
            } else {
                Color::Green
            };
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

    let chart = BarChart::default()
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .data(BarGroup::default().bars(&bars))
        .bar_width(5)
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
                Span::styled(k.as_str(), Style::default().fg(Color::Cyan)),
                Span::raw(v.as_str()),
            ])
        })
        .collect();

    let table = Table::new(rows, [Constraint::Length(12), Constraint::Min(20)]).block(
        Block::default()
            .title(" System Info ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
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
        .border_style(Style::default().fg(Color::Green));
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
                    Color::Red
                } else {
                    Color::Green
                })
                .bg(Color::DarkGray),
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
                    Color::Red
                } else {
                    Color::Yellow
                })
                .bg(Color::DarkGray),
        )
        .ratio(swap_pct.min(1.0))
        .label(format!("{:.0}%", swap_pct * 100.0));
    frame.render_widget(swap_gauge, inner[3]);

    let data: Vec<u64> = app.mem_history.iter().copied().collect();
    let spark = Sparkline::default()
        .data(&data)
        .max(100)
        .style(Style::default().fg(Color::Green));
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
        .border_style(Style::default().fg(Color::Blue));
    frame.render_widget(block, area);

    let net_info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("RX: ", Style::default().fg(Color::Green)),
            Span::raw(format_bytes(app.net_rx_rate)),
        ]),
        Line::from(vec![
            Span::styled("TX: ", Style::default().fg(Color::Red)),
            Span::raw(format_bytes(app.net_tx_rate)),
        ]),
    ]);
    frame.render_widget(net_info, inner[0]);

    let rx_data: Vec<u64> = app.net_rx_history.iter().copied().collect();
    let spark_rx = Sparkline::default()
        .data(&rx_data)
        .style(Style::default().fg(Color::Green));
    frame.render_widget(spark_rx, inner[1]);

    let tx_data: Vec<u64> = app.net_tx_history.iter().copied().collect();
    let spark_tx = Sparkline::default()
        .data(&tx_data)
        .style(Style::default().fg(Color::Red));
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
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(block, area);

    let disk_info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Read:  ", Style::default().fg(Color::Green)),
            Span::raw(format_bytes(app.disk_read_rate)),
        ]),
        Line::from(vec![
            Span::styled("Write: ", Style::default().fg(Color::Red)),
            Span::raw(format_bytes(app.disk_write_rate)),
        ]),
    ]);
    frame.render_widget(disk_info, inner[0]);

    let read_data: Vec<u64> = app.disk_read_history.iter().copied().collect();
    let spark_read = Sparkline::default()
        .data(&read_data)
        .style(Style::default().fg(Color::Green));
    frame.render_widget(spark_read, inner[1]);

    let write_data: Vec<u64> = app.disk_write_history.iter().copied().collect();
    let spark_write = Sparkline::default()
        .data(&write_data)
        .style(Style::default().fg(Color::Red));
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
    procs.truncate(15);

    let rows: Vec<Row> = procs
        .iter()
        .map(|(pid, name, cpu, mem)| {
            let cpu_color = if *cpu > 80.0 {
                Color::Red
            } else if *cpu > 40.0 {
                Color::Yellow
            } else {
                Color::White
            };
            Row::new(vec![
                Span::styled(format!("{}", pid), Style::default().fg(Color::DarkGray)),
                Span::raw(if name.chars().count() > 20 {
                    format!("{}...", name.chars().take(17).collect::<String>())
                } else {
                    name.clone()
                }),
                Span::styled(format!("{:.1}%", cpu), Style::default().fg(cpu_color)),
                Span::raw(format!("{:.1} MB", *mem as f64 / 1_048_576.0)),
            ])
        })
        .collect();

    let header = Row::new(vec!["PID", "Process", "CPU", "Memory"])
        .style(
            Style::default()
                .fg(Color::Cyan)
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
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red)),
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
        .map(|(pid, name, cpu, mem)| {
            let cpu_color = if *cpu > 80.0 {
                Color::Red
            } else if *cpu > 40.0 {
                Color::Yellow
            } else {
                Color::White
            };
            Row::new(vec![
                Span::styled(format!("{}", pid), Style::default().fg(Color::DarkGray)),
                Span::raw(if name.chars().count() > 30 {
                    format!("{}...", name.chars().take(27).collect::<String>())
                } else {
                    name.clone()
                }),
                Span::styled(format!("{:.1}%", cpu), Style::default().fg(cpu_color)),
                Span::raw(format!("{:.1} MB", *mem as f64 / 1_048_576.0)),
            ])
        })
        .collect();

    let header = Row::new(vec!["PID", "Process", "CPU", "Memory"])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    let title = format!(
        " Processes — sort: {} [{}/{}] ",
        sort_label(app.sort_mode),
        if procs.is_empty() { 0 } else { scroll + 1 },
        procs.len()
    );

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
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red)),
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

/// CPU Detail tab: per-core sparklines
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
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Each core: 1 row for label+sparkline
    let mut constraints: Vec<Constraint> = (0..cpu_count).map(|_| Constraint::Length(1)).collect();
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
        let color = if current > 90 {
            Color::Red
        } else if current > 60 {
            Color::Yellow
        } else {
            Color::Green
        };

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

/// Help overlay: centered popup
fn render_help_overlay(frame: &mut Frame) {
    let area = frame.area();
    let popup_w = 50u16.min(area.width.saturating_sub(4));
    let popup_h = 18u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(popup_w)) / 2;
    let y = (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup);

    let text = vec![
        Line::from(Span::styled(
            " Peppemon Keybindings",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tab      ", Style::default().fg(Color::Yellow)),
            Span::raw("Cycle tabs"),
        ]),
        Line::from(vec![
            Span::styled("  q        ", Style::default().fg(Color::Yellow)),
            Span::raw("Quit"),
        ]),
        Line::from(vec![
            Span::styled("  ?        ", Style::default().fg(Color::Yellow)),
            Span::raw("Toggle this help"),
        ]),
        Line::from(vec![
            Span::styled("  /        ", Style::default().fg(Color::Yellow)),
            Span::raw("Filter processes"),
        ]),
        Line::from(vec![
            Span::styled("  Esc      ", Style::default().fg(Color::Yellow)),
            Span::raw("Close filter / quit"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Sort",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  c        ", Style::default().fg(Color::Yellow)),
            Span::raw("Sort by CPU"),
        ]),
        Line::from(vec![
            Span::styled("  m        ", Style::default().fg(Color::Yellow)),
            Span::raw("Sort by Memory"),
        ]),
        Line::from(vec![
            Span::styled("  p        ", Style::default().fg(Color::Yellow)),
            Span::raw("Sort by PID"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Navigation",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  Up/Down  ", Style::default().fg(Color::Yellow)),
            Span::raw("Scroll process list"),
        ]),
    ];

    let help = Paragraph::new(text).block(
        Block::default()
            .title(" Help ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(help, popup);
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
                Style::default().fg(Color::Black).bg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled(
                format!(" {} ", tab_name),
                Style::default().fg(Color::Black).bg(Color::Magenta),
            ),
            Span::raw(format!("  sort: {}  ", sort_label(app.sort_mode))),
            Span::styled(
                format!(" {} cpus ", app.sys.cpus().len()),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("  ?: help ", Style::default().fg(Color::DarkGray)),
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

    loop {
        terminal.draw(|f| ui(f, &app))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
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

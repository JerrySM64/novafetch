use anyhow::Result;
use chrono::{DateTime, Datelike, Utc, Timelike};
use nix::sys::statvfs::statvfs;
use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
use serde::Deserialize;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    env,
    fs,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::Command,
};
use unicode_width::UnicodeWidthStr;

const RESET: &str = "\x1b[0m";

fn fg(rgb: (u8, u8, u8)) -> String {
    format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2)
}
fn bold() -> &'static str { "\x1b[1m" }

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(x) = chars.next() {
                if x == 'm' { break; }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn visible_len(s: &str) -> usize {
    UnicodeWidthStr::width(strip_ansi(s).as_str())
}

fn pad_to(mut s: String, width: usize) -> String {
    let len = visible_len(&s);
    if len < width {
        s.push_str(&" ".repeat(width - len));
    }
    s
}

fn print_box(lines: &[String], frame_rgb: (u8,u8,u8)) {
    let frame = fg(frame_rgb);
    let inner_w = lines.iter().map(|l| visible_len(l)).max().unwrap_or(0);

    println!("{frame}┌{}┐{RESET}", "─".repeat(inner_w + 2));
    for l in lines {
        let l = pad_to(l.clone(), inner_w);
        println!("{frame}│ {RESET}{l}{frame} │{RESET}");
    }
    println!("{frame}└{}┘{RESET}", "─".repeat(inner_w + 2));
}

fn row_with_colors(
    icon: &str,
    icon_rgb: (u8,u8,u8),
    key: &str,
    key_rgb: (u8,u8,u8),
    val_rgb: (u8,u8,u8),
    val: impl Into<Cow<'static, str>>,
) -> String {
    let ic = fg(icon_rgb);
    let keyc = fg(key_rgb);
    let valc = fg(val_rgb);
    format!("{ic}{icon}{RESET} {keyc}{key:<16}{RESET} {valc}{}{RESET}", val.into())
}

#[derive(Clone)]
struct Line {
    module: &'static str,
    icon: &'static str,
    icon_rgb: (u8,u8,u8),
    key: &'static str,
    value: String,
    val_rgb: Option<(u8,u8,u8)>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct ThemeCfg {
    frame_rgb: Option<[u8; 3]>,
    header_rgb: Option<[u8; 3]>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct ModuleColorCfg {
    icon_rgb: Option<[u8; 3]>,
    key_rgb: Option<[u8; 3]>,
    value_rgb: Option<[u8; 3]>,
}

#[derive(Debug, Deserialize)]
struct Config {
    allow_exec: Option<bool>,
    order: Option<Vec<String>>,
    modules: Option<HashMap<String, bool>>,
    theme: Option<ThemeCfg>,
    colors: Option<HashMap<String, ModuleColorCfg>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            allow_exec: Some(true),
            order: Some(vec![
                "os","kernel","uptime","os_age","shell",
                "spacer",
                "host","hostname","cpu","gpu","memory",
                "spacer",
                "session","de_wm","terminal","packages",
            ].into_iter().map(String::from).collect()),
            modules: Some(HashMap::new()),
            theme: Some(ThemeCfg { frame_rgb: None, header_rgb: None }),
            colors: Some(HashMap::new()),
        }
    }
}

fn load_config() -> Config {
    let mut candidates = vec![PathBuf::from("novafetch.toml")];
    let config_path = dirs::config_dir().map(|cfg| cfg.join("novafetch").join("config.toml"));
    if let Some(ref p) = config_path {
        candidates.push(p.clone());
    }

    for p in &candidates {
        if let Ok(s) = fs::read_to_string(p) {
            if let Ok(cfg) = toml::from_str::<Config>(&s) {
                return cfg;
            }
        }
    }

    if let Some(p) = config_path {
        let toml_str = r#"# novafetch configuration

# Allow external commands for shell version, git info, packages, etc.
allow_exec = true

# Order of modules to display
order = [
    "os", "kernel", "uptime", "os_age", "shell",
    "spacer",
    "host", "hostname", "cpu", "gpu", "memory",
    "spacer",
    "session", "de_wm", "terminal", "packages"
]

[modules]
# Set to false to disable specific modules
# os = true
# os_age = true

[theme]
# frame_rgb = [120, 180, 120]
# header_rgb = [120, 180, 120]
"#;
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&p, toml_str);
    }

    Config::default()
}

fn module_enabled(cfg: &Config, name: &str) -> bool {
    cfg.modules
        .as_ref()
        .and_then(|m| m.get(name))
        .copied()
        .unwrap_or(true)
}

fn icon_for(module: &str, key: &str) -> &'static str {
    match module {
        "os" => distro_icon(),
        "kernel" => "",
        "uptime" => "",
        "os_age" => "",
        "shell" => "",
        "cpu" => "",
        "gpu" => "󰢮",
        "memory" => "󰍛",
        "terminal" => "",
        "packages" => "󰏖",

        "de_wm" => match key {
            "DE" => "󰕮",
            "WM" => "󰨇",
            _ => "󰕮",
        },

        "host"     => "󰌢",
        "hostname" => "󰒋",

        "session" => "◉",

        _ => "•",
    }
}

fn row_line(cfg: &Config, l: &Line) -> String {
    let default_key_rgb = if l.module == "os" { l.icon_rgb } else { module_color(l.module) };
    let default_val_rgb = (210, 210, 210);

    let (mut icon_rgb, mut key_rgb, mut val_rgb) =
        (l.icon_rgb, default_key_rgb, l.val_rgb.unwrap_or(default_val_rgb));

    if let Some(colors) = cfg.colors.as_ref() {
        if let Some(mc) = colors.get(l.module) {
            if let Some(v) = mc.icon_rgb { icon_rgb = (v[0], v[1], v[2]); }
            if let Some(v) = mc.key_rgb { key_rgb = (v[0], v[1], v[2]); }
            if let Some(v) = mc.value_rgb { val_rgb = (v[0], v[1], v[2]); }
        }
    }

    row_with_colors(l.icon, icon_rgb, l.key, key_rgb, val_rgb, l.value.clone())
}

fn read_trim<P: AsRef<Path>>(p: P) -> Result<String> {
    Ok(fs::read_to_string(&p)?.trim().to_string())
}

fn parse_os_release(s: &str) -> HashMap<String, String> {
    s.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k,v)| (k.to_string(), v.trim_matches('"').to_string()))
        .collect()
}

fn distro_icon() -> &'static str {
    let Ok(s) = fs::read_to_string("/etc/os-release") else {
        return "\u{E31A}"; // nf-linux-tux
    };
    let map = parse_os_release(&s);
    let id      = map.get("ID")     .map(|s| s.to_lowercase()).unwrap_or_default();
    let id_like = map.get("ID_LIKE").map(|s| s.to_lowercase()).unwrap_or_default();

    for check in [id.as_str(), id_like.as_str()] {
        if check.contains("arch")       { return "\u{F08C7}"; } // nf-linux-arch
        if check.contains("manjaro")    { return "\u{F312}"; }  // nf-linux-manjaro
        if check.contains("endeavour")  { return "\u{F322}"; }  // nf-linux-endeavouros
        if check.contains("garuda")     { return "\u{F337}"; }  // nf-linux-garuda
        if check.contains("ubuntu")     { return "\u{F31B}"; }  // nf-linux-ubuntu
        if check.contains("pop")        { return "\u{F32A}"; }  // nf-linux-pop_os
        if check.contains("mint")       { return "\u{F3A8}"; }  // nf-linux-linuxmint
        if check.contains("zorin")      { return "\u{F33D}"; }  // nf-linux-zorin
        if check.contains("debian")     { return "\u{F306}"; }  // nf-linux-debian
        if check.contains("kali")       { return "\u{F327}"; }  // nf-linux-kali_linux
        if check.contains("mx")         { return "\u{F33A}"; }  // nf-linux-mx_linux
        if check.contains("fedora")     { return "\u{F30A}"; }  // nf-linux-fedora
        if check.contains("rhel") || check.contains("centos") || check.contains("redhat") {
                                          return "\u{F316}"; }  // nf-linux-redhat
        if check.contains("opensuse") || check.contains("suse") {
                                          return "\u{F314}"; }  // nf-linux-opensuse
        if check.contains("void")       { return "\u{F32E}"; }  // nf-linux-void
        if check.contains("gentoo")     { return "\u{F30D}"; }  // nf-linux-gentoo
        if check.contains("nixos") || check.contains("nix") {
                                          return "\u{F313}"; }  // nf-linux-nixos
        if check.contains("alpine")     { return "\u{F300}"; }  // nf-linux-alpine
        if check.contains("solus")      { return "\u{F33B}"; }  // nf-linux-solus
        if check.contains("raspberry")  { return "\u{F315}"; }  // nf-linux-raspberry_pi
        if check.contains("slackware")  { return "\u{F318}"; }  // nf-linux-slackware
    }
    "\u{F31A}" // nf-linux-tux fallback
}

fn distro_brand_rgb() -> (u8, u8, u8) {
    let Ok(s) = fs::read_to_string("/etc/os-release") else {
        return (200, 200, 200);
    };
    let map = parse_os_release(&s);
    if let Some(ansi) = map.get("ANSI_COLOR") {
        let parts: Vec<&str> = ansi.split(';').collect();
        if let Some(pos) = parts.windows(2).position(|w| w[0] == "38" && w[1] == "2") {
            if let (Some(r), Some(g), Some(b)) = (
                parts.get(pos + 2).and_then(|v| v.parse::<u8>().ok()),
                parts.get(pos + 3).and_then(|v| v.parse::<u8>().ok()),
                parts.get(pos + 4).and_then(|v| v.parse::<u8>().ok()),
            ) {
                return (r, g, b);
            }
        }
    }

    let id      = map.get("ID")     .map(|s| s.to_lowercase()).unwrap_or_default();
    let id_like = map.get("ID_LIKE").map(|s| s.to_lowercase()).unwrap_or_default();

    for check in [id.as_str(), id_like.as_str()] {
        if check.contains("arch")      { return ( 23, 147, 209); } // #1793D1
        if check.contains("manjaro")   { return ( 53, 191,  92); } // #35BF5C
        if check.contains("endeavour") { return (123,  61, 176); } // #7B3DB0
        if check.contains("garuda")    { return ( 30, 215, 170); } // teal
        if check.contains("ubuntu")    { return (233,  84,  32); } // #E95420
        if check.contains("pop")       { return ( 72, 185, 199); } // #48B9C7
        if check.contains("mint")      { return (135, 207,  62); } // #87CF3E
        if check.contains("zorin")     { return ( 21, 166, 240); } // #15A6F0
        if check.contains("debian")    { return (215,   7,  81); } // #D70751
        if check.contains("kali")      { return ( 85, 124, 153); } // #557C99
        if check.contains("mx")        { return ( 74, 144, 226); }
        if check.contains("fedora")    { return ( 60, 110, 180); } // brand blue
        if check.contains("rhel") || check.contains("centos") || check.contains("redhat") {
                                         return (238,   0,   0); } // #EE0000
        if check.contains("opensuse") || check.contains("suse") {
                                         return (115, 186,  37); } // #73BA25
        if check.contains("void")      { return ( 71, 128,  97); } // #478061
        if check.contains("gentoo")    { return (100,  85, 163); } // #6455A3
        if check.contains("nixos")     { return ( 82, 119, 195); } // #5277C3
        if check.contains("alpine")    { return ( 13,  89, 127); } // #0D597F
        if check.contains("solus")     { return ( 82, 148, 226); } // #5294E2
        if check.contains("raspberry") { return (197,  26,  74); } // #C51A4A
        if check.contains("slackware") { return (100, 100, 200); }
    }
    (200, 200, 200) // neutral fallback
}

fn fmt_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 { format!("{d}d {h}h {m}m") }
    else if h > 0 { format!("{h}h {m}m") }
    else { format!("{m}m") }
}

fn fmt_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B","KiB","MiB","GiB","TiB"];
    let mut x = b as f64;
    let mut i = 0;
    while x >= 1024.0 && i < 4 {
        x /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", x, U[i])
}

fn gather_linux_base() -> Result<HashMap<&'static str, String>> {
    let mut m = HashMap::new();

    if let Ok(s) = fs::read_to_string("/etc/os-release") {
        if let Some(v) = parse_os_release(&s).get("PRETTY_NAME") {
            m.insert("OS", v.clone());
        }
    }

    if let Ok(k) = fs::read_to_string("/proc/sys/kernel/osrelease") {
        m.insert("Kernel", k.trim().into());
    }

    if let Ok(u) = fs::read_to_string("/proc/uptime") {
        if let Some(s) = u.split_whitespace().next() {
            m.insert("Uptime", fmt_uptime(s.parse::<f64>()? as u64));
        }
    }

    if let Ok(cpu) = fs::read_to_string("/proc/cpuinfo") {
        if let Some(l) = cpu.lines().find(|l| l.starts_with("model name")) {
            m.insert("CPU", l.split_once(':').unwrap().1.trim().into());
        }
        m.insert("CPU_THREADS", cpu.lines().filter(|l| l.starts_with("processor")).count().to_string());
    }

    if let Ok(mem) = fs::read_to_string("/proc/meminfo") {
        let t = mem.lines().find(|l| l.starts_with("MemTotal")).unwrap();
        let a = mem.lines().find(|l| l.starts_with("MemAvailable")).unwrap();
        let total: u64 = t.split_whitespace().nth(1).unwrap().parse()?;
        let avail: u64 = a.split_whitespace().nth(1).unwrap().parse()?;
        let used = total - avail;
        let pct = (used as f64 / total as f64 * 100.0).round();
        m.insert("Memory", format!("{} / {} ({pct:.0}%)", fmt_bytes(used*1024), fmt_bytes(total*1024)));
    }

    if let Ok(s) = env::var("SHELL") { m.insert("Shell", s); }
    if let Ok(t) = env::var("TERM") { m.insert("Terminal", t); }

    if env::var("WAYLAND_DISPLAY").is_ok() { m.insert("Session","Wayland".into()); }
    else if env::var("DISPLAY").is_ok() { m.insert("Session","X11".into()); }

    if let Ok(v) = env::var("XDG_CURRENT_DESKTOP") { m.insert("DE", v); }
    if let Ok(v) = env::var("DESKTOP_SESSION") { m.insert("DESKTOP_SESSION", v); }

    if let Ok(model) = read_trim("/sys/devices/virtual/dmi/id/product_name") {
        if !model.is_empty() {
            m.insert("HostModel", model);
        }
    }
    if let Ok(h) = read_trim("/etc/hostname") {
        m.insert("Host", h);
    }

    Ok(m)
}

fn cmd_exists(bin: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else { return false; };
    env::split_paths(&paths).any(|p| p.join(bin).exists())
}

fn run_cmd_lines(bin: &str, args: &[&str]) -> Result<Vec<String>> {
    let out = Command::new(bin).args(args).output()?;
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect())
}

fn run_cmd_string(bin: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(bin).args(args).output()?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s).trim()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn module_color(module: &str) -> (u8, u8, u8) {
    match module {
        "os"       => (100, 200, 255), // soft sky-blue
        "kernel"   => (255, 165,  60), // warm amber
        "uptime"   => (160, 120, 255), // soft violet
        "os_age"   => (255, 200,  80), // warm gold
        "shell"    => ( 80, 200, 160), // teal-green
        "cpu"      => (255,  80,  80), // vivid red
        "gpu"      => (255, 140,  80), // coral/orange
        "memory"   => ( 80, 170, 255), // steel-blue
        "host"     => (200, 200, 200), // light grey
        "hostname" => (200, 200, 200), // light grey
        "session"  => (180, 100, 255), // purple
        "de_wm"    => (100, 220, 180), // aqua-green
        "terminal" => ( 80, 200, 120), // green
        "packages" => (255, 210,  60), // golden-yellow
        "disks"    => (140, 180, 220), // muted blue
        _          => (120, 180, 120), // default green
    }
}

fn module_simple(module: &'static str, key: &'static str, info: &HashMap<&'static str,String>, name: &'static str) -> Vec<Line> {
    vec![Line {
        module,
        icon: icon_for(module, key),
        icon_rgb: if module == "os" { distro_brand_rgb() } else { module_color(module) },
        key,
        value: info.get(name).cloned().unwrap_or_else(|| "Unknown".into()),
        val_rgb: None,
    }]
}

fn module_host_model(info: &HashMap<&'static str,String>) -> Vec<Line> {
    let model = info.get("HostModel").cloned().unwrap_or_else(|| "Unknown".into());
    vec![Line {
        module: "host",
        icon: icon_for("host", "Host"),
        icon_rgb: module_color("host"),
        key: "Host",
        value: model,
        val_rgb: None,
    }]
}

fn module_hostname(info: &HashMap<&'static str,String>) -> Vec<Line> {
    let host = info.get("Host").cloned().unwrap_or_else(|| "Unknown".into());
    vec![Line {
        module: "hostname",
        icon: icon_for("hostname", "Hostname"),
        icon_rgb: module_color("hostname"),
        key: "Hostname",
        value: host,
        val_rgb: None,
    }]
}

fn shell_pretty(allow_exec: bool) -> String {
    let shell_path = env::var("SHELL").unwrap_or_else(|_| "sh".into());
    let shell_name = basename(&shell_path).to_string();

    if !allow_exec {
        return shell_name;
    }

    let candidate = if cmd_exists(&shell_name) { shell_name.clone() } else { shell_path.clone() };

    let version = match shell_name.as_str() {
        "zsh" => run_cmd_string(&candidate, &["--version"]).ok().map(|s| {
            let l = first_line(&s);
            l.split_whitespace().nth(1).unwrap_or("").to_string()
        }),
        "bash" => run_cmd_string(&candidate, &["--version"]).ok().map(|s| {
            let l = first_line(&s);
            if let Some(pos) = l.find("version ") {
                let v = &l[pos + "version ".len()..];
                v.split_whitespace().next().unwrap_or("").to_string()
            } else {
                l.to_string()
            }
        }),
        "fish" => run_cmd_string(&candidate, &["--version"]).ok().map(|s| {
            let l = first_line(&s);
            l.split_whitespace().last().unwrap_or("fish").to_string()
        }),
        _ => None,
    };

    match version {
        Some(v) if !v.is_empty() => format!("{shell_name} {v}"),
        _ => shell_name,
    }
}

fn module_shell(allow_exec: bool) -> Vec<Line> {
    vec![Line {
        module: "shell",
        icon: icon_for("shell", "Shell"),
        icon_rgb: module_color("shell"),
        key: "Shell",
        value: shell_pretty(allow_exec),
        val_rgb: None,
    }]
}

fn module_os_age() -> Result<Vec<Line>> {
    let root_paths = ["/ostree", "/bedrock", "/"];
    let mut birth = None;
    for p in root_paths {
        if let Ok(meta) = fs::metadata(p) {
            if let Ok(c) = meta.created() {
                birth = Some(c);
                break;
            }
        }
    }

    let Some(birth) = birth else { return Ok(vec![]); };
    let birth: DateTime<Utc> = birth.into();
    let now = Utc::now();

    let mut years = now.year() - birth.year();
    let mut months = now.month() as i32 - birth.month() as i32;
    let mut days = now.day() as i32 - birth.day() as i32;

    if now.hour() < birth.hour()
        || (now.hour() == birth.hour() && now.minute() < birth.minute())
        || (now.hour() == birth.hour() && now.minute() == birth.minute() && now.second() < birth.second())
    {
        days -= 1;
    }

    if days < 0 {
        months -= 1;
        let last_day_of_prev_month = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
            .and_then(|d| d.pred_opt());
        let days_in_prev_month = last_day_of_prev_month.map(|d| d.day() as i32).unwrap_or(30);
        days += days_in_prev_month;
    }

    if months < 0 {
        months += 12;
        years -= 1;
    }

    let mut parts = vec![];
    if years > 0 { parts.push(format!("{} Year{}", years, if years == 1 { "" } else { "s" })); }
    if months > 0 { parts.push(format!("{} Month{}", months, if months == 1 { "" } else { "s" })); }
    if days > 0 || (years == 0 && months == 0) {
        parts.push(format!("{} Day{}", days, if days == 1 { "" } else { "s" }));
    }

    Ok(vec![Line {
        module: "os_age",
        icon: icon_for("os_age", "OS Age"),
        icon_rgb: module_color("os_age"),
        key: "OS Age",
        value: parts.join(" "),
        val_rgb: None,
    }])
}

fn read_ppid(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let rparen = stat.rfind(')')?;
    let after = &stat[rparen + 1..];
    let mut it = after.split_whitespace();
    it.next()?;
    let ppid = it.next()?.parse::<i32>().ok()?;
    Some(ppid)
}

fn read_comm(pid: i32) -> Option<String> {
    fs::read_to_string(format!("/proc/{}/comm", pid)).ok().map(|s| s.trim().to_string())
}

fn is_shell_name(name: &str) -> bool {
    matches!(
        name,
        "bash" | "zsh" | "fish" | "sh" | "ash" | "dash"
            | "ksh" | "mksh" | "oksh" | "tcsh" | "csh"
            | "nu"                    // nushell
            | "pwsh"                  // PowerShell
            | "elvish"                // Elvish
            | "xonsh"                 // xonsh
            | "oil.ovm"               // Oils / osh
            | "git-shell"             // git-shell
    )
}

fn is_wrapper_name(name: &str) -> bool {
    matches!(
        name,
        "sudo" | "su" | "doas" | "env"
            | "login"
            | "strace" | "ltrace" | "perf" | "gdb" | "lldb" | "valgrind"
            | "proot" | "script" | "time"
            | "chezmoi" | "clifm"
    ) || name.starts_with("flatpak-")
      || name.ends_with(".sh")
      || name.starts_with("Relay(")   // WSL2 artefact
}

fn terminal_pretty_name(raw: &str) -> String {
    match raw {
        "wezterm-gui"           => "WezTerm".into(),
        "gnome-terminal-server"
        | "gnome-terminal"      => "GNOME Terminal".into(),
        "kgx"                   => "GNOME Console".into(),
        "konsole"               => "Konsole".into(),
        "xterm"                 => "XTerm".into(),
        "urxvt" | "urxvtd"     => "URxvt".into(),
        "st"                    => "st".into(),
        "kitty"                 => "kitty".into(),
        "alacritty"             => "Alacritty".into(),
        "foot" | "footclient"  => "foot".into(),
        "ghostty"               => "Ghostty".into(),
        "hyper"                 => "Hyper".into(),
        "tilix"                 => "Tilix".into(),
        "terminator"            => "Terminator".into(),
        "terminology"           => "Terminology".into(),
        "rio"                   => "Rio".into(),
        other                   => other.into(),
    }
}

fn detect_terminal_process() -> Option<String> {
    let mut pid = std::process::id() as i32;
    for _ in 0..20 {
        let ppid = read_ppid(pid)?;
        if ppid <= 1 { return None; }
        let comm = read_comm(ppid)?;
        let name = comm.trim().to_string();

        if name == "novafetch"
            || is_shell_name(&name)
            || is_wrapper_name(&name)
            || matches!(name.as_str(), "tmux" | "screen")
        {
            pid = ppid;
            continue;
        }

        return Some(terminal_pretty_name(&name));
    }
    None
}

fn module_terminal(info: &HashMap<&'static str, String>) -> Vec<Line> {
    let term = detect_terminal_process()
        .or_else(|| {
            if let Ok(pid_s) = env::var("KITTY_PID") {
                if pid_s.parse::<i32>().is_ok() { return Some("kitty".into()); }
            }
            if env::var("KONSOLE_VERSION").is_ok() { return Some("Konsole".into()); }
            if env::var("GNOME_TERMINAL_SCREEN").is_ok()
                || env::var("GNOME_TERMINAL_SERVICE").is_ok() {
                return Some("GNOME Terminal".into());
            }
            if env::var("WEZTERM_PANE").is_ok() { return Some("WezTerm".into()); }
            if env::var("GHOSTTY_RESOURCES_DIR").is_ok() { return Some("Ghostty".into()); }
            if env::var("ALACRITTY_SOCKET").is_ok()
                || env::var("ALACRITTY_WINDOW_ID").is_ok() {
                return Some("Alacritty".into());
            }
            None
        })
        .or_else(|| env::var("TERM_PROGRAM").ok())
        .unwrap_or_else(|| info.get("Terminal").cloned().unwrap_or_else(|| "Unknown".into()));

    vec![Line {
        module: "terminal",
        icon: icon_for("terminal", "Terminal"),
        icon_rgb: module_color("terminal"),
        key: "Terminal",
        value: term,
        val_rgb: None,
    }]
}

fn detect_wayland_compositor_name() -> Option<String> {
    let runtime_dir = env::var("XDG_RUNTIME_DIR").ok()?;
    let wayland_display = env::var("WAYLAND_DISPLAY").ok()?;

    let socket_path = if wayland_display.starts_with('/') {
        PathBuf::from(&wayland_display)
    } else {
        PathBuf::from(&runtime_dir).join(&wayland_display)
    };

    let stream = UnixStream::connect(&socket_path).ok()?;
    let cred = getsockopt(&stream, PeerCredentials).ok()?;
    let pid = cred.pid();
    if pid <= 0 { return None; }

    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let exe = cmdline.split(|&b| b == 0).next()?;
    let exe_str = std::str::from_utf8(exe).ok()?;
    let name = exe_str.rsplit('/').next().unwrap_or(exe_str);
    if name == "wl-restart" {
        let real = cmdline.split(|&b| b == 0).nth(1)?;
        let real_str = std::str::from_utf8(real).ok()?;
        return Some(real_str.rsplit('/').next().unwrap_or(real_str).to_string());
    }
    Some(name.to_string())
}

fn is_standalone_compositor(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "niri" | "sway" | "hyprland" | "river" | "wayfire" | "labwc"
            | "kwin_wayland" | "mutter" | "weston"
    )
}

fn de_pretty(info: &HashMap<&'static str, String>, wm_name: Option<&str>, allow_exec: bool) -> String {
    let de = info
        .get("DE")
        .cloned()
        .or_else(|| info.get("DESKTOP_SESSION").cloned())
        .unwrap_or_default();

    if let Some(wm) = wm_name {
        if is_standalone_compositor(wm) {
            return String::new();
        }
    }

    if de.is_empty() || is_standalone_compositor(&de) {
        return String::new();
    }

    let de_l = de.to_lowercase();

    if de_l.contains("kde") {
        if allow_exec && cmd_exists("plasmashell") {
            if let Ok(s) = run_cmd_string("plasmashell", &["--version"]) {
                let l = first_line(&s);
                let ver = l.split_whitespace().last().unwrap_or("?");
                return format!("KDE Plasma {ver}");
            }
        }
        return "KDE Plasma".into();
    }

    de
}

fn wm_pretty(info: &HashMap<&'static str, String>, allow_exec: bool) -> (String, Option<String>) {
    let session = info.get("Session").cloned().unwrap_or_else(|| "Unknown".into());
    let de = info.get("DE").cloned().unwrap_or_default().to_lowercase();

    if de.contains("kde") {
        return ("KWin".into(), None);
    }

    if session == "Wayland" {
        if let Some(name) = detect_wayland_compositor_name() {
            return (name.clone(), Some(name));
        }

        if env::var("NIRI_SOCKET").is_ok() {
            return ("niri".into(), Some("niri".into()));
        }
        if allow_exec && cmd_exists("hyprctl") {
            return ("Hyprland".into(), Some("hyprland".into()));
        }
        if allow_exec && cmd_exists("swaymsg") {
            return ("sway".into(), Some("sway".into()));
        }
        return ("Unknown".into(), None);
    }

    if session == "X11" {
        return ("Unknown".into(), None);
    }

    ("Unknown".into(), None)
}

fn module_de_wm(info: &HashMap<&'static str, String>, allow_exec: bool) -> Vec<Line> {
    let mut out = vec![];

    let (wm_val, compositor_name) = wm_pretty(info, allow_exec);
    let de_val = de_pretty(info, compositor_name.as_deref(), allow_exec);

    if !de_val.is_empty() {
        out.push(Line {
            module: "de_wm",
            icon: icon_for("de_wm", "DE"),
            icon_rgb: module_color("de_wm"),
            key: "DE",
            value: de_val,
            val_rgb: None,
        });
    }

    out.push(Line {
        module: "de_wm",
        icon: icon_for("de_wm", "WM"),
        icon_rgb: module_color("de_wm"),
        key: "WM",
        value: wm_val,
        val_rgb: None,
    });

    out
}

fn module_cpu(info: &HashMap<&'static str,String>) -> Vec<Line> {
    let c = info.get("CPU").cloned().unwrap_or_else(|| "Unknown".into());
    let t = info.get("CPU_THREADS").cloned().unwrap_or_else(|| "?".into());
    vec![Line {
        module: "cpu",
        icon: icon_for("cpu", "CPU"),
        icon_rgb: module_color("cpu"),
        key: "CPU",
        value: format!("{c} ({t})"),
        val_rgb: None,
    }]
}

fn module_packages(allow: bool) -> Result<Vec<Line>> {
    if !allow { return Ok(vec![]); }
    let mut parts: Vec<String> = vec![];

    if Path::new("/var/lib/dpkg/status").exists() {
        let count = fs::read_to_string("/var/lib/dpkg/status")
            .ok()
            .map(|s| {
                s.split("\n\n")
                    .filter(|block| block.contains("Status: install ok installed"))
                    .count()
            })
            .unwrap_or(0);
        if count > 0 { parts.push(format!("{count} (dpkg)")); }
    }

    if cmd_exists("pacman") {
        if let Ok(lines) = run_cmd_lines("pacman", &["-Qq"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (pacman)")); }
        }
    }

    if cmd_exists("rpm") && !cmd_exists("pacman") {
        if let Ok(lines) = run_cmd_lines("rpm", &["-qa"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (rpm)")); }
        }
    }

    if cmd_exists("xbps-query") {
        if let Ok(lines) = run_cmd_lines("xbps-query", &["-l"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (xbps)")); }
        }
    }

    if cmd_exists("qlist") {
        if let Ok(lines) = run_cmd_lines("qlist", &["-I"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (portage)")); }
        }
    }

    if cmd_exists("eopkg") {
        if let Ok(lines) = run_cmd_lines("eopkg", &["list-installed", "-q"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (eopkg)")); }
        }
    }

    if cmd_exists("apk") {
        if let Ok(lines) = run_cmd_lines("apk", &["list", "--installed"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (apk)")); }
        }
    }

    if cmd_exists("nix-env") {
        if let Ok(lines) = run_cmd_lines("nix-env", &["-q"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (nix)")); }
        }
    }

    if cmd_exists("snap") {
        if let Ok(lines) = run_cmd_lines("snap", &["list"]) {
            let count = lines.iter().skip(1).filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (snap)")); }
        }
    }

    if cmd_exists("flatpak") {
        if let Ok(lines) = run_cmd_lines("flatpak", &["list"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (flatpak)")); }
        }
    }

    if cmd_exists("brew") {
        if let Ok(lines) = run_cmd_lines("brew", &["list", "--formula"]) {
            let count = lines.iter().filter(|l| !l.trim().is_empty()).count();
            if count > 0 { parts.push(format!("{count} (brew)")); }
        }
    }

    Ok(if parts.is_empty() {
        vec![]
    } else {
        vec![Line {
            module: "packages",
            icon: icon_for("packages", "Packages"),
            icon_rgb: module_color("packages"),
            key: "Packages",
            value: parts.join(", "),
            val_rgb: None,
        }]
    })
}

fn module_disks() -> Result<Vec<Line>> {
    let mut out = vec![];
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let mut seen = HashSet::new();

    for l in mounts.lines() {
        let mut p = l.split(" - ");
        let left = p.next().unwrap_or("");
        let right = p.next().unwrap_or("");

        let mp = left.split_whitespace().nth(4).unwrap_or("");
        let fs_ty = right.split_whitespace().next().unwrap_or("");

        if !seen.insert(mp.to_string()) {
            continue;
        }

        if matches!(fs_ty, "proc" | "sysfs" | "tmpfs" | "devtmpfs" | "cgroup2" | "overlay" | "squashfs") {
            continue;
        }

        if let Ok(v) = statvfs(mp) {
            let total = v.blocks() * v.block_size() as u64;
            if total == 0 { continue; }
            let free = v.blocks_free() * v.block_size() as u64;
            let used = total.saturating_sub(free);
            let pct = (used as f64 / total as f64 * 100.0).round();

            out.push(Line {
                module: "disks",
                icon: icon_for("disks", "Disk"),
                icon_rgb: module_color("disks"),
                key: "Disk",
                value: format!("{} / {} ({pct:.0}%) - {mp} - {fs_ty}", fmt_bytes(used), fmt_bytes(total)),
                val_rgb: None,
            });
        }
    }

    Ok(out)
}

fn has_pcie_link_info(addr: &str) -> bool {
    Path::new(&format!("/sys/bus/pci/devices/{addr}/current_link_width")).exists()
        && Path::new(&format!("/sys/bus/pci/devices/{addr}/current_link_speed")).exists()
}

fn gpu_kind(addr: &str) -> &'static str {
    if addr.starts_with("0000:00:02.") {
        return "Integrated";
    }
    if has_pcie_link_info(addr) {
        return "Discrete";
    }
    "Integrated"
}

fn bracket_chunks(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_br = false;

    for ch in s.chars() {
        match ch {
            '[' if !in_br => { in_br = true; cur.clear(); }
            ']' if in_br => { in_br = false; out.push(cur.trim().to_string()); }
            _ if in_br => cur.push(ch),
            _ => {}
        }
    }
    out
}

fn shorten_gpu_name_vendor_aware(raw: &str) -> String {
    let chunks = bracket_chunks(raw);

    let preferred = chunks.iter().rev().find(|c| {
        let l = c.to_lowercase();
        l.contains("radeon") || l.contains("geforce") || l.contains("intel") || l.contains("arc")
    });

    let fallback = chunks.iter().rev().find(|c| {
        let l = c.to_lowercase();
        !(l == "amd/ati" || l == "amd" || l == "nvidia corporation" || l == "intel corporation")
    });

    let mut name = preferred
        .or(fallback)
        .map(|s| s.to_string())
        .unwrap_or_else(|| raw.to_string());

    if let Some((first, _)) = name.split_once('/') {
        name = first.trim().to_string();
    }
    if let Some((before, _)) = name.split_once("(rev") {
        name = before.trim().to_string();
    }

    let lower = name.to_lowercase();
    if lower.starts_with("radeon") {
        format!("AMD {name}")
    } else if lower.starts_with("geforce") {
        format!("NVIDIA {name}")
    } else {
        name
    }
}

fn module_gpu(allow: bool) -> Result<Vec<Line>> {
    if !allow || !cmd_exists("lspci") { return Ok(vec![]); }

    let mut out = vec![];
    for l in run_cmd_lines("lspci", &["-D"])? {
        let ll = l.to_lowercase();
        if !(ll.contains("vga compatible controller") || ll.contains("3d controller") || ll.contains("display controller")) {
            continue;
        }

        let (addr, rest) = match l.split_once(' ') {
            Some(x) => x,
            None => continue,
        };

        let name_raw = rest.split_once(": ").map(|(_, b)| b).unwrap_or(rest);
        let pretty = shorten_gpu_name_vendor_aware(name_raw);

        let kind = gpu_kind(addr);
        let kind = if Path::new(&format!("/sys/bus/pci/devices/{addr}")).exists() { kind } else { "Unknown" };

        out.push(Line {
            module: "gpu",
            icon: icon_for("gpu", "GPU"),
            icon_rgb: module_color("gpu"),
            key: "GPU",
            value: format!("{pretty} [{kind}]"),
            val_rgb: None,
        });
    }
    Ok(out)
}

fn main() -> Result<()> {
    let cfg = load_config();
    let allow_exec = cfg.allow_exec.unwrap_or(true);
    let info = gather_linux_base()?;

    let frame_rgb = cfg.theme.as_ref().and_then(|t| t.frame_rgb).map(|v|(v[0],v[1],v[2])).unwrap_or((120,180,120));
    let header_rgb = cfg.theme.as_ref().and_then(|t| t.header_rgb).map(|v|(v[0],v[1],v[2])).unwrap_or((120,180,120));
    let header = fg(header_rgb);

    let user = env::var("USER").unwrap_or_else(|_| "user".into());

    let mut lines = vec![
        format!("{}{}Hello, {}!{}", bold(), header, user, RESET),
        String::new(),
    ];

    let order = cfg.order.clone().unwrap_or_else(|| Config::default().order.unwrap());

    for m in &order {
        let m = m.to_lowercase();
        if !module_enabled(&cfg, &m) { continue; }

        if m == "spacer" {
            if !lines.last().map(|l| l.is_empty()).unwrap_or(true) {
                lines.push(String::new());
            }
            continue;
        }

        let v: Vec<Line> = match m.as_str() {
            "os" => module_simple("os", "OS", &info, "OS"),
            "kernel" => module_simple("kernel", "Kernel", &info, "Kernel"),
            "uptime" => module_simple("uptime", "Uptime", &info, "Uptime"),
            "os_age" => module_os_age()?,
            "shell" => module_shell(allow_exec),

            "host" => module_host_model(&info),
            "hostname" => module_hostname(&info),

            "cpu" => module_cpu(&info),
            "gpu" => module_gpu(allow_exec)?,
            "memory" => module_simple("memory", "Memory", &info, "Memory"),

            "session" => module_simple("session", "Session", &info, "Session"),
            "de_wm" => module_de_wm(&info, allow_exec),
            "terminal" => module_terminal(&info),

            "packages" => module_packages(allow_exec)?,
            "disks" => module_disks()?,

            _ => vec![],
        };

        for l in v {
            lines.push(row_line(&cfg, &l));
        }
    }

    let w = lines.iter().map(|l| visible_len(l)).max().unwrap_or(0);
    lines[1] = format!("{header}{}{}", "─".repeat(w), RESET);

    print_box(&lines, frame_rgb);
    Ok(())
}

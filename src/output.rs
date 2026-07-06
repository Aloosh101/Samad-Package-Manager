use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static PROGRESS_TX: LazyLock<Mutex<HashMap<u32, mpsc::Sender<String>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_progress_tx(uid: u32, tx: mpsc::Sender<String>) {
    if let Ok(mut guard) = PROGRESS_TX.lock() {
        guard.insert(uid, tx);
    }
}

pub fn clear_progress_tx(uid: u32) {
    if let Ok(mut guard) = PROGRESS_TX.lock() {
        guard.remove(&uid);
    }
}

fn send_progress(uid: u32, msg: String) {
    if let Ok(guard) = PROGRESS_TX.lock() {
        if let Some(ref tx) = guard.get(&uid) {
            let _ = tx.send(msg);
        }
    }
}

thread_local! {
    static CURRENT_UID: std::cell::RefCell<Option<u32>> = const { std::cell::RefCell::new(None) };
}

pub fn set_current_uid(uid: u32) {
    CURRENT_UID.with(|c| *c.borrow_mut() = Some(uid));
}

pub fn clear_current_uid() {
    CURRENT_UID.with(|c| *c.borrow_mut() = None);
}

fn current_uid() -> Option<u32> {
    CURRENT_UID.with(|c| *c.borrow())
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";

fn is_color_terminal() -> bool {
    std::env::var("NO_COLOR").is_err() && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true)
}

fn colored(color: &str, text: impl std::fmt::Display) -> String {
    if is_color_terminal() {
        format!("{color}{text}{RESET}")
    } else {
        format!("{text}")
    }
}

pub fn bold(text: impl std::fmt::Display) -> String {
    if is_color_terminal() {
        format!("{BOLD}{text}{RESET}")
    } else {
        format!("{text}")
    }
}

pub fn dim(text: impl std::fmt::Display) -> String {
    if is_color_terminal() {
        format!("{DIM}{text}{RESET}")
    } else {
        format!("{text}")
    }
}

pub fn green(text: impl std::fmt::Display) -> String {
    colored(GREEN, text)
}

pub fn red(text: impl std::fmt::Display) -> String {
    colored(RED, text)
}

pub fn yellow(text: impl std::fmt::Display) -> String {
    colored(YELLOW, text)
}

pub fn blue(text: impl std::fmt::Display) -> String {
    colored(BLUE, text)
}

pub fn cyan(text: impl std::fmt::Display) -> String {
    colored(CYAN, text)
}

pub fn magenta(text: impl std::fmt::Display) -> String {
    colored(MAGENTA, text)
}

fn clean_msg(msg: &str) -> String {
    msg.replace('\n', " ").replace('\r', "")
}

fn send_msg(msg: impl std::fmt::Display) {
    let s = clean_msg(&msg.to_string());
    if let Some(uid) = current_uid() {
        send_progress(uid, s);
    }
}

pub fn section(title: impl std::fmt::Display) {
    eprintln!("\n  {} {}", bold(cyan("──")), bold(&title));
    send_msg(format!("section: {title}"));
}

pub fn step_success(msg: impl std::fmt::Display) {
    eprintln!("  {} {}", green("✔"), &msg);
    send_msg(format!("success: {msg}"));
}

pub fn step_info(msg: impl std::fmt::Display) {
    eprintln!("  {} {}", blue("ℹ"), &msg);
    send_msg(format!("info: {msg}"));
}

pub fn step_warn(msg: impl std::fmt::Display) {
    eprintln!("  {} {}", yellow("⚠"), msg);
}

pub fn step_error(msg: impl std::fmt::Display) {
    eprintln!("  {} {}", red("✖"), msg);
}

pub fn result_summary(name: &str, version: &str, pkg_type: &str, file_count: usize, elapsed: Duration) {
    println!("\n  {} {} {} ({}) — {} files recorded {}",
        green("✔"),
        bold(name),
        dim(format!("v{}", version)),
        dim(pkg_type),
        file_count,
        dim(format!("({})", fmt_duration(elapsed))),
    );
}

pub fn result_message(msg: impl std::fmt::Display) {
    println!("  {} {}", green("✔"), msg);
}

pub fn remove_message(name: &str, elapsed: Duration) {
    println!("  {} {} {}",
        green("✔"),
        bold(name),
        dim(format!("({})", fmt_duration(elapsed))),
    );
}

pub fn show_installed_info(pkg: &crate::types::InstalledPackage, file_count: usize) {
    let name = &pkg.name;
    let version = &pkg.version;
    let fmt = format!("{:?}", pkg.format);
    let install_type = format!("{:?}", pkg.install_type);
    let source = pkg.source_repo.as_deref().unwrap_or("none");
    let date = if pkg.install_date.len() >= 19 {
        &pkg.install_date[..19]
    } else {
        &pkg.install_date
    };

    println!();
    eprintln!("  {} {} {}", bold("📦"), bold(name), dim(format!("v{}", version)));
    eprintln!("  {}", dim("  ──"));
    eprintln!("  {:>12} : {}", dim("Package"), green(name));
    eprintln!("  {:>12} : {}", dim("Version"), green(version));
    eprintln!("  {:>12} : {}", dim("Format"), fmt);
    eprintln!("  {:>12} : {}", dim("Type"), install_type);
    eprintln!("  {:>12} : {}", dim("Source"), source);
    eprintln!("  {:>12} : {}", dim("Installed"), date);
    eprintln!("  {:>12} : {}", dim("Files"), file_count);
    eprintln!("  {}", dim("  ──"));
    println!();
}

pub fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 60.0 {
        format!("{:.0}m {:.0}s", secs / 60.0, secs % 60.0)
    } else if secs >= 1.0 {
        format!("{:.1}s", secs)
    } else {
        format!("{}ms", d.as_millis())
    }
}

pub fn fmt_size(bytes: f64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut b = bytes;
    for u in UNITS {
        if b < 1024.0 { return format!("{:.1} {}", b, u); }
        b /= 1024.0;
    }
    format!("{:.1} GB", b * 1024.0)
}

pub fn fmt_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000.0 {
        format!("{:.1} MB/s", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.1} KB/s", bytes_per_sec / 1_000.0)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}

/// Lightweight spinner for showing subprocess output line-by-line.
pub struct Spinner {
    message: String,
    spin: usize,
}

const SPINNER_CHARS: &[u8] = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".as_bytes();

impl Spinner {
    pub fn new(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self::print_frame(0, &msg);
        send_msg(&msg);
        Spinner { message: msg, spin: 1 }
    }

    pub fn tick(&mut self) {
        let ch = SPINNER_CHARS[self.spin % SPINNER_CHARS.len()] as char;
        let max = 80usize.saturating_sub(4);
        let truncated = if self.message.len() > max {
            format!("{}…", &self.message[..max.saturating_sub(1)])
        } else {
            self.message.clone()
        };
        if is_color_terminal() {
            eprint!("  {} {}       \r", ch, truncated);
        } else {
            eprint!("  {}       \r", truncated);
        }
        self.spin = (self.spin + 1) % SPINNER_CHARS.len();
    }

    pub fn message(&mut self, text: &str) {
        self.message = text.to_string();
        let ch = SPINNER_CHARS[self.spin % SPINNER_CHARS.len()] as char;
        let max = 80usize.saturating_sub(4);
        let truncated = if text.len() > max {
            format!("{}…", &text[..max.saturating_sub(1)])
        } else {
            text.to_string()
        };
        if is_color_terminal() {
            eprint!("  {} {}       \r", ch, truncated);
        } else {
            eprint!("  {}       \r", truncated);
        }
        send_msg(text);
        self.spin = (self.spin + 1) % SPINNER_CHARS.len();
    }

    pub fn finish(&self) {
        if is_color_terminal() {
            eprintln!("  {} {}        ", green("✔"), self.message);
        } else {
            eprintln!("  {}        ", self.message);
        }
        send_msg(format!("✔ {}", self.message));
    }

    fn print_frame(spin: usize, msg: &str) {
        let ch = SPINNER_CHARS[spin % SPINNER_CHARS.len()] as char;
        let max = 80usize.saturating_sub(4);
        let truncated = if msg.len() > max {
            format!("{}…", &msg[..max.saturating_sub(1)])
        } else {
            msg.to_string()
        };
        if is_color_terminal() {
            eprint!("  {} {}       \r", ch, truncated);
        } else {
            eprint!("  {}       \r", truncated);
        }
    }
}

/// Interactive download progress bar with spinner, size, speed, and bar.
///
/// Example output:
///   ⠙ 📥 [00:05] [████░░░░░░░░░░░░░░░░░░░░] 4.2 MB/8.5 MB (4.1 MB/s) Downloading nginx
///   ✔ 📥 8.5 MB [00:08] Downloading nginx
pub struct ProgressBar {
    message: String,
    total: Option<u64>,
    downloaded: u64,
    start: Instant,
    last_update: Instant,
    spin: usize,
    finished: bool,
}

impl ProgressBar {
    pub fn new(message: impl Into<String>) -> Self {
        ProgressBar {
            message: message.into(),
            total: None,
            downloaded: 0,
            start: Instant::now(),
            last_update: Instant::now(),
            spin: 0,
            finished: false,
        }
    }

    pub fn with_total(message: impl Into<String>, total: u64) -> Self {
        ProgressBar {
            message: message.into(),
            total: Some(total),
            downloaded: 0,
            start: Instant::now(),
            last_update: Instant::now(),
            spin: 0,
            finished: false,
        }
    }

    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.message = msg.into();
    }

    /// Add downloaded bytes. Call on every chunk.
    pub fn tick_by(&mut self, n: u64) {
        self.downloaded += n;
    }

    /// Render one frame. Call after every chunk. Returns true if a frame was drawn.
    pub fn update(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_update) < Duration::from_millis(100) {
            return false;
        }
        self.last_update = now;
        self.spin += 1;

        let spinner = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".as_bytes();
        let ch = spinner[self.spin % spinner.len()] as char;
        let elapsed = now.duration_since(self.start);
        let downloaded = self.downloaded as f64;
        let speed = if elapsed.as_secs_f64() > 0.0 {
            downloaded / elapsed.as_secs_f64()
        } else {
            0.0
        };

        let bar = match self.total {
            Some(total) if total > 0 => {
                let pct = downloaded / total as f64 * 100.0;
                let w = 30usize;
                let f = (pct / 100.0 * w as f64) as usize;
                format!("[{}]",
                    String::from_iter(
                        std::iter::repeat_n('█', f)
                            .chain(std::iter::repeat_n('░', w.saturating_sub(f)))
                    )
                )
            }
            _ => "[━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━]".into(),
        };

        eprint!("\r  {} 📥 [{}] {} {} ({}) {}       ",
            ch,
            fmt_duration(elapsed),
            bar,
            fmt_size(downloaded),
            fmt_speed(speed),
            self.message,
        );
        if current_uid().is_some() {
            let pct = self.total.map(|t| if t > 0 { downloaded / t as f64 * 100.0 } else { 0.0 });
            let pct_str = pct.map(|p| format!(" {:.0}%", p)).unwrap_or_default();
            send_msg(format!("📥 {}{} ({} {})", self.message, pct_str, fmt_size(downloaded), fmt_speed(speed)));
        }
        true
    }

    /// Mark complete. Shows either a resume or normal finish message.
    pub fn finish(&mut self, label: &str) {
        if self.finished { return; }
        self.finished = true;
        let elapsed = self.start.elapsed();
        eprintln!("\r  {} {} {}  [{}]        ",
            green("✔"),
            label,
            dim(fmt_size(self.downloaded as f64)),
            dim(fmt_duration(elapsed)),
        );
        send_msg(format!("✔ {} {} [{}]", label, fmt_size(self.downloaded as f64), fmt_duration(elapsed)));
    }

    /// Mark complete when the download was resumed (partial).
    pub fn finish_resumed(&mut self, label: &str, resumed_from: u64) {
        if self.finished { return; }
        self.finished = true;
        let elapsed = self.start.elapsed();
        let resumed_bytes = self.downloaded.saturating_sub(resumed_from);
        eprintln!("\r  {} {} {}  {}  [{}]        ",
            green("✔"),
            label,
            dim(fmt_size(self.downloaded as f64)),
            dim(format!("[+{}]", fmt_size(resumed_bytes as f64))),
            dim(fmt_duration(elapsed)),
        );
        send_msg(format!("✔ {} {} [+{}] [{}]", label, fmt_size(self.downloaded as f64), fmt_size(resumed_bytes as f64), fmt_duration(elapsed)));
    }

    /// Mark as failed.
    pub fn fail(&mut self, msg: impl std::fmt::Display) {
        if self.finished { return; }
        self.finished = true;
        eprintln!("\r  {} {} {}        ",
            red("✖"), self.message, msg,
        );
        send_msg(format!("✖ {} {}", self.message, msg));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_fmt_duration_millis() {
        let s = fmt_duration(Duration::from_millis(500));
        assert_eq!(s, "500ms");
    }

    #[test]
    fn test_fmt_duration_seconds() {
        let s = fmt_duration(Duration::from_secs_f64(3.5));
        assert_eq!(s, "3.5s");
    }

    #[test]
    fn test_fmt_duration_minutes() {
        let s = fmt_duration(Duration::from_secs(125));
        assert_eq!(s, "2m 5s");
    }

    #[test]
    fn test_fmt_duration_exact_minute() {
        let s = fmt_duration(Duration::from_secs(60));
        assert_eq!(s, "1m 0s");
    }

    #[test]
    fn test_fmt_size_bytes() {
        assert_eq!(fmt_size(500.0), "500.0 B");
    }

    #[test]
    fn test_fmt_size_kb() {
        assert_eq!(fmt_size(1024.0), "1.0 KB");
        assert_eq!(fmt_size(2048.0), "2.0 KB");
    }

    #[test]
    fn test_fmt_size_mb() {
        assert_eq!(fmt_size(1048576.0), "1.0 MB");
    }

    #[test]
    fn test_fmt_size_gb() {
        assert_eq!(fmt_size(1073741824.0), "1.0 GB");
    }

    #[test]
    fn test_fmt_size_large_mb() {
        let s = fmt_size(5_242_880.0);
        assert!(s.contains("MB"));
    }

    #[test]
    fn test_fmt_speed_bytes() {
        let s = fmt_speed(500.0);
        assert_eq!(s, "500 B/s");
    }

    #[test]
    fn test_fmt_speed_kb() {
        let s = fmt_speed(1500.0);
        assert!(s.contains("KB/s"));
    }

    #[test]
    fn test_fmt_speed_mb() {
        let s = fmt_speed(2_000_000.0);
        assert!(s.contains("MB/s"));
    }

    #[test]
    fn test_is_color_terminal_no_color() {
        // NO_COLOR set should disable colors
        std::env::set_var("NO_COLOR", "1");
        assert!(!is_color_terminal());
        std::env::remove_var("NO_COLOR");
    }

    #[test]
    fn test_is_color_terminal_dumb() {
        std::env::set_var("TERM", "dumb");
        std::env::remove_var("NO_COLOR");
        assert!(!is_color_terminal());
        std::env::remove_var("TERM");
    }

    #[test]
    fn test_colored_no_color() {
        std::env::set_var("NO_COLOR", "1");
        assert_eq!(colored(RED, "hello"), "hello");
        std::env::remove_var("NO_COLOR");
    }

    #[test]
    fn test_bold_dim_no_color() {
        std::env::set_var("NO_COLOR", "1");
        assert_eq!(bold("x"), "x");
        assert_eq!(dim("y"), "y");
        std::env::remove_var("NO_COLOR");
    }

    #[test]
    fn test_fmt_duration_zero() {
        assert_eq!(fmt_duration(Duration::from_secs(0)), "0ms");
    }
}

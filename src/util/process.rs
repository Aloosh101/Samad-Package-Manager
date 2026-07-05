use std::fs;
use std::path::Path;

use crate::error::SpmResult;

pub fn find_deleted_libs() -> SpmResult<()> {
    let proc_dir = Path::new("/proc");
    if !proc_dir.exists() {
        println!("No /proc filesystem found. Are you on Linux?");
        return Ok(());
    }

    let entries = fs::read_dir(proc_dir)?;
    let mut deleted_entries: Vec<(u32, String, String)> = Vec::new();

    for entry in entries {
        let entry = entry?;
        let pid_str = entry.file_name();
        let pid: u32 = match pid_str.to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let maps_path = entry.path().join("maps");
        if !maps_path.exists() {
            continue;
        }

        let maps_content = match fs::read_to_string(&maps_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in maps_content.lines() {
            if line.contains("(deleted)") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    let lib_path = parts.last().unwrap_or(&"").trim_end_matches(" (deleted)");
                    let inode_str = parts.get(4).unwrap_or(&"0");
                    let inode: u64 = inode_str.parse().unwrap_or(0);

                    if inode > 0 && !lib_path.is_empty() {
                        deleted_entries.push((
                            pid,
                            lib_path.to_string(),
                            format!("inode:{}", inode),
                        ));
                    }
                }
            }
        }
    }

    if deleted_entries.is_empty() {
        println!("No processes using deleted libraries found.");
        return Ok(());
    }

    println!("{:<8} {:<20} {:<40} SUGGESTION",
        "PID", "PROCESS", "DELETED LIB");
    println!("{}", "-".repeat(100));

    let mut seen = std::collections::HashSet::new();
    let mut suggested = std::collections::HashSet::new();

    for (pid, lib_path, _info) in &deleted_entries {
        let proc_name = get_process_name(*pid);
        let key = (*pid, lib_path.clone());

        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        let suggestion = if let Some(pkg_name) = find_package_for_file(lib_path) {
            format!("restart {} or reinstall {}", proc_name, pkg_name)
        } else {
            format!("restart {}", proc_name)
        };

        if !suggested.contains(&proc_name) {
            suggested.insert(proc_name.clone());
        }

        println!("{:<8} {:<20} {:<40} {}",
            pid,
            truncate(&proc_name, 18),
            truncate(lib_path, 38),
            suggestion);
    }

    if !suggested.is_empty() {
        println!("\nSuggested actions:");
        for proc in &suggested {
            println!("  systemctl restart {}  OR  kill -HUP {}", proc, proc);
        }
    }

    Ok(())
}

fn get_process_name(pid: u32) -> String {
    let cmdline_path = format!("/proc/{}/comm", pid);
    fs::read_to_string(&cmdline_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| format!("<pid:{}>", pid))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

fn find_package_for_file(path: &str) -> Option<String> {
    // Check dpkg info list files first
    let info_dir = std::path::Path::new("/var/lib/dpkg/info");
    if info_dir.is_dir() {
        for entry in std::fs::read_dir(info_dir).ok()? {
            let entry = entry.ok()?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("list") {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if content.lines().any(|l| l.trim() == path) {
                        return p.file_stem().map(|s| s.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    // Try RPM sqlite DB
    if let Ok(conn) = rusqlite::Connection::open("/var/lib/rpm/rpmdb.sqlite") {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT DISTINCT p.name FROM packages p \
             JOIN files fi ON fi.packageId = p.packageId \
             WHERE fi.name = ?1 LIMIT 1"
        ) {
            if let Ok(rows) = stmt.query_map([path], |row| row.get::<_, String>(0)) {
                if let Some(pkg) = rows.filter_map(|r| r.ok()).next() {
                    return Some(pkg);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exact() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_long_string() {
        let result = truncate("hello world this is a long string", 20);
        assert_eq!(result.len(), 20);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_max_zero() {
        assert_eq!(truncate("hello", 0), "...");
    }

    #[test]
    fn test_truncate_max_one() {
        let result = truncate("hello", 1);
        // With ellipsis, min is 3 chars ("...")
        assert_eq!(result.len(), 3);
        assert_eq!(result, "...");
    }

    #[test]
    fn test_get_process_name_nonexistent() {
        let name = get_process_name(9999999);
        // Should not panic, returns a fallback
        assert!(!name.is_empty());
    }
}

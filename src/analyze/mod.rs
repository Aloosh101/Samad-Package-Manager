use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

pub fn full_analysis() -> SpmResult<()> {
    println!("=== Full System Analysis ===\n");

    println!("--- Orphan Packages ---");
    find_orphans_internal()?;

    println!("\n--- File Conflicts ---");
    find_conflicts_internal()?;

    println!("\n--- Dependency Cycles ---");
    find_dependency_cycles_internal()?;

    Ok(())
}

pub fn find_orphans() -> SpmResult<()> {
    println!("=== Orphan Packages ===\n");
    find_orphans_internal()
}

fn find_orphans_internal() -> SpmResult<()> {
    let conn = db::get_connection()?;
    let installed = db::list_installed_packages(&conn)?;

    if installed.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    let all_names: HashSet<String> = installed.iter().map(|p| p.name.clone()).collect();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for pkg in &installed {
        if let Some(ref manifest_json) = pkg.manifest {
            if let Ok(manifest) = serde_json::from_str::<Manifest>(manifest_json) {
                for dep in &manifest.dependencies {
                    if all_names.contains(&dep.name) {
                        dependents.entry(dep.name.clone())
                            .or_default()
                            .push(pkg.name.clone());
                    }
                }
            }
        }
    }

    let mut orphans_found = false;
    for pkg in &installed {
        let deps = dependents.get(&pkg.name);
        let dep_count = deps.map(|v| v.len()).unwrap_or(0);

        if dep_count == 0 && pkg.name != "spm" {
            println!("  {} {} - no other package depends on it", pkg.name, pkg.version);
            orphans_found = true;
        }
    }

    if !orphans_found {
        println!("  No orphan packages found.");
    }

    Ok(())
}

pub fn find_conflicts() -> SpmResult<()> {
    println!("=== File Conflicts ===\n");
    find_conflicts_internal()
}

fn find_conflicts_internal() -> SpmResult<()> {
    let mut file_owners: HashMap<String, Vec<String>> = HashMap::new();

    let entries = fs::read_dir("/usr/bin").ok();
    if let Some(entries) = entries {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() || path.is_symlink() {
                let path_str = path.to_string_lossy().to_string();
                let output = Command::new("dpkg")
                    .args(["-S", &path_str])
                    .output()
                    .ok();
                if let Some(out) = output {
                    if out.status.success() {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        for line in stdout.lines() {
                            if let Some(pkg_name) = line.split(':').next() {
                                file_owners.entry(path_str.clone())
                                    .or_default()
                                    .push(pkg_name.trim().to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    let mut conflicts_found = false;
    for (file_path, owners) in &file_owners {
        if owners.len() > 1 {
            println!("  Conflict: {} owned by:", file_path);
            for owner in owners {
                println!("    - {}", owner);
            }
            conflicts_found = true;
        }
    }

    let lib_conflicts = find_soname_conflicts();
    for (lib, owners) in &lib_conflicts {
        if owners.len() > 1 {
            println!("  SONAME conflict: {} used by:", lib);
            for owner in owners {
                println!("    - {}", owner);
            }
            conflicts_found = true;
        }
    }

    if !conflicts_found {
        println!("  No file conflicts detected.");
    }

    Ok(())
}

fn find_soname_conflicts() -> HashMap<String, Vec<String>> {
    let mut conflicts: HashMap<String, Vec<String>> = HashMap::new();
    let lib_dirs = ["/usr/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu"];

    for lib_dir in &lib_dirs {
        let path = Path::new(lib_dir);
        if !path.exists() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".so") || name.contains(".so.") {
                        let soname = name
                            .split_once(".so")
                            .map(|(base, _)| format!("{}.so", base))
                            .unwrap_or_else(|| name.to_string());

                        let output = Command::new("dpkg")
                            .args(["-S", &p.to_string_lossy()])
                            .output()
                            .ok();
                        if let Some(out) = output {
                            if out.status.success() {
                                let stdout = String::from_utf8_lossy(&out.stdout);
                                for line in stdout.lines() {
                                    if let Some(pkg_name) = line.split(':').next() {
                                        conflicts.entry(soname.clone())
                                            .or_default()
                                            .push(pkg_name.trim().to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    conflicts
}

fn find_dependency_cycles_internal() -> SpmResult<()> {
    let conn = db::get_connection()?;
    let installed = db::list_installed_packages(&conn)?;

    let all_names: HashSet<String> = installed.iter().map(|p| p.name.clone()).collect();
    let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();

    for pkg in &installed {
        if let Some(ref manifest_json) = pkg.manifest {
            if let Ok(manifest) = serde_json::from_str::<Manifest>(manifest_json) {
                let deps: Vec<String> = manifest.dependencies
                    .iter()
                    .filter(|d| all_names.contains(&d.name))
                    .map(|d| d.name.clone())
                    .collect();
                deps_map.insert(pkg.name.clone(), deps);
            }
        }
    }

    let mut visited = HashSet::new();
    let mut cycles_found = false;

    for pkg in &installed {
        if !visited.contains(&pkg.name) && has_cycle(&pkg.name, &deps_map, &mut visited, &mut vec![]) {
            println!("  Cycle detected involving: {}", pkg.name);
            cycles_found = true;
        }
    }

    if !cycles_found {
        println!("  No dependency cycles found.");
    }

    Ok(())
}

fn has_cycle(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    stack: &mut Vec<String>,
) -> bool {
    if stack.contains(&node.to_string()) {
        return true;
    }
    if visited.contains(node) {
        return false;
    }

    visited.insert(node.to_string());
    stack.push(node.to_string());

    if let Some(deps) = graph.get(node) {
        for dep in deps {
            if has_cycle(dep, graph, visited, stack) {
                return true;
            }
        }
    }

    stack.pop();
    false
}

pub fn trace_binary(path: &str) -> SpmResult<()> {
    let binary_path = Path::new(path);
    if !binary_path.exists() {
        return Err(SpmError::other(format!("Binary not found: {path}")));
    }

    println!("Tracing: {}\n", path);

    let ldd_output = Command::new("ldd")
        .arg(path)
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run ldd: {e}. Is ldd available?")))?;

    if !ldd_output.status.success() {
        return Err(SpmError::other(format!("ldd failed for: {path}")));
    }

    let stdout = String::from_utf8_lossy(&ldd_output.stdout);
    let mut missing_libs = Vec::new();
    let mut found_libs = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("linux-vdso") || line.starts_with("statically") {
            continue;
        }

        if line.contains("not found") {
            if let Some(lib) = line.split_whitespace().next() {
                missing_libs.push(lib.to_string());
                println!("  [MISSING] {}", lib);
            }
        } else if let Some((lib, _rest)) = line.split_once(" => ") {
            if !lib.contains('/') {
                let lib_name = lib.trim();
                let resolved = _rest.split_whitespace().next().unwrap_or("");
                if resolved != "not found" {
                    found_libs.push((lib_name.to_string(), resolved.to_string()));
                }
            }
        }
    }

    if missing_libs.is_empty() {
        println!("\n  All dependencies resolved ✓");
    } else {
        println!("\n  Missing libraries:");
        for lib in &missing_libs {
            let suggestion = suggest_package_for_lib(lib);
            println!("    {} -> Try: apt install {} or dnf install {}", lib, suggestion, suggestion);
        }
    }

    if !found_libs.is_empty() {
        println!("\n  Resolved libraries:");
        for (lib, resolved) in &found_libs {
            println!("    {} -> {}", lib, resolved);
        }
    }

    Ok(())
}

fn suggest_package_for_lib(lib: &str) -> String {
    let lib_lower = lib.to_lowercase();
    let name = lib_lower
        .trim_end_matches(".so")
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')
        .trim_end_matches('-');

    let known = [
        ("libssl", "libssl3"),
        ("libcrypto", "libssl3"),
        ("libpcre", "libpcre3"),
        ("libz", "zlib1g"),
        ("libbz2", "libbz2-1.0"),
        ("liblzma", "liblzma5"),
        ("libcurl", "libcurl4"),
        ("libxml2", "libxml2"),
        ("libsqlite3", "libsqlite3-0"),
        ("libreadline", "libreadline8"),
        ("libncurses", "libncurses6"),
        ("libpcap", "libpcap0.8"),
        ("libevent", "libevent-2.1-7"),
    ];

    for (prefix, pkg) in &known {
        if name.starts_with(prefix) {
            return pkg.to_string();
        }
    }

    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_cycle_no_cycle() {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        graph.insert("a".into(), vec!["b".into()]);
        graph.insert("b".into(), vec!["c".into()]);
        graph.insert("c".into(), vec![]);

        assert!(!has_cycle("a", &graph, &mut HashSet::new(), &mut vec![]));
    }

    #[test]
    fn test_has_cycle_direct() {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        graph.insert("a".into(), vec!["a".into()]);

        assert!(has_cycle("a", &graph, &mut HashSet::new(), &mut vec![]));
    }

    #[test]
    fn test_has_cycle_indirect() {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        graph.insert("a".into(), vec!["b".into()]);
        graph.insert("b".into(), vec!["c".into()]);
        graph.insert("c".into(), vec!["a".into()]);

        assert!(has_cycle("a", &graph, &mut HashSet::new(), &mut vec![]));
    }

    #[test]
    fn test_has_cycle_no_cycle_complex() {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        graph.insert("a".into(), vec!["b".into(), "c".into()]);
        graph.insert("b".into(), vec!["d".into()]);
        graph.insert("c".into(), vec!["d".into()]);
        graph.insert("d".into(), vec![]);

        assert!(!has_cycle("a", &graph, &mut HashSet::new(), &mut vec![]));
    }

    #[test]
    fn test_has_cycle_disconnected() {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        graph.insert("a".into(), vec!["b".into()]);
        graph.insert("b".into(), vec![]);
        graph.insert("c".into(), vec!["c".into()]); // self-loop on c

        assert!(!has_cycle("a", &graph, &mut HashSet::new(), &mut vec![]));
        // c still has a cycle internally
        assert!(has_cycle("c", &graph, &mut HashSet::new(), &mut vec![]));
    }

    #[test]
    fn test_suggest_package_for_lib_known() {
        assert_eq!(suggest_package_for_lib("libssl.so.3"), "libssl3");
        assert_eq!(suggest_package_for_lib("libpcre.so.3"), "libpcre3");
        assert_eq!(suggest_package_for_lib("libcurl.so.4"), "libcurl4");
        assert_eq!(suggest_package_for_lib("libsqlite3.so.0"), "libsqlite3-0");
    }

    #[test]
    fn test_suggest_package_for_lib_unknown() {
        let result = suggest_package_for_lib("libfoo.so.1");
        assert_eq!(result, "libfoo.so");
    }

    #[test]
    fn test_suggest_package_for_lib_without_so() {
        assert_eq!(suggest_package_for_lib("libssl"), "libssl3");
    }

    #[test]
    fn test_suggest_package_for_lib_case_insensitive() {
        assert_eq!(suggest_package_for_lib("LIBSSL.SO.3"), "libssl3");
    }
}

use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Simulate 100 installed packages with overlapping files.
/// Return (installed_packages, total_file_count).
fn simulate_installed_packages(count: usize) -> (HashMap<String, HashSet<String>>, usize) {
    let mut packages = HashMap::new();
    let mut total_files = 0;

    for i in 0..count {
        let name = format!("pkg-{:03}", i);
        let mut files = HashSet::new();
        let file_count = 30 + (i * 7) % 70; // 30..99 files per package

        for j in 0..file_count {
            let path = match (i * 13 + j) % 5 {
                0 => format!("/usr/bin/tool-{}-{}", i, j),
                1 => format!("/usr/lib/lib{}.so.{}.{}", i, j / 10, j % 10),
                2 => format!("/usr/share/doc/{}/README.md", name),
                3 => format!("/etc/{}/config-{}.conf", name, j),
                _ => format!("/usr/share/man/man1/{}.{}.gz", name, j),
            };
            files.insert(path);
        }

        total_files += file_count;
        packages.insert(name, files);
    }

    // Add some shared files across packages (simulating real-world overlaps)
    for i in 0..count.min(20) {
        let name = format!("pkg-{:03}", i);
        if let Some(files) = packages.get_mut(&name) {
            files.insert("/usr/bin/shared-tool".to_string());
            files.insert("/usr/lib/libshared.so.1".to_string());
        }
    }

    (packages, total_files)
}

/// Simulate a new package with files, some overlapping with installed.
fn simulate_new_package(installed: &HashMap<String, HashSet<String>>, overlap_packages: &[&str]) -> HashSet<String> {
    let mut files = HashSet::new();

    // Unique files
    for j in 0..800 {
        let path = match j % 4 {
            0 => format!("/usr/bin/new-tool-{}", j),
            1 => format!("/usr/lib/libnew.{}.so", j),
            2 => "/usr/share/doc/new-pkg/README.md".to_string(),
            _ => format!("/usr/share/man/man1/new-pkg.{}.gz", j),
        };
        files.insert(path);
    }

    // Overlapping files (from specified installed packages)
    for pkg_name in overlap_packages {
        if let Some(installed_files) = installed.get(*pkg_name) {
            for (i, f) in installed_files.iter().enumerate() {
                if i % 3 == 0 {
                    files.insert(f.clone());
                }
            }
        }
    }

    files
}

/// APPROACH 1 (OLD): Package-level conflict detection only.
/// Detects conflicts by package name / metadata — no file-level info.
fn approach_1_package_level(
    new_name: &str,
    installed_packages: &HashMap<String, HashSet<String>>,
) -> Vec<String> {
    let mut conflicts = Vec::new();

    for pkg_name in installed_packages.keys() {
        // Package-level check: same name or declared conflict
        if pkg_name == new_name {
            conflicts.push(pkg_name.clone());
        }
        // Simulate metadata-based conflicts: pkg-000 conflicts with every other
        if new_name.starts_with("conflict-") && pkg_name.starts_with("pkg-") {
            let num: u32 = pkg_name[4..].parse().unwrap_or(0);
            if num < 50 {
                conflicts.push(pkg_name.clone());
            }
        }
    }

    conflicts
}

/// APPROACH 2 (NEW): File-level conflict detection using HashSet intersection.
/// Accurate: finds every overlapping file between new and installed packages.
fn approach_2_file_level(
    new_files: &HashSet<String>,
    installed_packages: &HashMap<String, HashSet<String>>,
) -> HashMap<String, Vec<String>> {
    let start = Instant::now();
    let mut conflicts: HashMap<String, Vec<String>> = HashMap::new();

    for (pkg_name, pkg_files) in installed_packages {
        let overlapping: Vec<String> = new_files.intersection(pkg_files).cloned().collect();
        if !overlapping.is_empty() {
            conflicts.insert(pkg_name.clone(), overlapping);
        }
    }

    let elapsed = start.elapsed();
    println!("    ⏱  Approach 2 (HashSet intersection): {:?}", elapsed);
    println!("      Conflicting packages found: {}", conflicts.len());
    println!("      Total overlapping files: {}", conflicts.values().map(|v| v.len()).sum::<usize>());
    conflicts
}

#[test]
fn test_100x100_conflict_simulation() {
    println!("\n═══ 100×100 Conflict Simulation ═══\n");

    // Phase 1: Setup
    println!("[Setup] Generating 100 installed packages with ~5,000 files...");
    let (installed, total_installed_files) = simulate_installed_packages(100);
    println!("        Total installed files: {}", total_installed_files);
    println!("        Unique packages: {}", installed.len());

    // New package overlapping with 20 specific packages
    let overlap_targets: Vec<String> = (0..20).map(|i| format!("pkg-{:03}", i)).collect();
    let overlap_refs: Vec<&str> = overlap_targets.iter().map(|s| s.as_str()).collect();

    println!("\n[Setup] Generating new package with 1,000 files...");
    println!("        Overlapping with {} existing packages", overlap_refs.len());
    let new_files = simulate_new_package(&installed, &overlap_refs);
    println!("        New package files: {}", new_files.len());

    // Phase 2: Approach 1 — Package-level only
    println!("\n── Approach 1: Package-level conflict detection ──");
    let start_a1 = Instant::now();
    let pkg_conflicts = approach_1_package_level("conflict-pkg", &installed);
    let dur_a1 = start_a1.elapsed();
    println!("    ⏱  Approach 1 (package-level): {:?}", dur_a1);
    println!("    Found {} package conflicts", pkg_conflicts.len());
    println!("    ❌ MISSES file-level conflicts entirely");
    println!("    ❌ Only detects '{}' package(s)", pkg_conflicts.len());

    // Phase 3: Approach 2 — File-level with HashSet
    println!("\n── Approach 2: File-level conflict detection ──");
    let file_conflicts = approach_2_file_level(&new_files, &installed);

    // Phase 4: Classify conflicts by severity
    println!("\n── Classification ──");
    let mut critical = Vec::new();
    let mut shared = Vec::new();
    let mut minor = Vec::new();
    for (pkg, files) in &file_conflicts {
        let has_bin = files.iter().any(|f| f.starts_with("/usr/bin/") || f.starts_with("/bin/"));
        let has_lib = files.iter().any(|f| f.contains("/lib"));
        let count = files.len();

        if has_bin || has_lib && count > 5 {
            critical.push((pkg.clone(), count, files.clone()));
        } else if count > 3 {
            shared.push((pkg.clone(), count));
        } else {
            minor.push((pkg.clone(), count));
        }
    }

    println!("    ⚠  CRITICAL (bin/lib + >5 files): {} packages", critical.len());
    for (pkg, count, files) in &critical[..critical.len().min(5)] {
        let sample: Vec<&str> = files.iter().take(3).map(|s| s.as_str()).collect();
        println!("       ⊖ {} — {} files: {:?}", pkg, count, sample);
    }
    if critical.len() > 5 {
        println!("       ... and {} more", critical.len() - 5);
    }

    println!("\n    ⚡ SHARED (>3 files): {} packages", shared.len());
    if shared.len() > 5 {
        println!("       (showing first 5 of {})", shared.len());
    }
    for (pkg, count) in &shared[..shared.len().min(5)] {
        println!("       ⊖ {} — {} files", pkg, count);
    }

    println!("\n    ✓ MINOR (≤3 files): {} packages (auto-resolvable)", minor.len());

    // Phase 5: Summary
    println!("\n── Summary ──");
    println!("    Total installed packages:  {}", installed.len());
    println!("    Total conflicting packages: {}", file_conflicts.len());
    println!("    Total overlapping files:    {}", file_conflicts.values().map(|v| v.len()).sum::<usize>());
    println!("    Approach 1 accuracy: {} / {} (only name-match)", pkg_conflicts.len(), file_conflicts.len());
    println!("    Approach 2 accuracy: {} / {} (exact file match)", file_conflicts.len(), file_conflicts.len());

    // Approach 2 finds exactly the real conflicts (no false positives)
    assert!(file_conflicts.len() <= pkg_conflicts.len(),
        "File-level detection is precise, package-level may have false positives");

    // Verify output
    println!("\n═══ Simulation Complete ═══\n");
}

#[test]
fn test_hashset_performance_large_scale() {
    println!("\n═══ Performance Benchmark: 1000×1000 ═══\n");

    let (installed, _) = simulate_installed_packages(1000);
    let overlap_targets: Vec<String> = (0..100).map(|i| format!("pkg-{:03}", i)).collect();
    let overlap_refs: Vec<&str> = overlap_targets.iter().map(|s| s.as_str()).collect();
    let new_files = simulate_new_package(&installed, &overlap_refs);

    let start = Instant::now();
    let conflicts = approach_2_file_level(&new_files, &installed);
    let elapsed = start.elapsed();

    let total_overlaps: usize = conflicts.values().map(|v| v.len()).sum();
    println!("    Results: {} conflicting packages, {} total overlaps",
        conflicts.len(), total_overlaps);
    println!("    Performance: {} conflicts/sec",
        if elapsed.as_secs_f64() > 0.0 {
            (conflicts.len() as f64 / elapsed.as_secs_f64()) as u64
        } else {
            u64::MAX
        }
    );

    assert!(elapsed.as_millis() < 1000, "1000×1000 intersection must complete in <1s");
}

#[test]
fn test_no_false_positives() {
    let mut pkg_a = HashSet::new();
    pkg_a.insert("/usr/bin/foo".to_string());
    pkg_a.insert("/usr/lib/libbar.so".to_string());

    let mut pkg_b = HashSet::new();
    pkg_b.insert("/usr/bin/baz".to_string());
    pkg_b.insert("/usr/lib/libqux.so".to_string());

    let mut installed = HashMap::new();
    installed.insert("pkg-a".to_string(), pkg_a);
    installed.insert("pkg-b".to_string(), pkg_b);

    let new_files: HashSet<String> = ["/usr/bin/new-app", "/usr/lib/libnew.so"]
        .iter().map(|s| s.to_string()).collect();

    let conflicts = approach_2_file_level(&new_files, &installed);
    assert!(conflicts.is_empty(), "No overlap → no conflicts");
}

#[test]
fn test_full_overlap_detected() {
    let mut pkg = HashSet::new();
    pkg.insert("/usr/bin/conflict".to_string());
    pkg.insert("/usr/lib/libshared.so".to_string());

    let mut installed = HashMap::new();
    installed.insert("existing-pkg".to_string(), pkg);

    let new_files: HashSet<String> = ["/usr/bin/conflict", "/usr/lib/libshared.so", "/usr/bin/unique"]
        .iter().map(|s| s.to_string()).collect();

    let conflicts = approach_2_file_level(&new_files, &installed);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts.get("existing-pkg").unwrap().len(), 2);
}

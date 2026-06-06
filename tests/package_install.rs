use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const SPM_BIN: &str = "/home/ilya/projects/spm/target/debug/spm";

fn is_root() -> bool {
    let status = match fs::read_to_string("/proc/self/status") {
        Ok(s) => s,
        Err(_) => return false,
    };
    for line in status.lines() {
        if line.starts_with("Uid:") {
            if let Some(uid_str) = line.split_whitespace().nth(1) {
                return uid_str.parse::<u32>().unwrap_or(u32::MAX) == 0;
            }
        }
    }
    false
}

fn run_spm(args: &[&str]) -> std::process::Output {
    let mut cmd = if is_root() {
        let mut c = Command::new(SPM_BIN);
        c.args(args);
        c
    } else if let Ok(pw) = std::env::var("SUDO_PASSWORD") {
        let mut c = Command::new("sudo");
        c.arg("-S").arg(SPM_BIN).args(args);
        c.stdin(Stdio::piped());
        c.stdout(Stdio::piped());
        c.stderr(Stdio::piped());
        let mut child = c.spawn().expect("Failed to spawn sudo command");
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{}", pw);
        }
        return child.wait_with_output().expect("Failed to wait for sudo command");
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n").arg(SPM_BIN).args(args);
        c
    };
    cmd.output().expect("Failed to run spm command")
}

fn skip_unless_root_or_sudo() {
    if is_root() {
        return;
    }
    if let Ok(_pw) = std::env::var("SUDO_PASSWORD") {
        return;
    }
    let status = Command::new("sudo").arg("-n").arg("true").status();
    if status.map(|s| s.success()).unwrap_or(false) {
        return;
    }
    eprintln!("SKIP: not root and no sudo access. Set SUDO_PASSWORD env var or run as root.");
    std::process::exit(0);
}

fn unique_pkg_name(base: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{base}-{:x}", ts % 0xFFFF_FFFF)
}

fn create_test_sam_package(output_path: &str, pkg_name: &str) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let bin_dir = root.join("usr/bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(bin_dir.join(pkg_name), "#!/bin/sh\necho 'hello from spm test'\n").unwrap();
    fs::set_permissions(
        bin_dir.join(pkg_name),
        fs::Permissions::from_mode(0o755),
    ).unwrap();

    let manifest = spm::types::Manifest {
        name: pkg_name.into(),
        version: "1.0.0".into(),
        architecture: "amd64".into(),
        maintainer: "spm test <test@example.com>".into(),
        description: "SPM integration test package".into(),
        dependencies: vec![],
        conflicts: vec![],
        provides: vec![],
        recommends: vec![],
        install_size: 0,
        format_version: 1,
        source: Some(spm::types::PackageSource {
            original_format: "sam".into(),
            original_package: pkg_name.into(),
            repo: "local".into(),
            hash_original: String::new(),
        }),
        ai_metadata: None,
        signature: None,
        systemd_units: vec![],
        sysusers: vec![],
        tmpfiles: vec![],
        triggers: vec![],
        obsoletes: vec![],
        conffiles: vec![],
    };

    let data_tar = root.join("data.tar");
    let mut tar_builder = tar::Builder::new(fs::File::create(&data_tar).unwrap());
    tar_builder.append_dir("usr", root.join("usr")).unwrap();
    tar_builder.append_dir("usr/bin", &bin_dir).unwrap();
    tar_builder.append_path_with_name(bin_dir.join(pkg_name), format!("usr/bin/{pkg_name}")).unwrap();
    tar_builder.finish().unwrap();

    spm::package::sam::create_sam(
        &manifest,
        data_tar.to_str().unwrap(),
        None,
        output_path,
    )
    .unwrap_or_else(|e| panic!("Failed to create test SAM package: {e}"));
}

#[test]
fn test_sam_install_and_remove() {
    skip_unless_root_or_sudo();

    let pkg_name = unique_pkg_name("spmtest");
    let tmp_dir = tempfile::tempdir().unwrap();
    let sam_path = tmp_dir.path().join(format!("{pkg_name}_1.0.0_amd64.sam"));
    let sam_str = sam_path.to_str().unwrap();

    create_test_sam_package(sam_str, &pkg_name);
    assert!(sam_path.exists(), "SAM package file should exist");

    let install_output = run_spm(&["install", sam_str, "--yes"]);
    assert!(
        install_output.status.success(),
        "spm install should succeed: stderr={}",
        String::from_utf8_lossy(&install_output.stderr),
    );

    let info_output = run_spm(&["info", &pkg_name]);
    assert!(info_output.status.success());
    let info_stderr = String::from_utf8_lossy(&info_output.stderr);
    assert!(
        info_stderr.contains(&pkg_name),
        "Installed package should appear in spm info (stderr): {info_stderr}",
    );

    let remove_output = run_spm(&["remove", &pkg_name, "--yes"]);
    assert!(
        remove_output.status.success(),
        "spm remove should succeed: stderr={}",
        String::from_utf8_lossy(&remove_output.stderr),
    );

    let info2_output = run_spm(&["info", &pkg_name]);
    let info2_stderr = String::from_utf8_lossy(&info2_output.stderr);
    assert!(
        !info2_output.status.success(),
        "spm info should fail for removed package: stderr={info2_stderr}",
    );
}

#[test]
fn test_sam_install_file_not_found() {
    skip_unless_root_or_sudo();

    let output = run_spm(&["install", "/tmp/nonexistent-package-file.sam", "--yes"]);
    assert!(
        !output.status.success(),
        "Install should fail when file doesn't exist: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn test_sandbox_install_and_remove_figlet() {
    skip_unless_root_or_sudo();

    // Install figlet in sandbox. Use --replace in case sandbox already exists from failed run.
    let install_out = run_spm(&["install", "--sandbox=full", "--replace", "--yes", "figlet"]);
    if !install_out.status.success() {
        let stderr = String::from_utf8_lossy(&install_out.stderr);
        if stderr.contains("No solution for")
            || stderr.contains("not found in any repository")
            || stderr.contains("Failed to download")
        {
            eprintln!("SKIP: figlet not available in repos: {stderr}");
            return;
        }
        panic!("Sandbox install should succeed: stderr={stderr}");
    }

    // Verify sandbox directory exists
    let sandbox_path = "/var/lib/spm/sandboxes/figlet".to_string();
    assert!(
        std::path::Path::new(&sandbox_path).exists(),
        "Sandbox directory should exist",
    );

    // Verify figlet binary exists in sandbox
    let figlet_in_sandbox = format!("{sandbox_path}/usr/bin/figlet");
    if std::path::Path::new(&figlet_in_sandbox).exists() {
        let out = Command::new(&figlet_in_sandbox)
            .arg("test")
            .output()
            .expect("figlet should be executable");
        assert!(out.status.success(), "figlet in sandbox should run");
    }

    // Remove sandbox (remove the package)
    let remove_out = run_spm(&["remove", "figlet", "--yes"]);
    assert!(
        remove_out.status.success(),
        "Sandbox remove should succeed: stderr={}",
        String::from_utf8_lossy(&remove_out.stderr),
    );

    assert!(
        !std::path::Path::new(&sandbox_path).exists(),
        "Sandbox directory should be removed",
    );
}

#[test]
fn test_remove_nonexistent_package_fails() {
    skip_unless_root_or_sudo();

    let nonexistent = format!("nonexistent-pkg-{}", std::process::id());
    let output = run_spm(&["remove", &nonexistent, "--yes"]);
    assert!(
        !output.status.success(),
        "Remove should fail for nonexistent package: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn test_install_remove_roundtrip_via_dnf() {
    skip_unless_root_or_sudo();

    // Use a small package likely available in DNF repos
    let pkg = "hello";
    let install_out = run_spm(&["install", pkg, "--yes"]);
    if !install_out.status.success() {
        let stderr = String::from_utf8_lossy(&install_out.stderr);
        if stderr.contains("No solution for")
            || stderr.contains("not found in any repository")
        {
            eprintln!("SKIP: '{pkg}' not available in repos");
            return;
        }
        // Might already be installed
        if stderr.contains("already installed") {
            // Remove then re-install to test roundtrip
            let remove_out = run_spm(&["remove", pkg, "--yes"]);
            if !remove_out.status.success() {
                eprintln!("SKIP: could not remove pre-installed '{pkg}': {}", String::from_utf8_lossy(&remove_out.stderr));
                return;
            }
            let reinstall_out = run_spm(&["install", pkg, "--yes"]);
            assert!(reinstall_out.status.success(), "Re-install should succeed: stderr={}", String::from_utf8_lossy(&reinstall_out.stderr));
        } else {
            panic!("Install of '{pkg}' should succeed: stderr={stderr}");
        }
    }

    // Verify via spm info
    let info_out = run_spm(&["info", pkg]);
    assert!(info_out.status.success(), "spm info should succeed");
    let info_stderr = String::from_utf8_lossy(&info_out.stderr);
    assert!(info_stderr.contains(pkg), "Package should be in spm info (stderr)");

    // Remove
    let remove_out = run_spm(&["remove", pkg, "--yes"]);
    assert!(remove_out.status.success(), "Remove should succeed: stderr={}", String::from_utf8_lossy(&remove_out.stderr));
}

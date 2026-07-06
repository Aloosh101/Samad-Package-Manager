use std::path::Path;
use std::process::Command;

use crate::error::SpmResult;

/// Try to find a system utility at known absolute paths, falling back to PATH lookup.
/// Prevents PATH poisoning attacks.
fn find_system_cmd(name: &str) -> String {
    let candidates: &[&str] = match name {
        "dracut" => &["/usr/sbin/dracut", "/usr/bin/dracut"],
        "update-initramfs" => &["/usr/sbin/update-initramfs", "/usr/bin/update-initramfs"],
        "mkinitrd" => &["/usr/sbin/mkinitrd", "/usr/bin/mkinitrd"],
        "dkms" => &["/usr/sbin/dkms", "/usr/bin/dkms"],
        "bootctl" => &["/usr/bin/bootctl", "/usr/sbin/bootctl"],
        "grub2-mkconfig" => &["/usr/sbin/grub2-mkconfig", "/usr/bin/grub2-mkconfig"],
        "grub2-install" => &["/usr/sbin/grub2-install", "/usr/bin/grub2-install"],
        "update-grub" => &["/usr/sbin/update-grub", "/usr/bin/update-grub"],
        "grub-install" => &["/usr/sbin/grub-install", "/usr/bin/grub-install"],
        "grub-mkconfig" => &["/usr/sbin/grub-mkconfig", "/usr/bin/grub-mkconfig"],
        "lspci" => &["/usr/bin/lspci", "/usr/sbin/lspci"],
        "lsmod" => &["/usr/sbin/lsmod", "/usr/bin/lsmod"],
        _ => &[],
    };
    for &candidate in candidates {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    // Fallback to PATH lookup
    name.to_string()
}

/// Information about an installed kernel
#[derive(Debug, Clone)]
pub struct KernelInfo {
    pub version: String,
    pub vmlinuz_path: String,
    pub initramfs_path: Option<String>,
    pub system_map_path: Option<String>,
    pub config_path: Option<String>,
}

/// Scan /boot for installed kernel images
pub fn get_installed_kernels() -> Vec<KernelInfo> {
    let boot = Path::new("/boot");
    if !boot.exists() {
        return Vec::new();
    }

    let mut kernels = Vec::new();
    let entries = match std::fs::read_dir(boot) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let fname = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Match vmlinuz-{version}
        if let Some(version) = fname.strip_prefix("vmlinuz-") {
            let ver = version.to_string();
            let initramfs = find_initramfs(&ver);
            let map = if Path::new(&format!("/boot/System.map-{}", ver)).exists() {
                Some(format!("/boot/System.map-{}", ver))
            } else {
                None
            };
            let config = if Path::new(&format!("/boot/config-{}", ver)).exists() {
                Some(format!("/boot/config-{}", ver))
            } else {
                None
            };

            kernels.push(KernelInfo {
                version: ver,
                vmlinuz_path: path.to_string_lossy().to_string(),
                initramfs_path: initramfs,
                system_map_path: map,
                config_path: config,
            });
        }
    }

    kernels.sort_by(|a, b| crate::types::Version::compare(&b.version, &a.version));
    kernels
}

fn find_initramfs(version: &str) -> Option<String> {
    let candidates = [
        format!("/boot/initramfs-{}.img", version),
        format!("/boot/initrd-{}.img", version),
        format!("/boot/initrd.img-{}", version),
    ];
    for c in &candidates {
        if Path::new(c).exists() {
            return Some(c.clone());
        }
    }
    None
}

/// Extract kernel version string from its path
pub fn kernel_version_from_path(path: &str) -> Option<String> {
    let fname = Path::new(path).file_name()?.to_str()?;
    fname.strip_prefix("vmlinuz-").map(|s| s.to_string())
        .or_else(|| fname.strip_prefix("initramfs-").and_then(|s| s.strip_suffix(".img")).map(|s| s.to_string()))
        .or_else(|| fname.strip_prefix("initrd-").and_then(|s| s.strip_suffix(".img")).map(|s| s.to_string()))
        .or_else(|| fname.strip_prefix("initrd.img-").map(|s| s.to_string()))
}

/// Check if a package name looks like a kernel package
pub fn is_kernel_package(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("kernel") || lower.contains("kmod") || lower.contains("nvidia")
}

/// Rebuild DKMS modules for all installed kernels
pub fn rebuild_dkms() {
    // distro integration hook — replace with pure Rust when needed
    let output = match Command::new(find_system_cmd("dkms")).arg("status").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => {
            tracing::debug!("dkms not available, skipping DKMS rebuild");
            return;
        }
    };

    if output.trim().is_empty() {
        tracing::debug!("No DKMS modules installed, skipping");
        return;
    }

    for line in output.lines() {
        let module_ver = line.split(',').next().map(|s| s.trim()).unwrap_or("");
        if module_ver.is_empty() {
            continue;
        }
        // Validate module_ver: must match "module_name/version" pattern
        let parts: Vec<&str> = module_ver.splitn(2, '/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            tracing::warn!("Skipping invalid DKMS module entry: {module_ver}");
            continue;
        }
        // Reject entries with shell metacharacters
        if module_ver.contains(';') || module_ver.contains('`') || module_ver.contains('$') {
            tracing::warn!("Skipping suspicious DKMS module entry: {module_ver}");
            continue;
        }
        tracing::debug!("Rebuilding DKMS module: {module_ver}");
        let _ = Command::new(find_system_cmd("dkms")).args(["build", module_ver]).status();
        let _ = Command::new(find_system_cmd("dkms")).args(["install", module_ver]).status();
    }
}

/// Regenerate initramfs for a specific kernel version (or all if version is None)
pub fn regenerate_initramfs(version: Option<&str>) {
    // distro integration hooks — replace with pure Rust when needed
    if let Some(ver) = version {
        // Regenerate for a specific kernel
        if Command::new(find_system_cmd("dracut")).arg("--version").output().is_ok() {
            tracing::debug!("Regenerating initramfs for kernel {ver} with dracut");
            let _ = Command::new(find_system_cmd("dracut")).args(["-f", &format!("/boot/initramfs-{}.img", ver), ver]).status();
        } else if Command::new(find_system_cmd("update-initramfs")).arg("--version").output().is_ok() {
            tracing::debug!("Regenerating initramfs with update-initramfs");
            let _ = Command::new(find_system_cmd("update-initramfs")).args(["-u", "-k", ver]).status();
        } else if Command::new(find_system_cmd("mkinitrd")).arg("--version").output().is_ok() {
            tracing::debug!("Regenerating initramfs with mkinitrd");
            let _ = Command::new(find_system_cmd("mkinitrd")).status();
        }
    } else {
        // Regenerate all
        if Command::new(find_system_cmd("dracut")).arg("--version").output().is_ok() {
            tracing::debug!("Regenerating all initramfs with dracut");
            let _ = Command::new(find_system_cmd("dracut")).args(["-f", "--regenerate-all"]).status();
        } else if Command::new(find_system_cmd("update-initramfs")).arg("--version").output().is_ok() {
            tracing::debug!("Regenerating all initramfs with update-initramfs");
            let _ = Command::new(find_system_cmd("update-initramfs")).arg("-u").status();
        } else if Command::new(find_system_cmd("mkinitrd")).arg("--version").output().is_ok() {
            tracing::debug!("Regenerating initramfs with mkinitrd");
            let _ = Command::new(find_system_cmd("mkinitrd")).status();
        } else {
            tracing::debug!("No initramfs tool found");
        }
    }
}

/// Update bootloader configuration
pub fn update_bootloader() {
    // distro integration hooks — replace with pure Rust when needed
    // Try systemd-boot first, then GRUB
    if Path::new("/boot/efi/EFI/systemd").exists() || Path::new("/boot/efi/loader/loader.conf").exists() {
        tracing::debug!("Updating systemd-boot");
        let _ = Command::new(find_system_cmd("bootctl")).arg("update").status();
        return;
    }

    if Command::new(find_system_cmd("grub2-mkconfig")).arg("--version").output().is_ok() {
        tracing::debug!("Updating GRUB2 config");
        let _ = Command::new(find_system_cmd("grub2-mkconfig"))
            .args(["-o", "/boot/grub2/grub.cfg"])
            .status();
        // Install bootloader if needed
        if Path::new("/sys/firmware/efi").exists() {
            let _ = Command::new(find_system_cmd("grub2-install")).status();
        }
    } else if Command::new(find_system_cmd("update-grub")).output().is_ok() {
        tracing::debug!("Updating GRUB with update-grub");
        let _ = Command::new(find_system_cmd("update-grub")).status();
        if Path::new("/sys/firmware/efi").exists() {
            let _ = Command::new(find_system_cmd("grub-install")).status();
        }
    } else if Command::new(find_system_cmd("grub-mkconfig")).arg("--version").output().is_ok() {
        tracing::debug!("Updating GRUB config with grub-mkconfig");
        let _ = Command::new(find_system_cmd("grub-mkconfig"))
            .args(["-o", "/boot/grub/grub.cfg"])
            .status();
    } else {
        tracing::debug!("No bootloader update tool found");
    }
}

/// Handle kernel installation: rebuild DKMS, initramfs, bootloader
pub fn handle_kernel_install(package_name: &str) {
    if !is_kernel_package(package_name) {
        return;
    }

    tracing::info!("Kernel package '{package_name}' installed, running kernel hooks");
    rebuild_dkms();

    // Find the newest kernel and rebuild its initramfs
    let kernels = get_installed_kernels();
    if let Some(newest) = kernels.first() {
        regenerate_initramfs(Some(&newest.version));
    } else {
        regenerate_initramfs(None);
    }

    update_bootloader();
}

/// Handle kernel removal: rebuild initramfs for remaining kernels, update bootloader
pub fn handle_kernel_remove(package_name: &str) {
    if !is_kernel_package(package_name) {
        return;
    }

    tracing::info!("Kernel package '{package_name}' removed, running kernel hooks");

    // Rebuild initramfs for remaining kernels
    let kernels = get_installed_kernels();
    if let Some(newest) = kernels.first() {
        regenerate_initramfs(Some(&newest.version));
    }

    update_bootloader();
}

/// Detect graphics hardware
pub fn detect_gpu() -> SpmResult<Vec<GpuDevice>> {
    let mut gpus = Vec::new();

    // distro integration hook — replace with /sys/bus/pci/devices parsing when possible
    let output = Command::new(find_system_cmd("lspci"))
        .args(["-nn", "-d", "::0300"])
        .output()
        .map_err(|e| crate::error::SpmError::command_failed(
            format!("Failed to run lspci: {e}. Install pciutils.")
        ))?;

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        let vendor = if lower.contains("nvidia") {
            GpuVendor::Nvidia
        } else if lower.contains("amd") || lower.contains("advanced micro devices") {
            GpuVendor::Amd
        } else if lower.contains("intel") {
            GpuVendor::Intel
        } else {
            GpuVendor::Other
        };

        gpus.push(GpuDevice {
            pci_id: line.split_whitespace().next().unwrap_or("?").to_string(),
            description: line.to_string(),
            vendor: vendor.clone(),
            driver: detect_gpu_driver(&vendor),
        });
    }

    Ok(gpus)
}

fn detect_gpu_driver(vendor: &GpuVendor) -> GpuDriver {
    // distro integration hooks — replace with /proc/modules parsing when possible
    match vendor {
        GpuVendor::Nvidia => {
            if let Ok(o) = Command::new(find_system_cmd("lsmod")).output() {
                let out = String::from_utf8_lossy(&o.stdout);
                if out.contains("nvidia") {
                    if let Ok(v) = std::fs::read_to_string("/proc/driver/nvidia/version") {
                        let ver = v.lines().next().unwrap_or("?").to_string();
                        return GpuDriver::Proprietary { version: ver };
                    }
                    return GpuDriver::Proprietary { version: "unknown".into() };
                }
                if out.contains("nouveau") {
                    return GpuDriver::OpenSource { name: "nouveau".into() };
                }
            }
            GpuDriver::None
        }
        GpuVendor::Amd => {
            if let Ok(o) = Command::new(find_system_cmd("lsmod")).output() {
                let out = String::from_utf8_lossy(&o.stdout);
                if out.contains("amdgpu") {
                    return GpuDriver::OpenSource { name: "amdgpu".into() };
                }
                if out.contains("radeon") {
                    return GpuDriver::OpenSource { name: "radeon".into() };
                }
            }
            GpuDriver::None
        }
        GpuVendor::Intel => {
            if let Ok(o) = Command::new(find_system_cmd("lsmod")).output() {
                let out = String::from_utf8_lossy(&o.stdout);
                if out.contains("i915") {
                    return GpuDriver::OpenSource { name: "i915".into() };
                }
            }
            GpuDriver::None
        }
        GpuVendor::Other => GpuDriver::None,
    }
}

#[derive(Debug, Clone)]
pub struct GpuDevice {
    pub pci_id: String,
    pub description: String,
    pub vendor: GpuVendor,
    pub driver: GpuDriver,
}

#[derive(Debug, Clone)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl std::fmt::Display for GpuVendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuVendor::Nvidia => write!(f, "NVIDIA"),
            GpuVendor::Amd => write!(f, "AMD"),
            GpuVendor::Intel => write!(f, "Intel"),
            GpuVendor::Other => write!(f, "Other"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum GpuDriver {
    Proprietary { version: String },
    OpenSource { name: String },
    None,
}

impl std::fmt::Display for GpuDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuDriver::Proprietary { version } => write!(f, "Proprietary ({version})"),
            GpuDriver::OpenSource { name } => write!(f, "Open Source ({name})"),
            GpuDriver::None => write!(f, "No driver loaded"),
        }
    }
}

pub fn suggest_gpu_packages(gpu: &GpuDevice) -> Vec<String> {
    match gpu.vendor {
        GpuVendor::Nvidia => {
            vec![
                "nvidia-driver-G06".into(),
                "nvidia-driver-G06-kmp-default".into(),
                "nvidia-video-G06".into(),
            ]
        }
        GpuVendor::Amd => {
            vec![
                "kernel-firmware-amdgpu".into(),
                "libdrm_amdgpu".into(),
            ]
        }
        GpuVendor::Intel => {
            vec![
                "kernel-firmware-intel".into(),
                "libva-intel-driver".into(),
            ]
        }
        GpuVendor::Other => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_kernel_package() {
        assert!(is_kernel_package("kernel-default"));
        assert!(is_kernel_package("kernel-devel"));
        assert!(is_kernel_package("kernel-firmware"));
        assert!(is_kernel_package("kmod-nvidia"));
        assert!(is_kernel_package("nvidia-driver"));
        assert!(!is_kernel_package("nginx"));
        assert!(!is_kernel_package("libc6"));
    }

    #[test]
    fn test_kernel_version_from_path_vmlinuz() {
        let ver = kernel_version_from_path("/boot/vmlinuz-6.1.0-1-amd64");
        assert_eq!(ver, Some("6.1.0-1-amd64".into()));
    }

    #[test]
    fn test_kernel_version_from_path_initramfs() {
        let ver = kernel_version_from_path("/boot/initramfs-6.1.0-1-amd64.img");
        assert_eq!(ver, Some("6.1.0-1-amd64".into()));
    }

    #[test]
    fn test_kernel_version_from_path_initrd() {
        let ver = kernel_version_from_path("/boot/initrd-6.1.0-1-amd64.img");
        assert_eq!(ver, Some("6.1.0-1-amd64".into()));
    }

    #[test]
    fn test_kernel_version_from_path_invalid() {
        let ver = kernel_version_from_path("/etc/passwd");
        assert!(ver.is_none());
    }

    #[test]
    fn test_suggest_gpu_packages_nvidia() {
        let gpu = GpuDevice {
            pci_id: "10de:1234".into(),
            description: "NVIDIA GPU".into(),
            vendor: GpuVendor::Nvidia,
            driver: GpuDriver::None,
        };
        let pkgs = suggest_gpu_packages(&gpu);
        assert!(pkgs.iter().any(|p| p.contains("nvidia")));
        assert_eq!(pkgs.len(), 3);
    }

    #[test]
    fn test_suggest_gpu_packages_amd() {
        let gpu = GpuDevice {
            pci_id: "1002:5678".into(),
            description: "AMD GPU".into(),
            vendor: GpuVendor::Amd,
            driver: GpuDriver::None,
        };
        let pkgs = suggest_gpu_packages(&gpu);
        assert!(pkgs.iter().any(|p| p.contains("amdgpu") || p.contains("drm")));
    }

    #[test]
    fn test_suggest_gpu_packages_intel() {
        let gpu = GpuDevice {
            pci_id: "8086:9bc4".into(),
            description: "Intel GPU".into(),
            vendor: GpuVendor::Intel,
            driver: GpuDriver::None,
        };
        let pkgs = suggest_gpu_packages(&gpu);
        assert!(pkgs.iter().any(|p| p.contains("intel")));
    }

    #[test]
    fn test_suggest_gpu_packages_other() {
        let gpu = GpuDevice {
            pci_id: "1af4:1050".into(),
            description: "VirtIO GPU".into(),
            vendor: GpuVendor::Other,
            driver: GpuDriver::None,
        };
        let pkgs = suggest_gpu_packages(&gpu);
        assert!(pkgs.is_empty());
    }

    #[test]
    fn test_gpu_vendor_display() {
        assert_eq!(format!("{}", GpuVendor::Nvidia), "NVIDIA");
        assert_eq!(format!("{}", GpuVendor::Amd), "AMD");
        assert_eq!(format!("{}", GpuVendor::Intel), "Intel");
        assert_eq!(format!("{}", GpuVendor::Other), "Other");
    }

    #[test]
    fn test_gpu_driver_display() {
        assert_eq!(format!("{}", GpuDriver::None), "No driver loaded");
        assert_eq!(
            format!("{}", GpuDriver::OpenSource { name: "nouveau".into() }),
            "Open Source (nouveau)"
        );
        assert_eq!(
            format!("{}", GpuDriver::Proprietary { version: "535".into() }),
            "Proprietary (535)"
        );
    }
}

use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::paths;
use crate::error::{SpmError, SpmResult};
use crate::package::store::sanitize_relative;

const SCRIPT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Default, Clone)]
pub struct Scripts {
    pub preinst: Option<String>,
    pub postinst: Option<String>,
    pub prerm: Option<String>,
    pub postrm: Option<String>,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.preinst.is_none()
            && self.postinst.is_none()
            && self.prerm.is_none()
            && self.postrm.is_none()
    }
}

fn read_script(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

pub fn extract_deb_scripts(path: &str, scripts_dir: &str) -> SpmResult<Scripts> {
    let backend = crate::util::backend::resolve("dpkg-deb");
    let result = extract_deb_scripts_with(path, scripts_dir, &backend);
    match result {
        Ok(scripts) => return Ok(scripts),
        Err(_) => {
            // Fall back to host dpkg-deb directly (not via chroot wrapper)
            if let Ok(scripts) = extract_deb_scripts_with(path, scripts_dir, &PathBuf::from("dpkg-deb")) {
                return Ok(scripts);
            }
        }
    }

    tracing::warn!("dpkg-deb -e failed for '{path}'. Trying manual extraction...");
    extract_deb_scripts_fallback(path, scripts_dir)
}

fn extract_deb_scripts_with(path: &str, scripts_dir: &str, cmd: &Path) -> SpmResult<Scripts> {
    let output = Command::new(cmd)
        .args(["-e", path, scripts_dir])
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run dpkg-deb -e: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SpmError::invalid_format(format!(
            "dpkg-deb -e failed: {}", stderr.trim()
        )));
    }

    let scripts_dir = PathBuf::from(scripts_dir);
    let mut scripts = Scripts::default();

    for name in &["preinst", "postinst", "prerm", "postrm"] {
        let script_path = scripts_dir.join(name);
        if script_path.exists() {
            let content = fs::read_to_string(&script_path).ok();
            match *name {
                "preinst" => scripts.preinst = content,
                "postinst" => scripts.postinst = content,
                "prerm" => scripts.prerm = content,
                "postrm" => scripts.postrm = content,
                _ => {}
            }
        }
    }

    Ok(scripts)
}

fn extract_deb_scripts_fallback(path: &str, scripts_dir: &str) -> SpmResult<Scripts> {
    let mut file = fs::File::open(path).map_err(|e|
        SpmError::invalid_format(format!("Failed to open .deb file {path}: {e}")
    ))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).map_err(|e|
        SpmError::invalid_format(format!("Failed to read ar magic: {e}")
    ))?;
    if &magic != b"!<arch>\n" {
        return Err(SpmError::invalid_format("Invalid .deb file: not an ar archive"));
    }

    fn read_ar_header(reader: &mut dyn Read) -> SpmResult<Option<(String, usize)>> {
        let mut header = [0u8; 60];
        match reader.read_exact(&mut header) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(SpmError::other(format!("Failed to read ar header: {e}"))),
        }
        let name = String::from_utf8_lossy(&header[0..16]).trim().to_string();
        let size_str = std::str::from_utf8(&header[48..58]).unwrap_or("0").trim();
        let size: usize = size_str.parse().unwrap_or(0);
        Ok(Some((name, size)))
    }

    fn read_ar_data(reader: &mut dyn Read, size: usize) -> SpmResult<Vec<u8>> {
        let mut data = vec![0u8; size];
        reader.read_exact(&mut data).map_err(|e|
            SpmError::other(format!("Failed to read ar data: {e}")
        ))?;
        if size % 2 == 1 {
            let mut pad = [0u8; 1];
            reader.read_exact(&mut pad).map_err(|e|
                SpmError::other(format!("Failed to read ar padding: {e}")
            ))?;
        }
        Ok(data)
    }

    let scripts_dir = PathBuf::from(scripts_dir);
    fs::create_dir_all(&scripts_dir)?;

    while let Some((name, size)) = read_ar_header(&mut file)? {
        let data = read_ar_data(&mut file, size)?;
        if name.starts_with("control.tar") {
            let mut decompressed = Vec::new();
            if let Ok(mut decoder) = zstd::Decoder::new(&data[..]) {
                decoder.read_to_end(&mut decompressed)?;
            } else {
                let mut decoder = flate2::read::GzDecoder::new(&data[..]);
                decoder.read_to_end(&mut decompressed)?;
            }
            if !decompressed.is_empty() {
                let mut archive = tar::Archive::new(&decompressed[..]);
                for entry in archive.entries()? {
                    let mut entry = entry?;
                    if let Ok(name) = entry.path().map(|p| p.to_string_lossy().to_string()) {
                        let clean = name.strip_prefix("./").unwrap_or(&name);
                        if clean == "preinst" || clean == "postinst"
                            || clean == "prerm" || clean == "postrm"
                        {
                            let target = scripts_dir.join(clean);
                            sanitize_relative(&target)?;
                            entry.unpack(&target)?;
                        }
                    }
                }
            }
            break;
        }
    }

    let mut scripts = Scripts::default();
    for script_name in &["preinst", "postinst", "prerm", "postrm"] {
        let script_path = scripts_dir.join(script_name);
        if script_path.exists() {
            let content = read_script(&script_path);
            match *script_name {
                "preinst" => scripts.preinst = content,
                "postinst" => scripts.postinst = content,
                "prerm" => scripts.prerm = content,
                "postrm" => scripts.postrm = content,
                _ => {}
            }
        }
    }

    Ok(scripts)
}

pub fn extract_sam_scripts(sam_path: &str, scripts_dir: &str) -> SpmResult<Scripts> {
    let mut file = fs::File::open(sam_path).map_err(|e|
        SpmError::invalid_format(format!("Failed to open .sam file {sam_path}: {e}")
    ))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"SAM1" {
        return Err(SpmError::invalid_format("Invalid .sam file: bad magic"));
    }

    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf)?;
    let manifest_len = u32::from_le_bytes(len_buf) as usize;

    let mut manifest_bytes = vec![0u8; manifest_len];
    file.read_exact(&mut manifest_bytes)?;

    let scripts_dir = PathBuf::from(scripts_dir);
    fs::create_dir_all(&scripts_dir)?;

    let data_tar_path = scripts_dir.join(".data.tar");
    {
        let mut decoder = zstd::Decoder::new(&file)
            .map_err(|e| SpmError::compression(format!("Failed to create zstd decoder: {e}")))?;
        let mut data_file = fs::File::create(&data_tar_path)
            .map_err(|e| SpmError::other(format!("Failed to create data.tar: {e}")))?;
        std::io::copy(&mut decoder, &mut data_file)
            .map_err(|e| SpmError::compression(format!("Failed to decompress data: {e}")))?;
    }
    let _ = fs::remove_file(&data_tar_path);

    let mut scripts = Scripts::default();
    let mut meta_decoder = zstd::Decoder::new(&file)
        .map_err(|e| SpmError::compression(format!("Failed to create meta zstd decoder: {e}")))?;
    let mut meta_bytes = Vec::new();
    if meta_decoder.read_to_end(&mut meta_bytes).is_ok() && !meta_bytes.is_empty() {
        let mut archive = tar::Archive::new(&meta_bytes[..]);
        for entry in archive.entries()? {
            let mut entry = entry?;
            if let Ok(name) = entry.path().map(|p| p.to_string_lossy().to_string()) {
                let clean = name.strip_prefix("./").unwrap_or(&name);
                let script_name = clean.strip_suffix(".sh").unwrap_or(clean);
                if script_name == "preinst" || script_name == "postinst"
                    || script_name == "prerm" || script_name == "postrm"
                {
                    let target = scripts_dir.join(clean);
                    sanitize_relative(&target)?;
                    entry.unpack(&target)?;
                }
            }
        }
    }

    for script_name in &["preinst.sh", "postinst.sh", "prerm.sh", "postrm.sh"] {
        let script_path = scripts_dir.join(script_name);
        if script_path.exists() {
            let key = script_name.strip_suffix(".sh").unwrap_or(script_name);
            let content = read_script(&script_path);
            match key {
                "preinst" => scripts.preinst = content,
                "postinst" => scripts.postinst = content,
                "prerm" => scripts.prerm = content,
                "postrm" => scripts.postrm = content,
                _ => {}
            }
        }
    }

    Ok(scripts)
}

pub fn extract_rpm_scripts(path: &str) -> SpmResult<Scripts> {
    let mut file = fs::File::open(path).map_err(|e|
        SpmError::invalid_format(format!("Failed to open .rpm file {path}: {e}")
    ))?;
    let mut lead = [0u8; 96];
    file.read_exact(&mut lead)?;
    if &lead[0..4] != b"\xed\xab\xee\xdb" {
        return Err(SpmError::invalid_format("Invalid .rpm file: bad magic"));
    }

    fn read_index_data(file: &mut fs::File, _header_pad: u16) -> SpmResult<Vec<u8>> {
        let mut header = [0u8; 16];
        file.read_exact(&mut header)?;
        let nindex = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
        let hsize = u32::from_be_bytes([header[12], header[13], header[14], header[15]]);
        let store_size = (nindex as usize * 16 + hsize as usize + 7) & !7;
        let mut store = vec![0u8; store_size];
        file.read_exact(&mut store)?;
        let mut result = header.to_vec();
        result.extend(store);
        Ok(result)
    }

    let _sig_data = read_index_data(&mut file, 16)?;
    let hdr_data = read_index_data(&mut file, 16)?;

    let nindex = u32::from_be_bytes([hdr_data[8], hdr_data[9], hdr_data[10], hdr_data[11]]);
    let store_offset = 16 + (nindex as usize) * 16;

    let mut offset = 16usize;
    let mut tags: Vec<(i32, i32, i32, i32)> = Vec::new();
    for _ in 0..nindex {
        if offset + 16 > hdr_data.len() {
            break;
        }
        let tag = i32::from_be_bytes([
            hdr_data[offset], hdr_data[offset + 1], hdr_data[offset + 2], hdr_data[offset + 3],
        ]);
        let ty = i32::from_be_bytes([
            hdr_data[offset + 4], hdr_data[offset + 5], hdr_data[offset + 6], hdr_data[offset + 7],
        ]);
        let val_offset = i32::from_be_bytes([
            hdr_data[offset + 8], hdr_data[offset + 9], hdr_data[offset + 10], hdr_data[offset + 11],
        ]);
        let count = i32::from_be_bytes([
            hdr_data[offset + 12], hdr_data[offset + 13], hdr_data[offset + 14], hdr_data[offset + 15],
        ]);
        offset += 16;
        tags.push((tag, ty, val_offset, count));
    }

    let read_string = |val_offset: i32| -> String {
        let start = store_offset + val_offset as usize;
        if start >= hdr_data.len() {
            return String::new();
        }
        let end = hdr_data[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|pos| start + pos)
            .unwrap_or(hdr_data.len());
        String::from_utf8_lossy(&hdr_data[start..end]).to_string()
    };

    let mut scripts = Scripts::default();

    for &(tag, ty, val_offset, _count) in &tags {
        match tag {
            1023 if ty == 8 || ty == 9 => scripts.preinst = Some(read_string(val_offset)),
            1024 if ty == 8 || ty == 9 => scripts.postinst = Some(read_string(val_offset)),
            1025 if ty == 8 || ty == 9 => scripts.prerm = Some(read_string(val_offset)),
            1026 if ty == 8 || ty == 9 => scripts.postrm = Some(read_string(val_offset)),
            _ => {}
        }
    }

    Ok(scripts)
}

pub fn run_script_in_sandbox(content: &str, phase: &str, sandbox_dir: &str) -> SpmResult<()> {
    if content.trim().is_empty() {
        return Ok(());
    }

    let script_dir = Path::new(sandbox_dir).join("tmp");
    fs::create_dir_all(&script_dir)
        .map_err(|e| SpmError::other(format!("Failed to create sandbox tmp dir: {e}")))?;
    let script_path = script_dir.join("script.sh");
    fs::write(&script_path, content.as_bytes())
        .map_err(|e| SpmError::other(format!("Failed to write script: {e}")))?;

    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;

    // Ensure /bin/sh exists inside sandbox
    let sandbox_sh = Path::new(sandbox_dir).join("bin").join("sh");
    if !sandbox_sh.exists() {
        let sandbox_bin = Path::new(sandbox_dir).join("bin");
        fs::create_dir_all(&sandbox_bin)?;
        let _ = fs::copy("/bin/sh", &sandbox_sh);
    }

    let output = Command::new("timeout")
        .arg(SCRIPT_TIMEOUT_SECS.to_string())
        .arg("chroot")
        .arg(sandbox_dir)
        .arg("/bin/sh")
        .arg("/tmp/script.sh")
        .arg(phase)
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to execute {phase} script in sandbox: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SpmError::other(format!(
            "{phase} script failed in sandbox (exit code: {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }

    Ok(())
}

pub fn run_script(content: &str, phase: &str) -> SpmResult<()> {
    if content.trim().is_empty() {
        return Ok(());
    }

    let tmp_dir_obj = tempfile::tempdir()
        .map_err(|e| SpmError::other(format!("Failed to create temp directory: {e}")))?;
    let script_path = tmp_dir_obj.path().join("script.sh");
    fs::write(&script_path, content.as_bytes())
        .map_err(|e| SpmError::other(format!("Failed to write script: {e}")))?;

    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;

    let output = unsafe {
        Command::new("timeout")
            .arg(SCRIPT_TIMEOUT_SECS.to_string())
            .arg("/bin/sh")
            .arg(&script_path)
            .arg(phase)
            .pre_exec(|| {
                isolate_child_process()
            })
            .output()
    }
    .map_err(|e| SpmError::command_failed(format!("Failed to execute {phase} script: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SpmError::other(format!(
            "{phase} script failed (exit code: {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }

    Ok(())
}

/// Apply security isolation in the child process after fork but before exec.
/// Called from `pre_exec` — must use only async-signal-safe syscalls.
fn isolate_child_process() -> Result<(), std::io::Error> {
    // 1. Prevent privilege escalation
    unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0); }

    // 2. Drop all capabilities from the bounding set via PR_CAPBSET_DROP
    for cap in 0..64u64 {
        unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0); }
    }

    // 3. Resource limits
    let nofile = libc::rlimit { rlim_cur: 1024, rlim_max: 1024 };
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &nofile); }
    let nproc = libc::rlimit { rlim_cur: 64, rlim_max: 64 };
    unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &nproc); }
    let fsize = libc::rlimit { rlim_cur: 10 * 1024 * 1024, rlim_max: 10 * 1024 * 1024 };
    unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &fsize); }
    let as_limit = libc::rlimit { rlim_cur: 512 * 1024 * 1024, rlim_max: 512 * 1024 * 1024 };
    unsafe { libc::setrlimit(libc::RLIMIT_AS, &as_limit); }

    // 4. Isolate PID namespace (requires CAP_SYS_ADMIN, still available before cap drop)
    unsafe { libc::unshare(libc::CLONE_NEWPID); }

    Ok(())
}

pub fn save_scripts(name: &str, scripts: &Scripts) -> SpmResult<()> {
    let scripts_dir = paths::scripts_dir().join(name);
    fs::create_dir_all(&scripts_dir)?;

    if let Some(ref content) = scripts.preinst {
        fs::write(scripts_dir.join("preinst"), content)?;
    }
    if let Some(ref content) = scripts.postinst {
        fs::write(scripts_dir.join("postinst"), content)?;
    }
    if let Some(ref content) = scripts.prerm {
        fs::write(scripts_dir.join("prerm"), content)?;
    }
    if let Some(ref content) = scripts.postrm {
        fs::write(scripts_dir.join("postrm"), content)?;
    }

    Ok(())
}

pub fn load_scripts(name: &str) -> SpmResult<Scripts> {
    let scripts_dir = paths::scripts_dir().join(name);

    if !scripts_dir.exists() {
        return Ok(Scripts::default());
    }

    let mut scripts = Scripts::default();

    for script_name in &["preinst", "postinst", "prerm", "postrm"] {
        let path = scripts_dir.join(script_name);
        if path.exists() {
            let content = read_script(&path);
            match *script_name {
                "preinst" => scripts.preinst = content,
                "postinst" => scripts.postinst = content,
                "prerm" => scripts.prerm = content,
                "postrm" => scripts.postrm = content,
                _ => {}
            }
        }
    }

    Ok(scripts)
}

pub fn remove_scripts(name: &str) -> SpmResult<()> {
    let scripts_dir = paths::scripts_dir().join(name);
    if scripts_dir.exists() {
        fs::remove_dir_all(&scripts_dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scripts_is_empty_all_none() {
        let s = Scripts::default();
        assert!(s.is_empty());
    }

    #[test]
    fn test_scripts_is_empty_with_preinst() {
        let s = Scripts { preinst: Some("echo hello".into()), ..Default::default() };
        assert!(!s.is_empty());
    }

    #[test]
    fn test_scripts_is_empty_with_postinst() {
        let s = Scripts { postinst: Some("echo done".into()), ..Default::default() };
        assert!(!s.is_empty());
    }

    #[test]
    fn test_scripts_is_empty_with_prerm() {
        let s = Scripts { prerm: Some("echo remove".into()), ..Default::default() };
        assert!(!s.is_empty());
    }

    #[test]
    fn test_scripts_is_empty_with_postrm() {
        let s = Scripts { postrm: Some("echo purge".into()), ..Default::default() };
        assert!(!s.is_empty());
    }
}

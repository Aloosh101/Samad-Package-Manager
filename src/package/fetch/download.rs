use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use crate::error::{SpmError, SpmResult};

pub(crate) fn run_with_progress(
    cmd: &mut Command,
    label: &str,
) -> std::io::Result<std::process::Output> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().expect("failed to capture stdout");
    let stderr = child.stderr.take().expect("failed to capture stderr");

    let (tx, rx) = mpsc::channel::<String>();
    let handle = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for l in reader.lines().map_while(Result::ok) {
            if !l.is_empty() && tx.send(l).is_err() {
                break;
            }
        }
    });

    let mut spinner = crate::output::Spinner::new(label);
    loop {
        match rx.try_recv() {
            Ok(line) => spinner.message(&line),
            Err(mpsc::TryRecvError::Empty) => {
                if child.try_wait()?.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }

    let _ = handle.join();
    let err_output = BufReader::new(stderr).lines().collect::<Result<Vec<_>, _>>()?;
    let status = child.wait()?;

    spinner.finish();

    Ok(std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: err_output.join("\n").into_bytes(),
    })
}

pub(crate) fn retry_download_resumable(url: &str, output_path: &Path, name: &str) -> SpmResult<u64> {
    let max_attempts = 3;
    let mut existing_size = if output_path.exists() {
        fs::metadata(output_path).map(|m| m.len()).unwrap_or(0)
    } else {
        fs::create_dir_all(output_path.parent().unwrap_or(Path::new("")))?;
        0
    };

    'resume_retry: for attempt in 1..=max_attempts {
        tracing::debug!("Download attempt {attempt}/{max_attempts}: {url} (offset={existing_size})");
        let mut req = ureq::get(url);
        if existing_size > 0 {
            req = req.header("Range", &format!("bytes={existing_size}-"));
        }

        match req.call() {
            Ok(response) => {
                let status = response.status();
                if status == 416 {
                    let _ = fs::remove_file(output_path);
                    existing_size = 0;
                    eprintln!("  ⚠ Cache invalid for '{name}', re-downloading from scratch...");
                    continue;
                }
                let total: Option<u64> = if status == 206 {
                    let cr = response.headers()
                        .get("content-range")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    cr.rsplit('/').next()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                } else {
                    response.headers()
                        .get("content-length")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse::<u64>().ok())
                };

                let mut file = if status == 206 && existing_size > 0 {
                    fs::OpenOptions::new().append(true).open(output_path)?
                } else {
                    fs::File::create(output_path)?
                };

                let mut reader = response.into_body().into_reader();
                let mut buf = vec![0u8; 16384];
                let mut written = 0u64;
                let base = if status == 206 { existing_size } else { 0 };
                let mut pb = crate::output::ProgressBar::with_total(
                    format!("Downloading {name}"),
                    total.unwrap_or(0),
                );

                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) => {
                            if attempt < max_attempts {
                                pb.fail(format!("Read error, retrying ({attempt}/{max_attempts})..."));
                                std::thread::sleep(std::time::Duration::from_secs(2));
                                continue 'resume_retry;
                            }
                            return Err(SpmError::network(format!(
                                "Read error for '{name}' after {attempt} attempts: {e}"
                            )));
                        }
                    };
                    file.write_all(&buf[..n])?;
                    written += n as u64;
                    pb.tick_by(n as u64);
                    pb.update();
                }

                if status == 206 && existing_size > 0 {
                    pb.finish_resumed(name, existing_size);
                } else {
                    pb.finish(name);
                }
                return Ok(base + written);
            }
            Err(e) => {
                if matches!(&e, ureq::Error::StatusCode(416)) {
                    let _ = fs::remove_file(output_path);
                    existing_size = 0;
                    eprintln!("  ⚠ Cache invalid for '{name}', re-downloading from scratch...");
                    continue;
                }
                if attempt < max_attempts {
                    eprintln!("  ⚠ Download failed on attempt {attempt} for '{name}': {e}. Retrying...");
                    std::thread::sleep(std::time::Duration::from_secs(2));
                } else {
                    return Err(SpmError::network(format!(
                        "Failed to download '{name}' after {max_attempts} attempts from {url}: {e}"
                    )));
                }
            }
        }
    }
    Err(SpmError::network(format!(
        "Failed to download '{name}' after {max_attempts} attempts from {url}"
    )))
}

pub(crate) fn find_deb_in_cache(cache_dir: &str, name: &str) -> SpmResult<String> {
    let entries = fs::read_dir(cache_dir)?;
    let prefix = format!("{name}_");
    for entry in entries {
        let entry = entry?;
        let p = entry.path();
        if let Some(name_str) = p.file_name().and_then(|n| n.to_str()) {
            if name_str.starts_with(&prefix) && name_str.ends_with(".deb") {
                return Ok(p.to_string_lossy().to_string());
            }
        }
    }
    Err(SpmError::other(format!(
        "Deb package for '{name}' not found in cache"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_find_deb_in_cache_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nginx_1.24.0_amd64.deb");
        fs::write(&path, b"dummy").unwrap();

        let result = find_deb_in_cache(dir.path().to_str().unwrap(), "nginx");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("nginx_1.24.0_amd64.deb"));
    }

    #[test]
    fn test_find_deb_in_cache_not_found() {
        let dir = tempfile::tempdir().unwrap();
        // Create a non-matching file
        fs::write(dir.path().join("other.deb"), b"data").unwrap();

        let result = find_deb_in_cache(dir.path().to_str().unwrap(), "nginx");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_deb_in_cache_wrong_prefix() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("nginx.deb"), b"data").unwrap();

        let result = find_deb_in_cache(dir.path().to_str().unwrap(), "nginx");
        // nginx.deb doesn't have the format nginx_<version>_arch.deb
        // but it starts with "nginx_" which is the prefix check with no underscore
        // Actually the prefix is "nginx_" and "nginx.deb" doesn't start with "nginx_"
        assert!(result.is_err());
    }

    #[test]
    fn test_find_deb_in_cache_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_deb_in_cache(dir.path().to_str().unwrap(), "nginx");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_deb_in_cache_nonexistent_dir() {
        let result = find_deb_in_cache("/nonexistent-cache-dir-xyz", "nginx");
        assert!(result.is_err());
    }
}

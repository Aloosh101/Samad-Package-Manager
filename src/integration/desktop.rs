use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::error::SpmResult;

pub fn update_desktop_database(app_dir: &Path) -> SpmResult<()> {
    if !app_dir.exists() {
        return Ok(());
    }

    let mut mime_map: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let entries = match fs::read_dir(app_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
            continue;
        }
        if !path.is_file() {
            continue;
        }

        let basename = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut in_desktop_entry = false;
        let mut mime_types: Option<String> = None;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == "[Desktop Entry]" {
                in_desktop_entry = true;
                continue;
            }
            if in_desktop_entry {
                if trimmed.starts_with('[') {
                    break;
                }
                if let Some(value) = trimmed.strip_prefix("MimeType=") {
                    mime_types = Some(value.to_string());
                    break;
                }
            }
        }

        if let Some(mime_str) = mime_types {
            for mime in mime_str.split(';') {
                let mime = mime.trim();
                if mime.is_empty() {
                    continue;
                }
                mime_map
                    .entry(mime.to_string())
                    .or_default()
                    .push(basename.clone());
            }
        }
    }

    if mime_map.is_empty() {
        return Ok(());
    }

    let cache_path = app_dir.join("mimeinfo.cache");
    let mut output = String::from("[MIME Cache]\n");
    for (mime, apps) in &mime_map {
        output.push_str(&format!("{}={}\n", mime, apps.join(";")));
    }

    fs::write(&cache_path, output.as_bytes())?;

    Ok(())
}

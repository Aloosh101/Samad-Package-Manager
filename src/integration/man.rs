use std::fs;
use std::io::Read;
use std::path::Path;

use crate::error::SpmResult;

pub fn update_man_db(mandir: &Path) -> SpmResult<()> {
    if !mandir.exists() {
        return Ok(());
    }

    let mut whatis_entries = Vec::new();

    for entry in walkdir::WalkDir::new(mandir)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(f) => f,
            None => continue,
        };

        if !filename.ends_with(".gz") && !filename.ends_with(".bz2") && !filename.ends_with(".xz") {
            continue;
        }

        let name_part = if filename.ends_with(".gz") {
            &filename[..filename.len() - 3]
        } else if filename.ends_with(".bz2") {
            &filename[..filename.len() - 4]
        } else {
            &filename[..filename.len() - 3]
        };

        if let Some((section, _rest)) = name_part.split_once('.') {
            let man_name = section;
            let content = read_compressed_man(path);
            if let Some(description) = extract_name_description(&content) {
                whatis_entries
                    .push(format!("{} ({})  - {}\n", man_name, section, description));
            }
        }
    }

    if whatis_entries.is_empty() {
        return Ok(());
    }

    let whatis_path = mandir.join("whatis");
    fs::write(&whatis_path, whatis_entries.concat())?;

    Ok(())
}

fn read_compressed_man(path: &Path) -> String {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };

    if path.to_string_lossy().ends_with(".gz") {
        let mut decompressed = Vec::new();
        if flate2::read::GzDecoder::new(&bytes[..])
            .read_to_end(&mut decompressed)
            .is_ok()
        {
            return String::from_utf8_lossy(&decompressed).into_owned();
        }
    }

    String::from_utf8_lossy(&bytes).into_owned()
}

fn extract_name_description(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(r".TH ") {
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix(r".SH NAME")
            .or_else(|| trimmed.strip_prefix(r".SH NAME"))
        {
            let name_line = rest.trim();
            if !name_line.is_empty() {
                let desc = name_line
                    .trim_start_matches('\\')
                    .trim_start_matches(char::from(0))
                    .trim();
                if desc.contains(" - ") {
                    return Some(desc.to_string());
                }
                return Some(desc.to_string());
            }
        }
        if let Some(desc) = trimmed.strip_prefix(r"NAME") {
            let desc = desc.trim();
            if desc.contains(" - ") {
                return Some(desc.to_string());
            }
        }
    }

    let mut in_name = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.to_uppercase().starts_with(".SH NAME") || trimmed.to_uppercase() == ".SH NAME" {
            in_name = true;
            continue;
        }
        if in_name {
            if trimmed.starts_with('.') && !trimmed.starts_with(r"\. ") {
                break;
            }
            let clean = trimmed
                .trim_start_matches(r"\fB")
                .trim_end_matches(r"\fR")
                .trim_start_matches(r"\fI")
                .trim();
            if clean.contains(" - ") {
                return Some(clean.to_string());
            }
        }
    }

    None
}

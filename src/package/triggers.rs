use std::fs;
use std::path::Path;

use crate::config::paths;
use crate::error::SpmResult;
use crate::types::Manifest;

/// Store trigger scripts from a package's manifest so they can be run later.
/// Trigger metadata is a JSON file; each trigger script is saved as a file.
pub fn store_triggers(package_name: &str, manifest: &Manifest, extracted_dir: &str) -> SpmResult<()> {
    if manifest.triggers.is_empty() {
        return Ok(());
    }
    let pkg_triggers_dir = paths::triggers_dir().join(package_name);
    fs::create_dir_all(&pkg_triggers_dir)
        .map_err(|e| crate::error::SpmError::other(format!("Cannot create triggers dir: {e}")))?;

    for (i, trigger) in manifest.triggers.iter().enumerate() {
        let trigger_dir = pkg_triggers_dir.join(i.to_string());
        fs::create_dir_all(&trigger_dir)
            .map_err(|e| crate::error::SpmError::other(format!("Cannot create trigger dir: {e}")))?;

        // Save trigger metadata
        let meta = serde_json::json!({
            "on": trigger.on,
            "pattern": trigger.pattern,
            "script": trigger.script,
        });
        fs::write(trigger_dir.join("meta.json"), serde_json::to_string_pretty(&meta).unwrap())
            .map_err(|e| crate::error::SpmError::other(format!("Cannot write trigger meta: {e}")))?;

        // Copy the trigger script from the extracted package
        let script_src = Path::new(extracted_dir).join(&trigger.script);
        if script_src.exists() {
            let script_content = fs::read_to_string(&script_src)
                .unwrap_or_default();
            fs::write(trigger_dir.join("script"), &script_content)
                .map_err(|e| crate::error::SpmError::other(format!("Cannot write trigger script: {e}")))?;
        }
    }
    Ok(())
}

/// Remove all stored triggers for a package.
pub fn remove_triggers(package_name: &str) {
    let pkg_triggers_dir = paths::triggers_dir().join(package_name);
    if pkg_triggers_dir.exists() {
        let _ = fs::remove_dir_all(&pkg_triggers_dir);
    }
}

/// Run triggers that match the given package names for the given event.
/// `event` is "install" or "remove".
pub fn run_triggers(event: &str, package_names: &[String]) {
    let triggers_dir = paths::triggers_dir();
    if !triggers_dir.exists() {
        return;
    }

    let Ok(read_dir) = fs::read_dir(&triggers_dir) else { return };

    for entry in read_dir.flatten() {
        let pkg_dir = entry.path();
        if !pkg_dir.is_dir() {
            continue;
        }

        let Ok(trigger_dir_reader) = fs::read_dir(&pkg_dir) else { continue };
        for trigger_entry in trigger_dir_reader.flatten() {
            let trigger_dir = trigger_entry.path();
            if !trigger_dir.is_dir() {
                continue;
            }

            let meta_path = trigger_dir.join("meta.json");
            let script_path = trigger_dir.join("script");
            if !meta_path.exists() || !script_path.exists() {
                continue;
            }

            let meta_content = match fs::read_to_string(&meta_path) {
                Ok(c) => c,
                _ => continue,
            };
            let meta: serde_json::Value = match serde_json::from_str(&meta_content) {
                Ok(v) => v,
                _ => continue,
            };

            let trigger_on = meta["on"].as_str().unwrap_or("");
            let pattern = meta["pattern"].as_str().unwrap_or("");
            let script_rel = meta["script"].as_str().unwrap_or("");

            if trigger_on != event {
                continue;
            }

            for pkg_name in package_names {
                if glob_match(pattern, pkg_name) {
                    crate::output::step_info(format!(
                        "Running trigger '{}' from '{}' on '{}' ({})",
                        script_rel,
                        pkg_dir.file_name().unwrap_or_default().to_string_lossy(),
                        pkg_name,
                        event,
                    ));
                    let script_content = match fs::read_to_string(&script_path) {
                        Ok(c) => c,
                        _ => continue,
                    };
                    let _ = crate::package::scripts::run_script(&script_content, &format!("trigger-{event}"));
                    break;
                }
            }
        }
    }
}

/// Simple glob matching (supports `*` and `?`).
fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    glob_match_inner(&pattern_chars, &name_chars, 0, 0)
}

fn glob_match_inner(p: &[char], s: &[char], pi: usize, si: usize) -> bool {
    if pi == p.len() {
        return si == s.len();
    }
    if si == s.len() {
        return p[pi..].iter().all(|&c| c == '*');
    }
    match p[pi] {
        '?' => glob_match_inner(p, s, pi + 1, si + 1),
        '*' => {
            glob_match_inner(p, s, pi + 1, si)
                || glob_match_inner(p, s, pi, si + 1)
        }
        c if c == s[si] => glob_match_inner(p, s, pi + 1, si + 1),
        _ => false,
    }
}

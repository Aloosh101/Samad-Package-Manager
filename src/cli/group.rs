use std::fs;
use std::path::Path;

use crate::error::{SpmError, SpmResult};
use crate::output;

const SPM_GROUP: &str = "spm";
const GROUP_FILE: &str = "/etc/group";
const GSHADOW_FILE: &str = "/etc/gshadow";

fn group_exists_in_content(content: &str, name: &str) -> bool {
    content.lines().any(|l| l.starts_with(&format!("{name}:")))
}

fn group_exists() -> bool {
    fs::read_to_string(GROUP_FILE)
        .map(|c| group_exists_in_content(&c, SPM_GROUP))
        .unwrap_or(false)
}

fn group_gid() -> Option<u32> {
    let content = fs::read_to_string(GROUP_FILE).ok()?;
    for line in content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 && parts[0] == SPM_GROUP {
            return parts[2].parse().ok();
        }
    }
    None
}

fn parse_group_members(content: &str, group_name: &str) -> Vec<String> {
    for line in content.lines() {
        if line.starts_with(&format!("{group_name}:")) {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 4 {
                let members = parts[3];
                if members.is_empty() {
                    return Vec::new();
                }
                return members.split(',').map(|m| m.trim().to_string()).collect();
            }
            return Vec::new();
        }
    }
    Vec::new()
}

fn read_group_members() -> SpmResult<Vec<String>> {
    let content = fs::read_to_string(GROUP_FILE)
        .map_err(|e| SpmError::other(format!("Cannot read {GROUP_FILE}: {e}")))?;
    Ok(parse_group_members(&content, SPM_GROUP))
}

fn atomic_write_file(path: &str, content: &str) -> SpmResult<()> {
    let tmp = format!("{}.spm-tmp.{}", path, std::process::id());
    fs::write(&tmp, content.as_bytes())
        .map_err(|e| SpmError::other(format!("Cannot write {path}: {e}")))?;
    // Preserve permissions from original file if it exists
    if let Ok(meta) = fs::metadata(path) {
        fs::set_permissions(&tmp, meta.permissions()).ok();
    }
    fs::rename(&tmp, path)
        .map_err(|e| SpmError::other(format!("Cannot rename {path}: {e}")))
}

fn replace_group_entry(content: &str, group_name: &str, gid: u32, members: &[String]) -> String {
    let new_entry = if members.is_empty() {
        format!("{group_name}:x:{gid}:\n")
    } else {
        format!("{group_name}:x:{gid}:{mems}\n", mems = members.join(","))
    };
    let mut new_content = String::new();
    let mut replaced = false;
    for line in content.lines() {
        if line.starts_with(&format!("{group_name}:")) {
            new_content.push_str(&new_entry);
            replaced = true;
        } else {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }
    if !replaced {
        new_content.push_str(&new_entry);
    }
    if content.ends_with('\n') && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    new_content
}

fn write_group_entry(gid: u32, members: &[String]) -> SpmResult<()> {
    let content = fs::read_to_string(GROUP_FILE)
        .map_err(|e| SpmError::other(format!("Cannot read {GROUP_FILE}: {e}")))?;
    let new_content = replace_group_entry(&content, SPM_GROUP, gid, members);
    atomic_write_file(GROUP_FILE, &new_content)
}

fn user_exists_in_content(content: &str, username: &str) -> bool {
    content.lines().any(|l| l.starts_with(&format!("{username}:")))
}

fn user_exists(username: &str) -> bool {
    fs::read_to_string("/etc/passwd")
        .map(|c| user_exists_in_content(&c, username))
        .unwrap_or(false)
}

pub fn handle_group_create() -> SpmResult<()> {
    if group_exists() {
        output::step_info("Group 'spm' already exists");
        return Ok(());
    }

    // Find next available GID >= 1000 (or use 999 for system group)
    let gid = find_free_gid(999)?;
    let entry = format!("{}:x:{}:\n", SPM_GROUP, gid);

    let content = fs::read_to_string(GROUP_FILE)
        .map_err(|e| SpmError::other(format!("Cannot read {GROUP_FILE}: {e}")))?;

    let new_content = format!("{}{}", content, entry);
    atomic_write_file(GROUP_FILE, &new_content)?;

    // Also update gshadow if it exists
    let gshadow = Path::new(GSHADOW_FILE);
    if gshadow.exists() {
        let gshadow_entry = format!("{}:!::\n", SPM_GROUP);
        let gs_content = fs::read_to_string(gshadow)
            .map_err(|e| SpmError::other(format!("Cannot read {GSHADOW_FILE}: {e}")))?;
        let new_gs = format!("{}{}", gs_content, gshadow_entry);
        atomic_write_file(GSHADOW_FILE, &new_gs)?;
    }

    output::result_message(format!("Created group 'spm' (GID {gid})"));
    Ok(())
}

fn find_free_gid_from_content(content: &str, start: u32) -> Option<u32> {
    let mut used = std::collections::HashSet::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 {
            if let Ok(gid) = parts[2].parse::<u32>() {
                used.insert(gid);
            }
        }
    }
    (start..=65535).find(|&gid| !used.contains(&gid))
}

fn find_free_gid(start: u32) -> SpmResult<u32> {
    let content = fs::read_to_string(GROUP_FILE)
        .map_err(|e| SpmError::other(format!("Cannot read {GROUP_FILE}: {e}")))?;
    find_free_gid_from_content(&content, start)
        .ok_or_else(|| SpmError::other("No free GID found"))
}

pub fn handle_group_add_user(username: &str) -> SpmResult<()> {
    if !user_exists(username) {
        return Err(SpmError::other(format!("User '{username}' not found in /etc/passwd")));
    }

    if !group_exists() {
        handle_group_create()?;
    }

    let mut members = read_group_members()?;
    if members.iter().any(|m| m == username) {
        output::step_info(format!("User '{username}' is already in group 'spm'"));
        return Ok(());
    }

    members.push(username.to_string());
    let gid = group_gid().unwrap_or(999);
    write_group_entry(gid, &members)?;

    output::result_message(format!("Added user '{username}' to group 'spm'"));
    output::step_info("Note: The user must log out and back in for group changes to take effect");
    Ok(())
}

pub fn handle_group_remove_user(username: &str) -> SpmResult<()> {
    if !group_exists() {
        return Err(SpmError::other("Group 'spm' does not exist"));
    }

    let mut members = read_group_members()?;
    let initial_len = members.len();
    members.retain(|m| m != username);

    if members.len() == initial_len {
        output::step_info(format!("User '{username}' is not in group 'spm'"));
        return Ok(());
    }

    let gid = group_gid().unwrap_or(999);
    write_group_entry(gid, &members)?;

    output::result_message(format!("Removed user '{username}' from group 'spm'"));
    Ok(())
}

pub fn handle_group_list() -> SpmResult<()> {
    if !group_exists() {
        output::step_info("Group 'spm' does not exist. Create it with 'spm group create'");
        return Ok(());
    }

    let gid = group_gid();
    let members = read_group_members()?;

    println!("  {} Group: {} ({})",
        output::green("✔"),
        output::bold("spm"),
        output::dim(format!("GID {}", gid.unwrap_or(0))));
    if members.is_empty() {
        output::step_info("No members in group 'spm'");
    } else {
        println!("  Members:");
        for m in &members {
            println!("    - {}", m);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- group_exists_in_content --

    #[test]
    fn test_group_exists_in_content_found() {
        let content = "root:x:0:\ndaemon:x:1:\nspm:x:999:\n";
        assert!(group_exists_in_content(content, "spm"));
    }

    #[test]
    fn test_group_exists_in_content_not_found() {
        let content = "root:x:0:\ndaemon:x:1:\n";
        assert!(!group_exists_in_content(content, "spm"));
    }

    #[test]
    fn test_group_exists_in_content_empty() {
        assert!(!group_exists_in_content("", "spm"));
    }

    #[test]
    fn test_group_exists_in_content_partial_match() {
        // "spm-user" should not match "spm"
        let content = "spm-user:x:100:\n";
        assert!(!group_exists_in_content(content, "spm"));
    }

    // -- user_exists_in_content --

    #[test]
    fn test_user_exists_in_content_found() {
        let content = "root:x:0:0:root:/root:/bin/bash\nilya:x:1000:1000:Ilya:/home/ilya:/bin/bash\n";
        assert!(user_exists_in_content(content, "ilya"));
    }

    #[test]
    fn test_user_exists_in_content_not_found() {
        let content = "root:x:0:0:root:/root:/bin/bash\n";
        assert!(!user_exists_in_content(content, "nobody"));
    }

    #[test]
    fn test_user_exists_in_content_empty() {
        assert!(!user_exists_in_content("", "root"));
    }

    // -- parse_group_members --

    #[test]
    fn test_parse_group_members_multiple() {
        let content = "root:x:0:\nspm:x:999:alice,bob,charlie\n";
        let members = parse_group_members(content, "spm");
        assert_eq!(members, vec!["alice", "bob", "charlie"]);
    }

    #[test]
    fn test_parse_group_members_single() {
        let content = "spm:x:999:alice\n";
        let members = parse_group_members(content, "spm");
        assert_eq!(members, vec!["alice"]);
    }

    #[test]
    fn test_parse_group_members_empty() {
        let content = "spm:x:999:\n";
        let members = parse_group_members(content, "spm");
        assert!(members.is_empty());
    }

    #[test]
    fn test_parse_group_members_not_found() {
        let content = "root:x:0:\n";
        let members = parse_group_members(content, "spm");
        assert!(members.is_empty());
    }

    #[test]
    fn test_parse_group_members_no_field() {
        let content = "spm:x:999\n";
        let members = parse_group_members(content, "spm");
        assert!(members.is_empty());
    }

    // -- replace_group_entry --

    #[test]
    fn test_replace_group_entry_replaces_existing() {
        let content = "root:x:0:\nspm:x:999:\ndaemon:x:1:\n";
        let result = replace_group_entry(content, "spm", 1001, &["alice".into(), "bob".into()]);
        assert_eq!(result, "root:x:0:\nspm:x:1001:alice,bob\ndaemon:x:1:\n");
    }

    #[test]
    fn test_replace_group_entry_appends_new() {
        let content = "root:x:0:\n";
        let result = replace_group_entry(content, "spm", 999, &[] as &[String]);
        assert_eq!(result, "root:x:0:\nspm:x:999:\n");
    }

    #[test]
    fn test_replace_group_entry_empty_content() {
        let result = replace_group_entry("", "spm", 999, &[] as &[String]);
        assert_eq!(result, "spm:x:999:\n");
    }

    #[test]
    fn test_replace_group_entry_multiple_entries() {
        let content = "spm:x:999:a\nspm:x:1000:b\n";
        let result = replace_group_entry(content, "spm", 1001, &["c".into()]);
        // Both matching lines are replaced
        assert_eq!(result, "spm:x:1001:c\nspm:x:1001:c\n");
    }

    #[test]
    fn test_replace_group_entry_preserves_trailing_newline() {
        let content = "root:x:0:\nspm:x:999:\n";
        let result = replace_group_entry(content, "spm", 1000, &[] as &[String]);
        assert_eq!(result, "root:x:0:\nspm:x:1000:\n");
        assert!(result.ends_with('\n'));
    }

    // -- find_free_gid_from_content --

    #[test]
    fn test_find_free_gid_from_content_first_available() {
        let content = "root:x:0:\ndaemon:x:1:\n";
        assert_eq!(find_free_gid_from_content(content, 0), Some(2));
    }

    #[test]
    fn test_find_free_gid_from_content_uses_available() {
        let content = "root:x:0:\nuser:x:1000:\nother:x:1001:\n";
        assert_eq!(find_free_gid_from_content(content, 999), Some(999));
    }

    #[test]
    fn test_find_free_gid_from_content_skips_used() {
        let content = "g1:x:999:\ng2:x:1000:\ng3:x:1001:\n";
        assert_eq!(find_free_gid_from_content(content, 999), Some(1002));
    }

    #[test]
    fn test_find_free_gid_from_content_high_number() {
        let content = "g:x:65534:\n";
        assert_eq!(find_free_gid_from_content(content, 65530), Some(65530));
    }

    #[test]
    fn test_find_free_gid_from_content_empty() {
        assert_eq!(find_free_gid_from_content("", 0), Some(0));
    }

    #[test]
    fn test_find_free_gid_from_content_invalid_lines_skipped() {
        let content = "invalid:no:gid:here\nbadline\nroot:x:0:\n";
        assert_eq!(find_free_gid_from_content(content, 0), Some(1));
    }
}

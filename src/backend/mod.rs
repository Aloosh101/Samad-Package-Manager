/// SPM is 100% pure Rust — no external backends required.
pub fn check_missing() -> Vec<&'static str> {
    Vec::new()
}

/// No-op. All system operations are performed via pure Rust libraries.
pub fn show_warnings() {}

/// Parse a dependency name + optional version constraint from deb822/RPM format.
pub fn parse_dep_entry(raw: &str) -> (String, String) {
    let raw = raw.trim();
    if let Some((name_part, constraint)) = raw.split_once('(') {
        let name = name_part.trim().to_string();
        let inner = constraint.trim_end_matches(')').trim();
        let (op, ver) = parse_op_ver(inner);
        return (name, format!("{} {}", op, ver));
    }
    for op in &[">=", "<=", ">>", "<<", "=", ">", "<"] {
        if let Some((name_part, ver)) = raw.split_once(op) {
            return (name_part.trim().to_string(), format!("{} {}", op, ver.trim()));
        }
    }
    let clean = raw.trim_matches(|c| c == '<' || c == '>' || c == '=' || c == ' ').to_string();
    (clean, String::new())
}

fn parse_op_ver(input: &str) -> (String, String) {
    let input = input.trim();
    for op in &[">=", "<=", ">>", "<<", "=", ">", "<"] {
        if let Some((_, ver)) = input.split_once(op) {
            return (op.to_string(), ver.trim().to_string());
        }
    }
    (String::new(), input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_missing_no_panic() {
        let _ = check_missing();
    }

    #[test]
    fn test_parse_dep_entry_deb_format() {
        let (name, constraint) = parse_dep_entry("libssl (>= 1.1)");
        assert_eq!(name, "libssl");
        assert_eq!(constraint, ">= 1.1");
    }

    #[test]
    fn test_parse_dep_entry_rpm_format() {
        let (name, constraint) = parse_dep_entry("libssl >= 1.1");
        assert_eq!(name, "libssl");
        assert_eq!(constraint, ">= 1.1");
    }

    #[test]
    fn test_parse_dep_entry_no_constraint() {
        let (name, constraint) = parse_dep_entry("nginx");
        assert_eq!(name, "nginx");
        assert_eq!(constraint, "");
    }

    #[test]
    fn test_parse_dep_entry_equals() {
        let (name, v) = parse_dep_entry("foo (= 1.0)");
        assert_eq!(name, "foo");
        assert_eq!(v, "= 1.0");
    }

    #[test]
    fn test_parse_op_ver_typical() {
        assert_eq!(parse_op_ver(">= 1.2.3"), (">=".to_string(), "1.2.3".to_string()));
        assert_eq!(parse_op_ver("<= 5.0"), ("<=".to_string(), "5.0".to_string()));
        assert_eq!(parse_op_ver("= 2.0"), ("=".to_string(), "2.0".to_string()));
    }

    #[test]
    fn test_parse_op_ver_no_op() {
        assert_eq!(parse_op_ver("1.2.3"), ("".to_string(), "1.2.3".to_string()));
    }
}

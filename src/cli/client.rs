use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use crate::error::{SpmError, SpmResult};
use serde::Serialize;

#[derive(Serialize)]
struct ClientRequest {
    action: String,
    package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
    user_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_home: Option<String>,
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn current_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/root".to_string())
}

pub fn send_request_action(action: &str, package: Option<String>) -> SpmResult<serde_json::Value> {
    let req = ClientRequest {
        action: action.to_string(),
        package,
        params: None,
        user_id: current_uid(),
        user_home: Some(current_home()),
    };
    send_request_raw(&req)
}

pub fn send_command(action: &str, params: serde_json::Value) -> SpmResult<serde_json::Value> {
    let package = params.get("package").and_then(|v| v.as_str()).map(String::from);
    let req = ClientRequest {
        action: action.to_string(),
        package,
        params: Some(params),
        user_id: current_uid(),
        user_home: Some(current_home()),
    };
    send_request_raw(&req)
}

fn send_request_raw(req: &ClientRequest) -> SpmResult<serde_json::Value> {
    let socket_path = crate::daemon::socket_path();

    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| SpmError::other(format!(
            "Cannot connect to spmd at {socket_path}: {e}. Is spmd running?"
        )))?;

    let json = serde_json::to_string(req)
        .map_err(|e| SpmError::other(format!("Serialization error: {e}")))?;

    writeln!(stream, "{json}")
        .map_err(|e| SpmError::other(format!("Cannot send request: {e}")))?;

    let mut reader = BufReader::new(&stream);

    // Read streaming progress lines, then final result
    loop {
        let mut response = String::new();
        reader.read_line(&mut response)
            .map_err(|e| SpmError::other(format!("Cannot read response: {e}")))?;

        if response.trim().is_empty() {
            continue;
        }

        // Try to parse as progress line or result
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(response.trim()) {
            if let Some(typ) = val.get("type").and_then(|v| v.as_str()) {
                if typ == "progress" {
                    if let Some(msg) = val.get("message").and_then(|v| v.as_str()) {
                        if msg.starts_with("✔ ") || msg.starts_with("ℹ ") || msg.starts_with("⚠ ") || msg.starts_with("✖ ") {
                            eprintln!("  {}", msg);
                        } else if msg.starts_with("📥") {
                            eprint!("\r  {}", msg);
                            let _ = std::io::stderr().flush();
                        } else if msg.starts_with("success: ") {
                            eprintln!("  ✔ {}", &msg[9..]);
                        } else if msg.starts_with("info: ") {
                            eprintln!("  ℹ {}", &msg[6..]);
                        } else if msg.starts_with("warn: ") {
                            eprintln!("  ⚠ {}", &msg[6..]);
                        } else {
                            eprintln!("  {}", msg);
                        }
                    }
                    continue;
                }
            }
            if let Some(status) = val.get("status").and_then(|v| v.as_str()) {
                let message = val.get("message").and_then(|v| v.as_str()).unwrap_or("");
                if status == "error" {
                    return Err(SpmError::other(format!("Daemon error: {message}")));
                }
                return Ok(val);
            }
        }

        // Fallback: print unrecognized lines
        eprintln!("{}", response.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_request_serialization() {
        let req = ClientRequest {
            action: "install".to_string(),
            package: Some("nginx".to_string()),
            params: None,
            user_id: 1000,
            user_home: Some("/home/user".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"install\""));
        assert!(json.contains("\"nginx\""));
        assert!(json.contains("1000"));
        assert!(json.contains("\"/home/user\""));
    }

    #[test]
    fn test_client_request_no_package() {
        let req = ClientRequest {
            action: "list".to_string(),
            package: None,
            params: None,
            user_id: 0,
            user_home: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"list\""));
        assert!(json.contains("\"package\":null"));
    }

    #[test]
    fn test_client_request_repo_serialization() {
        let detail = serde_json::json!({
            "action": "add",
            "name": "myrepo",
            "source": "native",
            "url": "https://example.com/repo",
            "mirrors": [],
        });
        let req = ClientRequest {
            action: "repo".to_string(),
            package: Some(detail.to_string()),
            params: None,
            user_id: 0,
            user_home: Some("/root".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"repo\""));
        assert!(json.contains("myrepo"));
        assert!(json.contains("example.com"));
    }

    #[test]
    fn test_client_request_with_params() {
        let req = ClientRequest {
            action: "build".to_string(),
            package: None,
            params: Some(serde_json::json!({"path": "/tmp/pkg", "output": "/tmp/out"})),
            user_id: 1000,
            user_home: Some("/home/user".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"build\""));
        assert!(json.contains("\"/tmp/out\""));
    }
}

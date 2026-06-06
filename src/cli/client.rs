use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use crate::error::{SpmError, SpmResult};
use serde::Serialize;

#[derive(Serialize)]
struct ClientRequest {
    action: String,
    package: Option<String>,
    user_id: u32,
    user_home: Option<String>,
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn current_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/root".to_string())
}

fn send_action_request(action: &str, package: Option<String>) -> SpmResult<String> {
    let req = ClientRequest {
        action: action.to_string(),
        package,
        user_id: current_uid(),
        user_home: Some(current_home()),
    };
    send_request(&req)
}

pub fn send_install_request(package: &str) -> SpmResult<String> {
    send_action_request("install", Some(package.to_string()))
}

pub fn send_remove_by_name_request(package: &str) -> SpmResult<String> {
    send_action_request("remove", Some(package.to_string()))
}

pub fn send_list_request() -> SpmResult<String> {
    send_action_request("list", None)
}

pub fn send_update_request() -> SpmResult<String> {
    send_action_request("update", None)
}

pub fn send_upgrade_request(package: Option<String>) -> SpmResult<String> {
    send_action_request("upgrade", package)
}

pub fn send_purge_request(package: &str) -> SpmResult<String> {
    send_action_request("purge", Some(package.to_string()))
}

pub fn send_cleanup_request() -> SpmResult<String> {
    send_action_request("cleanup", None)
}

pub fn send_repo_request(action: &str, name: &str, source: Option<&str>, url: Option<&str>, mirrors: &[String]) -> SpmResult<String> {
    let detail = serde_json::json!({
        "action": action,
        "name": name,
        "source": source,
        "url": url,
        "mirrors": mirrors,
    });
    let req = ClientRequest {
        action: "repo".to_string(),
        package: Some(detail.to_string()),
        user_id: current_uid(),
        user_home: Some(current_home()),
    };
    send_request(&req)
}

pub fn send_snapshot_request(action: &str, id: Option<&str>) -> SpmResult<String> {
    let req = ClientRequest {
        action: "snapshot".to_string(),
        package: Some(serde_json::json!({
            "action": action,
            "id": id,
        }).to_string()),
        user_id: current_uid(),
        user_home: Some(current_home()),
    };
    send_request(&req)
}

fn send_request(req: &ClientRequest) -> SpmResult<String> {
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
    let mut response = String::new();
    reader.read_line(&mut response)
        .map_err(|e| SpmError::other(format!("Cannot read response: {e}")))?;

    #[derive(serde::Deserialize)]
    struct RpcResponse {
        status: String,
        message: String,
    }

    let resp: RpcResponse = serde_json::from_str(response.trim())
        .map_err(|e| SpmError::other(format!("Invalid response from daemon: {e}")))?;

    if resp.status == "error" {
        return Err(SpmError::other(format!("Daemon error: {}", resp.message)));
    }

    Ok(resp.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_request_serialization() {
        let req = ClientRequest {
            action: "install".to_string(),
            package: Some("nginx".to_string()),
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
            user_id: 0,
            user_home: Some("/root".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"repo\""));
        assert!(json.contains("myrepo"));
        assert!(json.contains("example.com"));
    }

    #[test]
    fn test_client_request_snapshot_serialization() {
        let detail = serde_json::json!({
            "action": "create",
            "id": null,
        });
        let req = ClientRequest {
            action: "snapshot".to_string(),
            package: Some(detail.to_string()),
            user_id: 0,
            user_home: Some("/root".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"snapshot\""), "missing snapshot action: {json}");
        // "create" is inside a nested JSON string (escaped quotes), so check without surrounding quotes
        assert!(json.contains("create"), "missing create in nested json: {json}");
        assert!(json.contains("null"), "missing null id: {json}");
    }
}

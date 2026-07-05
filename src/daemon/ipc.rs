use serde::{Deserialize, Serialize};

use crate::error::SpmResult;

/// Request from CLI to daemon
#[derive(Debug, Serialize, Deserialize)]
pub enum DaemonRequest {
    Install {
        package: String,
        flags: Vec<String>,
    },
    Remove {
        package: String,
    },
    Purge {
        package: String,
    },
    Status,
    Shutdown,
}

/// Response from daemon to CLI
#[derive(Debug, Serialize, Deserialize)]
pub enum DaemonResponse {
    Ok {
        message: String,
    },
    Error {
        code: u32,
        message: String,
    },
    Progress {
        percent: u8,
        stage: String,
    },
    Status {
        running: bool,
        version: String,
    },
}

/// Serialize a request to JSON bytes
pub fn serialize_request(req: &DaemonRequest) -> SpmResult<Vec<u8>> {
    Ok(serde_json::to_vec(req)?)
}

/// Deserialize a request from JSON bytes
pub fn deserialize_request(data: &[u8]) -> SpmResult<DaemonRequest> {
    Ok(serde_json::from_slice(data)?)
}

/// Serialize a response to JSON bytes
pub fn serialize_response(resp: &DaemonResponse) -> SpmResult<Vec<u8>> {
    Ok(serde_json::to_vec(resp)?)
}

/// Deserialize a response from JSON bytes
pub fn deserialize_response(data: &[u8]) -> SpmResult<DaemonResponse> {
    Ok(serde_json::from_slice(data)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_install_request() {
        let req = DaemonRequest::Install {
            package: "nginx".into(),
            flags: vec!["--replace".into()],
        };
        let bytes = serialize_request(&req).unwrap();
        let deserialized = deserialize_request(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonRequest::Install { .. }));
    }

    #[test]
    fn test_serialize_deserialize_remove_request() {
        let req = DaemonRequest::Remove {
            package: "nginx".into(),
        };
        let bytes = serialize_request(&req).unwrap();
        let deserialized = deserialize_request(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonRequest::Remove { .. }));
    }

    #[test]
    fn test_serialize_deserialize_status_request() {
        let req = DaemonRequest::Status;
        let bytes = serialize_request(&req).unwrap();
        let deserialized = deserialize_request(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonRequest::Status));
    }

    #[test]
    fn test_serialize_deserialize_shutdown_request() {
        let req = DaemonRequest::Shutdown;
        let bytes = serialize_request(&req).unwrap();
        let deserialized = deserialize_request(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonRequest::Shutdown));
    }

    #[test]
    fn test_serialize_deserialize_ok_response() {
        let resp = DaemonResponse::Ok {
            message: "done".into(),
        };
        let bytes = serialize_response(&resp).unwrap();
        let deserialized = deserialize_response(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonResponse::Ok { .. }));
    }

    #[test]
    fn test_serialize_deserialize_error_response() {
        let resp = DaemonResponse::Error {
            code: 42,
            message: "something failed".into(),
        };
        let bytes = serialize_response(&resp).unwrap();
        let deserialized = deserialize_response(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonResponse::Error { .. }));
    }

    #[test]
    fn test_serialize_deserialize_progress_response() {
        let resp = DaemonResponse::Progress {
            percent: 50,
            stage: "installing".into(),
        };
        let bytes = serialize_response(&resp).unwrap();
        let deserialized = deserialize_response(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonResponse::Progress { .. }));
    }

    #[test]
    fn test_serialize_deserialize_status_response() {
        let resp = DaemonResponse::Status {
            running: true,
            version: "0.2.0".into(),
        };
        let bytes = serialize_response(&resp).unwrap();
        let deserialized = deserialize_response(&bytes).unwrap();
        assert!(matches!(deserialized, DaemonResponse::Status { .. }));
    }

    #[test]
    fn test_purge_request_roundtrip() {
        let req = DaemonRequest::Purge {
            package: "old-pkg".into(),
        };
        let bytes = serialize_request(&req).unwrap();
        let deserialized = deserialize_request(&bytes).unwrap();
        match deserialized {
            DaemonRequest::Purge { package } => assert_eq!(package, "old-pkg"),
            _ => panic!("Expected Purge variant"),
        }
    }
}

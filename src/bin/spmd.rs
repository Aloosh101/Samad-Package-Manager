use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    if let Err(e) = spm::daemon::run_daemon().await {
        eprintln!("spmd fatal: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    // Verify the daemon public API is accessible from the binary
    #[test]
    fn test_daemon_socket_path() {
        let path = spm::daemon::socket_path();
        assert!(!path.is_empty());
        assert!(path.ends_with(".sock"));
    }
}

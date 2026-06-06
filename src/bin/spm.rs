use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = spm::cli::args::SpmArgs::parse();
    if let Err(e) = args.execute() {
        eprintln!("{}", e.format_user_error());
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use spm::error::SpmError;

    #[test]
    fn test_error_formatting() {
        let e = SpmError::package_already_installed("nginx");
        assert_eq!(e.format_user_error(), "Package 'nginx' is already installed. Use --replace to reinstall.");
    }
}

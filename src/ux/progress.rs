use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

/// Central progress-bar manager built on indicatif.
///
/// Wraps a `MultiProgress` so that several concurrent progress bars
/// (download, extract, install) are stacked cleanly in the terminal.
pub struct SpmProgress {
    multi: MultiProgress,
    style: ProgressStyle,
}

impl SpmProgress {
    pub fn new() -> Self {
        let style = ProgressStyle::default_bar()
            .template("{spinner:.green} {wide_msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .expect("valid download template")
            .progress_chars("█▉▊▋▌▍▎▏  ");

        Self {
            multi: MultiProgress::new(),
            style,
        }
    }

    /// Add a download-progress bar.
    pub fn download(&self, name: &str, total: u64) -> ProgressBar {
        let pb = self.multi.add(ProgressBar::new(total));
        pb.set_style(self.style.clone());
        pb.set_message(format!("Downloading {}", name));
        pb.enable_steady_tick(Duration::from_millis(100));
        pb
    }

    /// Add an extraction-progress bar.
    pub fn extract(&self, name: &str, total: u64) -> ProgressBar {
        let pb = self.multi.add(ProgressBar::new(total));
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.yellow} Extracting {wide_msg} [{bar:40.green/white}] {pos}/{len}",
                )
                .expect("valid extract template")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );
        pb.set_message(name.to_string());
        pb
    }

    /// Add an install-progress bar.
    pub fn install(&self, name: &str, steps: u64) -> ProgressBar {
        let pb = self.multi.add(ProgressBar::new(steps));
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} Installing {wide_msg} [{bar:40.cyan/blue}] {pos}/{len}")
                .expect("valid install template")
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );
        pb.set_message(name.to_string());
        pb
    }
}

impl Default for SpmProgress {
    fn default() -> Self {
        Self::new()
    }
}

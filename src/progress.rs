//! Lightweight progress reporting for parallel batch operations.
//!
//! Uses atomic counters — no channels, no mutexes, negligible overhead.

use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CompileProgress {
    total: usize,
    completed: Arc<AtomicUsize>,
    failed: Arc<AtomicUsize>,
    skipped: Arc<AtomicUsize>,
}

impl CompileProgress {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            completed: Arc::new(AtomicUsize::new(0)),
            failed: Arc::new(AtomicUsize::new(0)),
            skipped: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn spawn_printer(&self, label: &str) {
        let completed = Arc::clone(&self.completed);
        let failed = Arc::clone(&self.failed);
        let skipped = Arc::clone(&self.skipped);
        let total = self.total;
        let label = label.to_string();

        thread::spawn(move || {
            loop {
                let done = completed.load(Ordering::Relaxed);
                let errs = failed.load(Ordering::Relaxed);
                let skip = skipped.load(Ordering::Relaxed);

                if done + skip >= total {
                    break;
                }

                let mut msg = format!("\r  {label} {}/{} files", done + skip, total);
                if errs > 0 {
                    let _ = write!(msg, " ({errs} failed)");
                }
                eprint!("{msg: <80}");

                thread::sleep(Duration::from_millis(100));
            }
            eprintln!();
        });
    }

    pub fn inc_completed(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_failed(&self) {
        self.failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_skipped(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }
}

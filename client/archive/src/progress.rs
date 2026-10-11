//! Progress on stderr: bytes, rate and an eta, once a second. A gui would
//! implement ProgressBars itself and draw whatever it likes; this is what a
//! terminal gets.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rustic_core::{Progress, ProgressBars, ProgressType, RusticProgress};

#[derive(Clone, Copy, Default, Debug)]
pub struct Stderr;

#[derive(Debug)]
struct Bar {
    kind: ProgressType,
    title: Mutex<String>,
    started: Instant,
    last: Mutex<Instant>,
    done: AtomicU64,
    total: AtomicU64,
}

impl Bar {
    fn draw(&self, force: bool) {
        let mut last = self.last.lock().unwrap();
        if !force && last.elapsed().as_millis() < 1000 {
            return;
        }
        *last = Instant::now();
        let done = self.done.load(Ordering::Relaxed);
        let total = self.total.load(Ordering::Relaxed);
        let secs = self.started.elapsed().as_secs_f64().max(0.001);
        let title = self.title.lock().unwrap();
        match self.kind {
            ProgressType::Bytes => {
                let rate = done as f64 / secs;
                let eta = if total > done && rate > 0.0 {
                    format!("{:.0}s left", (total - done) as f64 / rate)
                } else {
                    String::new()
                };
                eprint!(
                    "\r{:<22} {} / {}  {}/s  {}   ",
                    title,
                    human(done),
                    human(total),
                    human(rate as u64),
                    eta
                );
            }
            ProgressType::Counter => eprint!("\r{:<22} {done} / {total}   ", title),
            ProgressType::Spinner => eprint!("\r{:<22} ...   ", title),
        }
    }
}

impl RusticProgress for Bar {
    fn is_hidden(&self) -> bool {
        false
    }
    fn set_length(&self, len: u64) {
        self.total.store(len, Ordering::Relaxed);
    }
    fn set_title(&self, title: &str) {
        *self.title.lock().unwrap() = title.to_string();
    }
    fn inc(&self, inc: u64) {
        self.done.fetch_add(inc, Ordering::Relaxed);
        self.draw(false);
    }
    fn finish(&self) {
        self.draw(true);
        eprintln!();
    }
}

impl ProgressBars for Stderr {
    fn progress(&self, kind: ProgressType, prefix: &str) -> Progress {
        Progress::new(Bar {
            kind,
            title: Mutex::new(prefix.to_string()),
            started: Instant::now(),
            last: Mutex::new(Instant::now()),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
        })
    }
}

fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

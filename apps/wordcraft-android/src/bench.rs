//! Opt-in, on-device frame-time benchmark.
//!
//! There is no automated Android device in CI, so the performance claim has to be measurable on the
//! tablet itself. Set `WORDFCRAFT_BENCH_FRAMES=600` (and optionally `WORDFCRAFT_BENCH_WARMUP=60`)
//! before launching the Activity, or put `frames=600` in `<internal data>/bench.conf`. The shell
//! then repaints continuously for that many frames and writes one summary line to `android.log`:
//!
//! ```text
//! bench: 600 frames (60 warm-up): cpu ms min=2.10 avg=4.32 p50=4.05 p95=6.20 max=19.80; frame ms avg=16.68 p95=17.02 -> 60.0 fps
//! ```
//!
//! `cpu` is eframe's own per-frame CPU usage (app UI + layout + rasterisation + wgpu submission,
//! excluding the vsync wait). `frame` is the wall-clock interval between completed frames and
//! therefore includes the display's vsync pacing; `fps` is derived from it. The benchmark keeps
//! running the normal interface, so the numbers cover what the tablet actually draws.
//!
//! The statistics are plain integer/f64 code and are unit-tested on the host. The benchmark is
//! disabled unless the environment variable or file enables it, so normal use pays only for one
//! small file lookup at startup.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::path::Path;
use std::time::Instant;

/// A file in the app's internal data directory, for devices where environment variables are not
/// practical. Two lines are enough: `frames=600` and (optionally) `warmup=60`.
pub(crate) const FILE_NAME: &str = "bench.conf";
const MAX_CONFIG_BYTES: u64 = 4096;

/// The number of measured frames.
const ENV_FRAMES: &str = "WORDFCRAFT_BENCH_FRAMES";
/// The number of frames to discard before measuring (shaders, font atlas, first layout).
const ENV_WARMUP: &str = "WORDFCRAFT_BENCH_WARMUP";
/// A typo such as `6000000` must not keep the device repainting for hours.
const MAX_FRAMES: u32 = 100_000;
const DEFAULT_WARMUP: u32 = 30;

/// What to measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BenchConfig {
    pub(crate) frames: u32,
    pub(crate) warmup: u32,
}

impl BenchConfig {
    /// Reads the environment. `None` disables the benchmark; bad values are logged and ignored.
    pub(crate) fn from_env() -> Option<Self> {
        let raw = std::env::var(ENV_FRAMES).ok()?;
        let Some(frames) = parse_frames(&raw) else {
            log::warn!("{ENV_FRAMES}={raw:?} is not a frame count in 1..={MAX_FRAMES}; benchmark disabled");
            return None;
        };
        let warmup = std::env::var(ENV_WARMUP).ok().and_then(|value| parse_u32(&value)).unwrap_or(DEFAULT_WARMUP).min(frames / 2);
        Some(Self { frames, warmup })
    }

    /// Loads the benchmark from the environment first, then from `<internal data>/bench.conf`.
    ///
    /// A set-but-invalid environment variable disables the benchmark instead of silently using the
    /// file: the operator asked for something specific, and the warning in [`Self::from_env`] says
    /// what was wrong. Without the environment, a missing/invalid file simply means "not enabled".
    pub(crate) fn load(data_dir: Option<&Path>) -> Option<Self> {
        if std::env::var_os(ENV_FRAMES).is_some() {
            return Self::from_env();
        }
        let dir = data_dir?;
        let path = dir.join(FILE_NAME);
        let Ok(metadata) = std::fs::metadata(&path) else {
            return None;
        };
        if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
            log::warn!("bench: ignoring {} (not a small regular file)", path.display());
            return None;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            log::warn!("bench: could not read {}", path.display());
            return None;
        };
        let config = parse_config(&text);
        match config {
            Some(_) => log::info!("bench: using {}", path.display()),
            None => log::warn!("bench: {} has no valid `frames=` line; benchmark disabled", path.display()),
        }
        config
    }
}

/// Parses the small `key=value` file. Unknown keys and comments are ignored; `frames` is required.
pub(crate) fn parse_config(text: &str) -> Option<BenchConfig> {
    let mut frames = None;
    let mut warmup = DEFAULT_WARMUP;
    for line in text.lines().take(64) {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "frames" => frames = parse_frames(value),
            "warmup" => warmup = parse_u32(value).unwrap_or(DEFAULT_WARMUP),
            _ => {}
        }
    }
    let frames = frames?;
    Some(BenchConfig { frames, warmup: warmup.min(frames / 2) })
}

fn parse_u32(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok()
}

fn parse_frames(value: &str) -> Option<u32> {
    let frames = parse_u32(value)?;
    (1..=MAX_FRAMES).contains(&frames).then_some(frames)
}

/// Summary of one series of measurements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stats {
    pub(crate) count: usize,
    pub(crate) min_ms: f64,
    pub(crate) avg_ms: f64,
    pub(crate) p50_ms: f64,
    pub(crate) p95_ms: f64,
    pub(crate) max_ms: f64,
}

/// Summarises microseconds without panicking on empty input or overflowing the sum.
pub(crate) fn summarize(samples_us: &[u64]) -> Option<Stats> {
    if samples_us.is_empty() {
        return None;
    }
    let mut sorted = samples_us.to_vec();
    sorted.sort_unstable();
    let count = sorted.len();
    let sum = sorted.iter().map(|sample| u128::from(*sample)).sum::<u128>();
    let ms = |micros: u64| micros as f64 / 1000.0;
    let at = |percent: usize| {
        let index = count.saturating_mul(percent) / 100;
        sorted.get(index.min(count - 1)).copied().map(ms)
    };
    Some(Stats {
        count,
        min_ms: ms(*sorted.first()?),
        avg_ms: sum as f64 / count as f64 / 1000.0,
        p50_ms: at(50)?,
        p95_ms: at(95)?,
        max_ms: ms(*sorted.last()?),
    })
}

/// The live benchmark, stored in the shell only when enabled.
pub(crate) struct FrameBench {
    config: BenchConfig,
    remaining: u32,
    cpu_us: Vec<u64>,
    interval_us: Vec<u64>,
    last_frame: Option<Instant>,
}

impl FrameBench {
    pub(crate) fn new(config: BenchConfig) -> Self {
        Self {
            config,
            remaining: config.frames.saturating_add(config.warmup),
            cpu_us: Vec::with_capacity(usize::try_from(config.frames).unwrap_or(0)),
            interval_us: Vec::with_capacity(usize::try_from(config.frames).unwrap_or(0)),
            last_frame: None,
        }
    }

    /// True while the next frame should be requested immediately.
    pub(crate) fn wanted(&self) -> bool {
        self.remaining > 0
    }

    /// Records the end of one frame. `cpu_seconds` is eframe's `IntegrationInfo::cpu_usage`, which
    /// describes the previous frame; `None` on the first frame.
    pub(crate) fn record(&mut self, cpu_seconds: Option<f32>) {
        if self.remaining == 0 {
            return;
        }
        let now = Instant::now();
        if let Some(previous) = self.last_frame {
            self.interval_us.push(duration_us(now.saturating_duration_since(previous)));
        }
        self.last_frame = Some(now);
        if let Some(cpu) = cpu_seconds
            && cpu.is_finite()
            && cpu >= 0.0
        {
            let micros = (f64::from(cpu) * 1_000_000.0).round();
            if micros <= u64::MAX as f64 {
                self.cpu_us.push(micros as u64);
            }
        }
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining == 0 {
            self.finish();
        }
    }

    fn finish(&self) {
        let skip = usize::try_from(self.config.warmup).unwrap_or(0);
        let cpu = summarize(self.cpu_us.get(skip..).unwrap_or_default());
        let frame = summarize(self.interval_us.get(skip..).unwrap_or_default());
        let frames = cpu.map_or(self.config.frames, |stats| u32::try_from(stats.count).unwrap_or(self.config.frames));
        match (cpu, frame) {
            (Some(cpu), Some(frame)) => {
                let fps = if frame.avg_ms > 0.0 { 1000.0 / frame.avg_ms } else { 0.0 };
                log::info!(
                    "bench: {frames} frames ({} warm-up): cpu ms min={:.2} avg={:.2} p50={:.2} p95={:.2} max={:.2}; frame ms min={:.2} avg={:.2} p95={:.2} -> {fps:.1} fps",
                    self.config.warmup,
                    cpu.min_ms,
                    cpu.avg_ms,
                    cpu.p50_ms,
                    cpu.p95_ms,
                    cpu.max_ms,
                    frame.min_ms,
                    frame.avg_ms,
                    frame.p95_ms,
                );
            }
            (Some(cpu), None) => log::info!(
                "bench: {frames} frames ({} warm-up): cpu ms min={:.2} avg={:.2} p50={:.2} p95={:.2} max={:.2}; frame interval unavailable",
                self.config.warmup,
                cpu.min_ms,
                cpu.avg_ms,
                cpu.p50_ms,
                cpu.p95_ms,
                cpu.max_ms,
            ),
            (None, Some(frame)) => {
                let fps = if frame.avg_ms > 0.0 { 1000.0 / frame.avg_ms } else { 0.0 };
                log::info!(
                    "bench: {frames} frames ({} warm-up): frame ms min={:.2} avg={:.2} p95={:.2} -> {fps:.1} fps; cpu usage unavailable",
                    self.config.warmup,
                    frame.min_ms,
                    frame.avg_ms,
                    frame.p95_ms,
                );
            }
            (None, None) => log::warn!("bench: no samples collected"),
        }
        log::info!("bench: finished; the app keeps running normally");
    }
}

fn duration_us(duration: std::time::Duration) -> u64 {
    // `Duration` is non-negative; clamp an absurdly long pause instead of truncating.
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_series_has_no_stats() {
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn stats_use_sorted_samples() {
        let stats =
            summarize(&[3000, 1000, 2000, 4000, 5000]).unwrap_or(Stats { count: 0, min_ms: 0.0, avg_ms: 0.0, p50_ms: 0.0, p95_ms: 0.0, max_ms: 0.0 });
        assert_eq!(stats.count, 5);
        assert_eq!(stats.min_ms, 1.0);
        assert_eq!(stats.avg_ms, 3.0);
        assert_eq!(stats.p50_ms, 3.0);
        assert_eq!(stats.p95_ms, 5.0);
        assert_eq!(stats.max_ms, 5.0);
    }

    #[test]
    fn percentiles_stay_inside_the_series() {
        // Samples are microseconds: 1000 us = 1.0 ms, 2000 us = 2.0 ms. With two samples the
        // nearest-rank p50 is index 1 (the second sample), as documented by `summarize`.
        let stats = summarize(&[1000, 2000]).unwrap_or(Stats { count: 0, min_ms: 0.0, avg_ms: 0.0, p50_ms: 0.0, p95_ms: 0.0, max_ms: 0.0 });
        assert_eq!(stats.p50_ms, 2.0);
        assert_eq!(stats.p95_ms, 2.0);
    }

    #[test]
    fn frame_counts_are_bounded() {
        assert_eq!(parse_frames("1"), Some(1));
        assert_eq!(parse_frames("600"), Some(600));
        assert_eq!(parse_frames(&MAX_FRAMES.to_string()), Some(MAX_FRAMES));
        assert_eq!(parse_frames("0"), None);
        assert_eq!(parse_frames("100001"), None);
        assert_eq!(parse_frames("many"), None);
        assert_eq!(parse_frames("-1"), None);
        assert_eq!(parse_frames(" 42 "), Some(42));
    }

    #[test]
    fn durations_convert_without_overflow() {
        assert_eq!(duration_us(std::time::Duration::from_millis(1)), 1000);
        assert_eq!(duration_us(std::time::Duration::from_micros(7)), 7);
    }

    #[test]
    fn config_file_requires_a_valid_frame_count() {
        assert_eq!(parse_config(""), None);
        assert_eq!(parse_config("# only a comment\n"), None);
        assert_eq!(parse_config("warmup=10\n"), None);
        assert_eq!(parse_config("frames=0\n"), None);
        assert_eq!(parse_config("frames=lots\n"), None);
    }

    #[test]
    fn config_file_parses_frames_and_comments() {
        let config = parse_config("# tablet\nframes = 600\nwarmup = 60\nignored=true\n");
        assert_eq!(config, Some(BenchConfig { frames: 600, warmup: 60 }));
    }

    #[test]
    fn config_file_clamps_the_warmup() {
        let config = parse_config("frames=10\nwarmup=999\n");
        assert_eq!(config, Some(BenchConfig { frames: 10, warmup: 5 }));
    }

    #[test]
    fn a_bad_warmup_uses_the_default() {
        let config = parse_config("frames=100\nwarmup=soon\n");
        assert_eq!(config, Some(BenchConfig { frames: 100, warmup: DEFAULT_WARMUP }));
    }
}

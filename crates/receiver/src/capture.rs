use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::{imageops::FilterType, GrayImage, RgbaImage};
use xcap::{Frame, Monitor, Window};

use crate::decode::{rgba_to_gray, run_decode_loop};

// ─── Screen capture via xcap's streaming video recorder ─────────────────────

#[allow(clippy::too_many_arguments)]
pub fn receive_screen(
    privkey_path: &str,
    output: Option<String>,
    monitor_selector: Option<String>,
    target_w: u32,
    target_h: u32,
    fps: u32,
    every: u32,
) -> Result<()> {
    guard_wayland()?;

    let monitor = pick_monitor(monitor_selector.as_deref())?;
    let name = monitor
        .friendly_name()
        .or_else(|_| monitor.name())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let (mw, mh) = (monitor.width().unwrap_or(0), monitor.height().unwrap_or(0));
    eprintln!(
        "Capturing monitor '{}' ({}x{}) at up to {} fps → decoding at {}x{}",
        name, mw, mh, fps, target_w, target_h
    );

    let (recorder, rx) = monitor
        .video_recorder()
        .context("failed to open xcap video recorder")?;
    recorder
        .start()
        .context("failed to start xcap video recorder")?;

    let mut throttle = FrameThrottle::new(fps);
    let mut frame_num: u32 = 0;
    let result = run_decode_loop(|| loop {
        throttle.wait();

        let latest = match next_latest_frame(&rx) {
            Some(f) => f,
            None => return None,
        };

        frame_num = frame_num.wrapping_add(1);
        if every > 1 && !frame_num.is_multiple_of(every) {
            continue;
        }

        return Some(frame_to_gray(
            &latest.raw,
            latest.width,
            latest.height,
            target_w,
            target_h,
        ));
    });

    let _ = recorder.stop();

    let encrypted = result?;
    crate::finalize_transfer(encrypted, privkey_path, output)
}

/// Block on the channel, then drain any queued frames so we decode the
/// freshest one. Returns `None` if the recorder has disconnected.
///
/// On `sync_channel(0)` backends (macOS, Windows) the drain is a no-op; on
/// Linux the recorder uses an unbounded channel so old frames can pile up.
fn next_latest_frame(rx: &Receiver<Frame>) -> Option<Frame> {
    let mut latest = rx.recv().ok()?;
    loop {
        match rx.try_recv() {
            Ok(f) => latest = f,
            Err(TryRecvError::Empty) => return Some(latest),
            Err(TryRecvError::Disconnected) => return Some(latest),
        }
    }
}

/// Caps how often the decode loop pulls a frame to decode. xcap doesn't take
/// an fps parameter — the OS streams at its native rate — so the throttle
/// lives consumer-side: we sleep until the next deadline and then take the
/// freshest queued frame.
struct FrameThrottle {
    interval: Option<Duration>,
    next: Instant,
}

impl FrameThrottle {
    fn new(fps: u32) -> Self {
        let interval = if fps == 0 {
            None
        } else {
            Some(Duration::from_secs_f64(1.0 / fps as f64))
        };
        Self {
            interval,
            next: Instant::now(),
        }
    }

    fn wait(&mut self) {
        let Some(interval) = self.interval else {
            return;
        };
        let now = Instant::now();
        if now < self.next {
            thread::sleep(self.next - now);
        }
        // Anchor the next deadline to whichever is later of the missed
        // deadline or "now". This caps the decode rate at one tick per
        // `interval` without ever accumulating a backlog of catch-up sleeps:
        // a single slow decode skips the next wait, but doesn't make later
        // waits faster than the configured fps.
        self.next = self.next.max(now) + interval;
    }
}

// ─── Window capture (screenshot loop — xcap recorder doesn't do windows) ────

#[allow(clippy::too_many_arguments)]
pub fn receive_window(
    privkey_path: &str,
    output: Option<String>,
    title: Option<String>,
    id: Option<u32>,
    target_w: u32,
    target_h: u32,
    fps: u32,
    every: u32,
) -> Result<()> {
    guard_wayland()?;

    let window = pick_window(title.as_deref(), id)?;
    let title_str = window.title().unwrap_or_else(|_| "<unknown>".to_string());
    let app = window
        .app_name()
        .unwrap_or_else(|_| "<unknown>".to_string());
    eprintln!(
        "Capturing window '{}' (app: {}) at {} fps → decoding at {}x{}",
        title_str, app, fps, target_w, target_h
    );

    let frame_interval = if fps == 0 {
        Duration::from_millis(200)
    } else {
        Duration::from_secs_f64(1.0 / fps as f64)
    };

    let mut next_capture = std::time::Instant::now();
    let mut frame_num: u32 = 0;

    let result = run_decode_loop(|| loop {
        let now = std::time::Instant::now();
        if now < next_capture {
            thread::sleep(next_capture - now);
        }
        next_capture += frame_interval;

        let image = match window.capture_image() {
            Ok(img) => img,
            Err(e) => {
                eprintln!("\nwindow capture error: {e}");
                return None;
            }
        };

        frame_num = frame_num.wrapping_add(1);
        if every > 1 && !frame_num.is_multiple_of(every) {
            continue;
        }

        return Some(rgba_image_to_gray(&image, target_w, target_h));
    });

    let encrypted = result?;
    crate::finalize_transfer(encrypted, privkey_path, output)
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn frame_to_gray(rgba: &[u8], src_w: u32, src_h: u32, tgt_w: u32, tgt_h: u32) -> GrayImage {
    let gray = rgba_to_gray(rgba, src_w, src_h);
    maybe_resize(gray, tgt_w, tgt_h)
}

fn rgba_image_to_gray(img: &RgbaImage, tgt_w: u32, tgt_h: u32) -> GrayImage {
    let gray = rgba_to_gray(img.as_raw(), img.width(), img.height());
    maybe_resize(gray, tgt_w, tgt_h)
}

fn maybe_resize(gray: GrayImage, tgt_w: u32, tgt_h: u32) -> GrayImage {
    if tgt_w == 0 || tgt_h == 0 || (gray.width() == tgt_w && gray.height() == tgt_h) {
        return gray;
    }
    image::imageops::resize(&gray, tgt_w, tgt_h, FilterType::Triangle)
}

fn pick_monitor(selector: Option<&str>) -> Result<Monitor> {
    let monitors = Monitor::all().context("failed to enumerate monitors")?;
    if monitors.is_empty() {
        anyhow::bail!("no monitors detected");
    }

    match selector {
        None => monitors
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .cloned()
            .or_else(|| monitors.first().cloned())
            .context("no monitors detected"),
        Some(s) => {
            if let Ok(idx) = s.parse::<usize>() {
                monitors.get(idx).cloned().with_context(|| {
                    format!(
                        "monitor index {} out of range (have {})",
                        idx,
                        monitors.len()
                    )
                })
            } else {
                let lower = s.to_lowercase();
                monitors
                    .iter()
                    .find(|m| {
                        m.friendly_name()
                            .map(|n| n.to_lowercase().contains(&lower))
                            .unwrap_or(false)
                            || m.name()
                                .map(|n| n.to_lowercase().contains(&lower))
                                .unwrap_or(false)
                    })
                    .cloned()
                    .with_context(|| format!("no monitor matches '{}'", s))
            }
        }
    }
}

fn pick_window(title: Option<&str>, id: Option<u32>) -> Result<Window> {
    let windows = Window::all().context("failed to enumerate windows")?;
    if windows.is_empty() {
        anyhow::bail!("no windows detected");
    }

    if let Some(wanted_id) = id {
        return windows
            .into_iter()
            .find(|w| w.id().map(|x| x == wanted_id).unwrap_or(false))
            .with_context(|| format!("no window with id {}", wanted_id));
    }

    let needle = title.context("must supply --title or --id to select a window")?;
    let needle_lower = needle.to_lowercase();
    windows
        .into_iter()
        .filter(|w| !w.is_minimized().unwrap_or(false))
        .find(|w| {
            w.title()
                .map(|t| t.to_lowercase().contains(&needle_lower))
                .unwrap_or(false)
        })
        .with_context(|| format!("no visible window with title containing '{}'", needle))
}

fn guard_wayland() -> Result<()> {
    if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
        anyhow::bail!(
            "Wayland is not supported for direct screen/window capture. \
             Use `receiver pipe` with a Wayland-aware grabber \
             (e.g. `wf-recorder`, `grim`)."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maybe_resize_is_noop_when_dimensions_match() {
        let img = GrayImage::from_pixel(10, 5, image::Luma([200]));
        let out = maybe_resize(img.clone(), 10, 5);
        assert_eq!(out.dimensions(), (10, 5));
        assert_eq!(out.get_pixel(3, 2).0[0], 200);
    }

    #[test]
    fn maybe_resize_skips_when_target_zero() {
        let img = GrayImage::from_pixel(20, 20, image::Luma([100]));
        let out = maybe_resize(img.clone(), 0, 0);
        assert_eq!(out.dimensions(), (20, 20));
    }

    #[test]
    fn maybe_resize_downscales_to_target() {
        let img = GrayImage::from_pixel(40, 40, image::Luma([128]));
        let out = maybe_resize(img, 20, 10);
        assert_eq!(out.dimensions(), (20, 10));
    }

    #[test]
    fn frame_to_gray_converts_rgba_and_resizes() {
        // 2×2 RGBA all white → resized to 1×1 should still be near-white.
        let rgba = vec![255u8; 2 * 2 * 4];
        let g = frame_to_gray(&rgba, 2, 2, 1, 1);
        assert_eq!(g.dimensions(), (1, 1));
        assert!(g.get_pixel(0, 0).0[0] > 240);
    }

    // ── Channel draining ───────────────────────────────────────────────────

    use std::sync::mpsc;

    fn mk_frame(tag: u8) -> Frame {
        Frame {
            width: 1,
            height: 1,
            // First byte of `raw` carries an identifying tag so tests can
            // verify *which* frame the drainer returned.
            raw: vec![tag, 0, 0, 255],
        }
    }

    #[test]
    fn next_latest_frame_returns_most_recent_queued() {
        let (tx, rx) = mpsc::channel();
        tx.send(mk_frame(1)).unwrap();
        tx.send(mk_frame(2)).unwrap();
        tx.send(mk_frame(3)).unwrap();

        let got = next_latest_frame(&rx).expect("should yield a frame");
        assert_eq!(got.raw[0], 3, "drainer must discard stale frames");
    }

    #[test]
    fn next_latest_frame_blocks_then_returns_single_frame() {
        let (tx, rx) = mpsc::channel();
        tx.send(mk_frame(7)).unwrap();
        let got = next_latest_frame(&rx).expect("should yield the only frame");
        assert_eq!(got.raw[0], 7);
    }

    #[test]
    fn next_latest_frame_returns_none_when_disconnected() {
        let (tx, rx) = mpsc::channel::<Frame>();
        drop(tx);
        assert!(next_latest_frame(&rx).is_none());
    }

    /// If the sender disconnects mid-burst, the last delivered frame should
    /// still be returned rather than dropped — losing it would skip a
    /// potentially useful QR.
    #[test]
    fn next_latest_frame_keeps_last_when_disconnected_after_send() {
        let (tx, rx) = mpsc::channel();
        tx.send(mk_frame(11)).unwrap();
        tx.send(mk_frame(22)).unwrap();
        drop(tx);
        let got = next_latest_frame(&rx).expect("should still deliver the latest");
        assert_eq!(got.raw[0], 22);
    }

    // ── --fps throttle ─────────────────────────────────────────────────────

    #[test]
    fn frame_throttle_fps_zero_does_not_sleep() {
        let mut t = FrameThrottle::new(0);
        let start = Instant::now();
        for _ in 0..50 {
            t.wait();
        }
        // 50 zero-cost waits should be effectively instantaneous (allow lots
        // of slack for slow CI but still catch a real sleep).
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "fps=0 wait should be a no-op, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn frame_throttle_enforces_minimum_interval_between_ticks() {
        // 50 fps → 20 ms between ticks. Three ticks ≥ 2 * 20 ms (first wait
        // is a no-op because the deadline starts at "now").
        let mut t = FrameThrottle::new(50);
        let start = Instant::now();
        t.wait();
        t.wait();
        t.wait();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(38),
            "expected ≥ 38ms across 3 ticks at 50fps, got {:?}",
            elapsed
        );
        // And not absurdly slow — bounds the test runtime on slow machines.
        assert!(
            elapsed < Duration::from_millis(500),
            "throttle should not block for long, got {:?}",
            elapsed
        );
    }

    /// A single slow decode shouldn't add up to a "backlog" of catch-up
    /// sleeps. The first wait after the stall fires immediately; subsequent
    /// waits resume the normal cadence (no flood-decode at higher than fps).
    #[test]
    fn frame_throttle_does_not_accumulate_backlog_after_stall() {
        let mut t = FrameThrottle::new(100); // 10 ms interval
        t.wait(); // baseline

        // Stall past several deadlines.
        thread::sleep(Duration::from_millis(50));

        // First wait after stall: we're past the deadline, so this returns
        // immediately rather than sleeping for the missed interval.
        let start = Instant::now();
        t.wait();
        assert!(
            start.elapsed() < Duration::from_millis(5),
            "first wait after stall should be ~0ms, got {:?}",
            start.elapsed()
        );

        // Subsequent waits resume the configured cadence — they don't fire
        // back-to-back to "catch up" the missed ticks, which would defeat
        // the purpose of --fps as a rate limit.
        let start = Instant::now();
        t.wait();
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(8) && elapsed < Duration::from_millis(25),
            "post-stall wait should be ~one interval (10ms), got {:?}",
            elapsed
        );
    }
}

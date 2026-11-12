//! A read-ahead cache for disk-streamed (`AudioData::Stream`) sources, so the real-time render
//! path never blocks on disk I/O once it has warmed up. A single background thread decodes a
//! window ahead of the playhead for each streamed source in use; the render thread only ever
//! takes a non-blocking `try_lock` of the cached window and a non-blocking `try_send` to ask for
//! more. On a miss (startup, or right after a seek the background thread hasn't caught up to
//! yet) the caller falls back to the existing direct, blocking read — so this is a pure
//! optimization layered over already-correct behavior, never a new way to get silence or wrong
//! audio.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Seconds of audio the background thread keeps decoded ahead of the last requested position.
const WINDOW_SECONDS: u64 = 4;

/// A decoded window of one streamed source: `start` is its first frame, at the file's own rate
/// (which, by construction, only equals the session rate for anything the cache is asked about).
struct Window {
    start: u64,
    channels: Vec<Vec<f32>>,
}

/// What the render thread asks the background thread to do: make sure `path` is decoded from
/// `from_frame` onward, for `window_seconds` of audio.
struct Request {
    path: PathBuf,
    channels: u16,
    sample_rate: u32,
    from_frame: u64,
}

/// Read-ahead cache. Cheap to hold when unused: the background thread sits blocked on `recv()`
/// until the render path asks it for something.
pub(crate) struct StreamAheadCache {
    windows: Arc<Mutex<HashMap<PathBuf, Window>>>,
    /// `None` only after `Drop` has taken it, to disconnect the channel and wake the worker.
    requests: Option<SyncSender<Request>>,
    worker: Option<JoinHandle<()>>,
}

impl StreamAheadCache {
    pub(crate) fn new() -> Self {
        let windows = Arc::new(Mutex::new(HashMap::new()));
        // A small bound: this is a "what's the playhead near now" mailbox, not a work queue, so
        // a full channel means several stale requests piled up and dropping them is correct.
        let (tx, rx) = sync_channel::<Request>(4);
        let worker_windows = windows.clone();
        let worker = std::thread::spawn(move || prefetch_loop(&worker_windows, rx));
        StreamAheadCache { windows, requests: Some(tx), worker: Some(worker) }
    }

    /// `[from, from + want)` of every channel of `path`, if the background thread already has
    /// the whole window cached. Never blocks on I/O: a `Mutex::try_lock` miss (the worker is
    /// mid-update) counts the same as a cache miss. Also nudges the background thread to cover
    /// `from` onward, so a steadily advancing playhead stays covered without asking twice.
    pub(crate) fn read(&self, path: &Path, channels: u16, sample_rate: u32, from: u64, want: usize) -> Option<Vec<Vec<f32>>> {
        let hit = self.windows.try_lock().ok().and_then(|w| {
            let win = w.get(path)?;
            let rel = usize::try_from(from.checked_sub(win.start)?).ok()?;
            let end = rel.checked_add(want)?;
            win.channels.iter().all(|c| end <= c.len()).then(|| win.channels.iter().map(|c| c[rel..end].to_vec()).collect())
        });
        if let Some(tx) = &self.requests {
            let _ = tx.try_send(Request { path: path.to_path_buf(), channels, sample_rate, from_frame: from });
        }
        hit
    }
}

impl Drop for StreamAheadCache {
    fn drop(&mut self) {
        // Dropping the sender disconnects the channel, which wakes the worker's blocking `recv()`
        // with `Disconnected` so it can exit; then wait for it so no thread outlives the cache.
        self.requests.take();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// The background thread: waits for a position request, collapses any that piled up behind it
/// (only the newest matters), decodes a window there, and publishes it.
fn prefetch_loop(windows: &Arc<Mutex<HashMap<PathBuf, Window>>>, requests: Receiver<Request>) {
    while let Ok(mut latest) = requests.recv() {
        loop {
            match requests.try_recv() {
                Ok(r) => latest = r,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        let want_frames = latest.sample_rate as usize * WINDOW_SECONDS as usize;
        let channels = soundcraft_audio_io::stream_reader::read_frames_from_file(
            &latest.path,
            latest.from_frame,
            want_frames,
            latest.channels as usize,
            latest.sample_rate,
        );
        if let Ok(mut w) = windows.lock() {
            w.insert(latest.path.clone(), Window { start: latest.from_frame, channels });
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::{AudioBuffer, EncodeOptions};

    /// A cold `read` nudges the background thread, which then warms the cache; once warm, a
    /// `read` for the same window returns the exact samples that were on disk.
    #[test]
    fn warm_cache_hit_returns_the_right_samples() {
        let dir = std::env::temp_dir().join(format!("sc-stream-ahead-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let rate = 48_000u32;
        let frames = rate as usize * 2;
        let tone: Vec<f32> = (0..frames).map(|i| (i % 97) as f32 / 97.0 - 0.5).collect();
        let buf = AudioBuffer { sample_rate: rate, channels: vec![tone.clone()] };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
        let path = dir.join("tone.wav");
        std::fs::write(&path, bytes).unwrap();

        let cache = StreamAheadCache::new();
        // First call is a guaranteed miss: nothing's cached yet, but it nudges the worker.
        assert!(cache.read(&path, 1, rate, 0, 256).is_none());

        let want = 256;
        let mut hit = None;
        for _ in 0..200 {
            if let Some(h) = cache.read(&path, 1, rate, 0, want) {
                hit = Some(h);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let hit = hit.expect("background thread should have warmed the cache within 1s");
        assert_eq!(hit.len(), 1);
        // Round-tripped through a WAV encode/decode, so compare with the tolerance that format's
        // quantization allows rather than bit-exact equality.
        for (a, b) in hit[0].iter().zip(tone[0..want].iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

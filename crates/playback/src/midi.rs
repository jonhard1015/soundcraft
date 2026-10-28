//! Hardware handles live off the audio thread; only bounded queues cross into it.
use super::*;
use soundcraft_midi::live::{Event, PortQueue};

#[derive(Clone)]
pub(crate) struct Port {
    pub id: String,
    pub queue: Arc<PortQueue>,
}
#[derive(Clone, Copy, Debug)]
pub enum RecordedMidi {
    Reset,
    Event { track: TrackId, sample: Samples, bytes: [u8; 3] },
    Boundary { sample: Samples },
}
pub(crate) struct CaptureQueue {
    pub events: crossbeam_queue::ArrayQueue<RecordedMidi>,
    pub lost: AtomicBool,
    boundary: AtomicI64,
}
impl Default for CaptureQueue {
    fn default() -> Self {
        Self { events: crossbeam_queue::ArrayQueue::new(65536), lost: AtomicBool::new(false), boundary: AtomicI64::new(i64::MIN) }
    }
}
impl CaptureQueue {
    pub fn push(&self, event: RecordedMidi) {
        if (matches!(event, RecordedMidi::Event { .. }) && self.events.len() >= self.events.capacity().saturating_sub(256))
            || self.events.push(event).is_err()
        {
            self.lost.store(true, Ordering::Release);
            if let RecordedMidi::Boundary { sample } = event {
                self.boundary.store(sample, Ordering::Release);
            }
        }
    }
}
#[derive(Default)]
pub(crate) struct Hardware {
    keys: Vec<(bool, String)>,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    stop: Option<Arc<AtomicBool>>,
    errors: Vec<String>,
}
impl Drop for Hardware {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Release);
        }
    }
}
impl Hardware {
    pub fn configure(&mut self, s: &Session) -> bool {
        let mut keys: Vec<_> = s
            .tracks
            .iter()
            .filter(|t| t.kind.is_midi() && !t.inactive)
            .flat_map(|t| [(true, t.midi_input.as_ref()), (false, t.midi_output.as_ref())])
            .filter_map(|(input, id)| id.filter(|id| id.len() <= 1024).map(|id| (input, id.clone())))
            .take(256)
            .collect();
        keys.sort();
        keys.dedup();
        if keys == self.keys {
            return false;
        }
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Release);
        }
        self.errors.clear();
        self.inputs.clear();
        self.outputs.clear();
        self.keys = keys;
        #[cfg(not(target_arch = "wasm32"))]
        self.open();
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn open(&mut self) {
        use soundcraft_midi::live::{Input, Output, devices};
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for (is_input, id) in &self.keys {
            if *is_input {
                match Input::open(id) {
                    Ok(input) => {
                        self.inputs.push(Port { id: id.clone(), queue: input.queue.clone() });
                        inputs.push((id.clone(), input));
                    }
                    Err(e) => self.errors.push(format!("MIDI input {id}: {e}")),
                }
            } else {
                match Output::open(id) {
                    Ok(output) => {
                        let queue = Arc::new(PortQueue::default());
                        self.outputs.push(Port { id: id.clone(), queue: queue.clone() });
                        outputs.push((output, queue, Vec::<Event>::with_capacity(4096)));
                    }
                    Err(e) => self.errors.push(format!("MIDI output {id}: {e}")),
                }
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        let spawn = std::thread::Builder::new().name("wabty-midi".into()).spawn(move || {
            let mut checked = std::time::Instant::now();
            while !stop.load(Ordering::Acquire) {
                for (_, input) in &inputs {
                    input.queue.check_active_sensing();
                }
                for (output, queue, pending) in &mut outputs {
                    if queue.take_reset() {
                        pending.clear();
                        let _ = output.panic();
                    }
                    while pending.len() < 4096 {
                        let Some(event) = queue.pop() else {
                            break;
                        };
                        pending.push(event);
                    }
                    pending.sort_by_key(|e| e.timestamp_us);
                    let now = queue.now_us();
                    let due = pending.partition_point(|e| e.timestamp_us <= now);
                    for event in pending.drain(..due) {
                        if output.send(event.message).is_err() {
                            queue.request_reset();
                        }
                    }
                }
                if checked.elapsed() >= std::time::Duration::from_secs(1) {
                    if let Ok(ports) = devices() {
                        for (id, input) in &inputs {
                            if !ports.inputs.iter().any(|p| &p.id == id) {
                                input.queue.request_reset();
                            }
                        }
                    }
                    checked = std::time::Instant::now();
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            // Output::drop releases pedal and notes on every channel.
        });
        if let Err(e) = spawn {
            log::warn!("Cannot start MIDI worker: {e}");
        }
    }
}
impl Player {
    pub fn set_midi_punch(&self, range: Option<Range>) {
        let _ = self.tx.send(Cmd::MidiPunch(range));
    }
    pub fn midi_errors(&self) -> Vec<String> {
        self.side().midi.errors.clone()
    }
    pub fn refresh_midi(&self, session: Arc<Session>) {
        self.side().midi.keys.clear();
        self.update_session(session);
    }
    pub fn reset_midi_capture(&self) {
        let _ = self.tx.send(Cmd::MidiResetCapture);
    }

    pub(crate) fn silence_midi_hardware(&self) {
        for port in &self.side().midi.outputs {
            port.queue.request_reset();
        }
    }
    pub fn midi_panic(&self) {
        self.silence_midi_hardware();
        let _ = self.tx.send(Cmd::MidiPanic);
    }
    pub fn drain_midi_recording(&self) -> (Vec<RecordedMidi>, bool) {
        let q = &self.shared.midi_capture;
        let mut events = Vec::new();
        while let Some(event) = q.events.pop() {
            events.push(event);
        }
        let boundary = q.boundary.swap(i64::MIN, Ordering::AcqRel);
        if boundary != i64::MIN {
            events.push(RecordedMidi::Boundary { sample: boundary });
        }
        (events, q.lost.swap(false, Ordering::AcqRel))
    }
}

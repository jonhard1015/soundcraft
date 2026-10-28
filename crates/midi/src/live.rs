//! Validated MIDI 1.0 channel messages and bounded, timestamped hardware queues.
use crossbeam_queue::ArrayQueue;
use serde::Serialize;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A complete channel-voice message. System exclusive is deliberately not buffered here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Message([u8; 3]);
impl Message {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let status = *bytes.first()?;
        if !(0x80..0xf0).contains(&status) {
            return None;
        }
        let len = if matches!(status & 0xf0, 0xc0 | 0xd0) { 2 } else { 3 };
        if bytes.len() != len || bytes.get(1..)?.iter().any(|b| *b > 127) {
            return None;
        }
        Some(Self([status, *bytes.get(1)?, bytes.get(2).copied().unwrap_or(0)]))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0[..if matches!(self.0[0] & 0xf0, 0xc0 | 0xd0) { 2 } else { 3 }]
    }
    pub fn raw(self) -> [u8; 3] {
        self.0
    }
    pub fn channel(self) -> u8 {
        self.0[0] & 15
    }
    pub fn with_channel(self, channel: u8) -> Self {
        Self([(self.0[0] & 0xf0) | channel.min(15), self.0[1], self.0[2]])
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub timestamp_us: u64,
    pub message: Message,
}

/// Overflow is a musical discontinuity: consumers must release notes before continuing.
pub struct PortQueue {
    epoch: web_time::Instant,
    events: ArrayQueue<(u64, Event)>,
    generation: AtomicU64,
    active_sensing: AtomicBool,
    last_received: AtomicU64,
    reset: AtomicBool,
}
impl Default for PortQueue {
    fn default() -> Self {
        Self {
            epoch: web_time::Instant::now(),
            events: ArrayQueue::new(4096),
            generation: AtomicU64::new(0),
            active_sensing: AtomicBool::new(false),
            last_received: AtomicU64::new(0),
            reset: AtomicBool::new(false),
        }
    }
}
impl PortQueue {
    pub fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
    pub fn push(&self, event: Event) {
        if self.events.push((self.generation.load(Ordering::Acquire), event)).is_err() {
            self.request_reset();
        }
    }
    pub fn pop(&self) -> Option<Event> {
        while let Some((generation, event)) = self.events.pop() {
            if generation == self.generation.load(Ordering::Acquire) {
                return Some(event);
            }
        }
        None
    }
    pub fn received(&self, bytes: &[u8]) {
        self.last_received.store(self.now_us(), Ordering::Release);
        if bytes == [0xfe] {
            self.active_sensing.store(true, Ordering::Release);
        }
    }
    pub fn check_active_sensing(&self) {
        if self.active_sensing.load(Ordering::Acquire) && self.now_us().saturating_sub(self.last_received.load(Ordering::Acquire)) > 300_000 {
            self.active_sensing.store(false, Ordering::Release);
            self.request_reset();
        }
    }
    pub fn request_reset(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.reset.store(true, Ordering::Release);
    }
    pub fn take_reset(&self) -> bool {
        if !self.reset.swap(false, Ordering::AcqRel) {
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct Devices {
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
    pub fn devices() -> Result<Devices, String> {
        let input = MidiInput::new("WABTY Studio").map_err(|e| e.to_string())?;
        let output = MidiOutput::new("WABTY Studio").map_err(|e| e.to_string())?;
        Ok(Devices {
            inputs: input.ports().iter().take(256).filter_map(|p| input.port_name(p).ok().map(|name| Device { id: p.id(), name })).collect(),
            outputs: output.ports().iter().take(256).filter_map(|p| output.port_name(p).ok().map(|name| Device { id: p.id(), name })).collect(),
        })
    }
    pub struct Input {
        _connection: MidiInputConnection<Arc<PortQueue>>,
        pub queue: Arc<PortQueue>,
    }
    impl Input {
        pub fn open(id: &str) -> Result<Self, String> {
            let input = MidiInput::new("WABTY Studio input").map_err(|e| e.to_string())?;
            let port = input.find_port_by_id(id).ok_or("MIDI input is unavailable")?;
            let queue = Arc::new(PortQueue::default());
            let mut origin = None;
            let connection = input
                .connect(
                    &port,
                    "WABTY Studio input",
                    move |timestamp_us, bytes, queue| {
                        queue.received(bytes);
                        let (backend, host) = *origin.get_or_insert_with(|| (timestamp_us, queue.now_us()));
                        let timestamp_us = host.saturating_add(timestamp_us.saturating_sub(backend));
                        if let Some(message) = Message::parse(bytes) {
                            queue.push(Event { timestamp_us, message });
                        }
                        if bytes == [0xff] {
                            queue.request_reset();
                        }
                    },
                    queue.clone(),
                )
                .map_err(|e| e.to_string())?;
            Ok(Self { _connection: connection, queue })
        }
    }
    impl Drop for Input {
        fn drop(&mut self) {
            self.queue.request_reset();
        }
    }
    /// Output belongs on a MIDI worker thread, never on the audio callback.
    pub struct Output {
        connection: MidiOutputConnection,
        held: [[bool; 128]; 16],
    }
    impl Output {
        pub fn open(id: &str) -> Result<Self, String> {
            let output = MidiOutput::new("WABTY Studio output").map_err(|e| e.to_string())?;
            let port = output.find_port_by_id(id).ok_or("MIDI output is unavailable")?;
            let connection = output.connect(&port, "WABTY Studio output").map_err(|e| e.to_string())?;
            Ok(Self { connection, held: [[false; 128]; 16] })
        }
        pub fn send(&mut self, message: Message) -> Result<(), String> {
            self.connection.send(message.bytes()).map_err(|e| e.to_string())?;
            let [status, key, velocity] = message.raw();
            if matches!(status & 0xf0, 0x80 | 0x90)
                && let Some(held) = self.held.get_mut(usize::from(message.channel())).and_then(|c| c.get_mut(usize::from(key)))
            {
                *held = status & 0xf0 == 0x90 && velocity > 0;
            }
            Ok(())
        }
        pub fn panic(&mut self) -> Result<(), String> {
            let mut error = None;
            for channel in 0..16 {
                if let Some(notes) = self.held.get_mut(usize::from(channel)) {
                    for (key, held) in notes.iter_mut().enumerate() {
                        if std::mem::take(held)
                            && let Err(e) = self.connection.send(&[0x80 | channel, key as u8, 0])
                        {
                            error = Some(e.to_string());
                        }
                    }
                }
                for controller in [64, 123, 120] {
                    if let Err(e) = self.connection.send(&[0xb0 | channel, controller, 0]) {
                        error = Some(e.to_string());
                    }
                }
            }
            error.map_or(Ok(()), Err)
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            let _ = self.panic();
        }
    }
}
#[cfg(target_arch = "wasm32")]
pub fn devices() -> Result<Devices, String> {
    Err("Native MIDI hardware is unavailable in this browser build".into())
}

/// Sustain fallback for instruments that only implement note-on/off.
pub struct NoteGate {
    pedal: [bool; 16],
    deferred: [[bool; 128]; 16],
}
impl Default for NoteGate {
    fn default() -> Self {
        Self { pedal: [false; 16], deferred: [[false; 128]; 16] }
    }
}
impl NoteGate {
    pub fn clear(&mut self) {
        self.pedal.fill(false);
        self.deferred.iter_mut().for_each(|c| c.fill(false));
    }
    pub fn dispatch(&mut self, message: Message, mut emit: impl FnMut([u8; 3])) {
        let [status, key, value] = message.raw();
        let ch = usize::from(message.channel());
        let Some(pedal) = self.pedal.get_mut(ch) else {
            return;
        };
        let Some(notes) = self.deferred.get_mut(ch) else {
            return;
        };
        match status & 0xf0 {
            0x80 | 0x90 if status & 0xf0 == 0x80 || value == 0 => {
                if *pedal {
                    if let Some(n) = notes.get_mut(usize::from(key)) {
                        *n = true;
                    }
                } else {
                    emit([0x80 | message.channel(), key, value]);
                }
            }
            0xb0 if key == 64 || key == 121 => {
                *pedal = key == 64 && value >= 64;
                if !*pedal {
                    for (pitch, n) in notes.iter_mut().enumerate() {
                        if std::mem::take(n) {
                            emit([0x80 | message.channel(), pitch as u8, 0]);
                        }
                    }
                }
                emit(message.raw());
            }
            0xb0 if matches!(key, 120 | 123) => {
                *pedal = false;
                notes.fill(false);
                emit(message.raw());
            }
            0x90 => {
                if let Some(n) = notes.get_mut(usize::from(key)) {
                    *n = false;
                }
                emit(message.raw());
            }
            _ => emit(message.raw()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_preserves_new_generation_and_sustain_releases_once() {
        let q = PortQueue::default();
        let note = Message::parse(&[0x92, 60, 100]).unwrap();
        q.push(Event { timestamp_us: 1, message: note });
        q.request_reset();
        q.push(Event { timestamp_us: 2, message: note });
        assert!(q.take_reset());
        assert_eq!(q.pop().unwrap().timestamp_us, 2);
        assert!(q.pop().is_none());
        let mut gate = NoteGate::default();
        let mut events = Vec::new();
        for raw in [[0x92, 60, 100], [0xb2, 64, 127], [0x82, 60, 45], [0xb2, 64, 0]] {
            gate.dispatch(Message::parse(&raw).unwrap(), |msg| events.push(msg));
        }
        assert_eq!(events, vec![[0x92, 60, 100], [0xb2, 64, 127], [0x82, 60, 0], [0xb2, 64, 0]]);
    }
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires access to the native CoreMIDI service"]
    fn native_virtual_ports_round_trip_and_output_drop_releases_notes() {
        use midir::os::unix::{VirtualInput, VirtualOutput};
        let name = format!("WABTY MIDI test {}", std::process::id());
        let (tx, rx) = std::sync::mpsc::channel();
        let _sink = midir::MidiInput::new(&name)
            .unwrap()
            .create_virtual(
                &format!("{name} sink"),
                |_, bytes, tx| {
                    let _ = tx.send(bytes.to_vec());
                },
                tx,
            )
            .unwrap();
        let mut source = midir::MidiOutput::new(&name).unwrap().create_virtual(&format!("{name} source")).unwrap();
        let ports = devices().unwrap();
        let input = Input::open(&ports.inputs.iter().find(|p| p.name.contains(&format!("{name} source"))).unwrap().id).unwrap();
        let output_id = &ports.outputs.iter().find(|p| p.name.contains(&format!("{name} sink"))).unwrap().id;
        let mut output = Output::open(output_id).unwrap();
        let note = Message::parse(&[0x93, 60, 100]).unwrap();
        source.send(note.bytes()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let incoming = loop {
            if let Some(event) = input.queue.pop() {
                break event;
            }
            assert!(std::time::Instant::now() < deadline, "no input from CoreMIDI virtual port");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert_eq!(incoming.message, note);
        output.send(note).unwrap();
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), note.bytes());
        drop(output);
        let mut got_off = false;
        for _ in 0..49 {
            let msg = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
            got_off |= msg == [0x83, 60, 0];
        }
        assert!(got_off, "output shutdown must emit the tracked note-off");
    }
    #[test]
    fn validates_lengths_and_data() {
        assert!(Message::parse(&[0x90, 60, 128]).is_none());
        assert!(Message::parse(&[0x90, 60]).is_none());
        assert!(Message::parse(&[0xf0, 1, 2]).is_none());
        assert_eq!(Message::parse(&[0xc3, 12]).map(|m| m.with_channel(5).raw()), Some([0xc5, 12, 0]));
    }
    #[test]
    fn overflow_discards_stale_notes_and_requests_release() {
        let queue = PortQueue::default();
        let Some(message) = Message::parse(&[0x90, 60, 100]) else {
            return;
        };
        for timestamp_us in 0..4097 {
            queue.push(Event { timestamp_us, message });
        }
        assert!(queue.take_reset());
        assert!(queue.pop().is_none());
        assert!(!queue.take_reset());
    }
}

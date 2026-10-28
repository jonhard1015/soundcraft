//! Bounded recording of channel messages on the session's musical clock.
use crate::{CtrlEvent, CtrlKind, Note, Sequence, live::Message};

/// Records physical key releases separately from sustain, preserving editable controller data.
pub struct Capture {
    sequence: Sequence,
    held: [[Option<(i64, u8)>; 128]; 16],
    limit: usize,
    pedals: [bool; 16],
    pub overflowed: bool,
}
impl Capture {
    pub fn new(limit: usize) -> Self {
        Self { sequence: Sequence::default(), held: [[None; 128]; 16], pedals: [false; 16], limit: limit.min(crate::MAX_EVENTS), overflowed: false }
    }
    fn room(&mut self) -> bool {
        let room = self.sequence.notes.len().saturating_add(self.sequence.ctrls.len()) < self.limit;
        self.overflowed |= !room;
        room
    }
    fn release(&mut self, tick: i64, channel: u8, pitch: u8, velocity: u8) {
        let held = self.held.get_mut(usize::from(channel)).and_then(|c| c.get_mut(usize::from(pitch))).and_then(Option::take);
        if let Some((start, attack)) = held
            && self.room()
        {
            self.sequence.notes.push(Note {
                pitch,
                channel,
                start,
                length: tick.saturating_sub(start).max(1),
                velocity: attack,
                release_velocity: velocity,
            });
        }
    }
    pub fn push(&mut self, tick: i64, message: Message) {
        let [status, data, value] = message.raw();
        let channel = message.channel();
        match status & 0xf0 {
            0x80 => self.release(tick, channel, data, value),
            0x90 if value == 0 => self.release(tick, channel, data, 0),
            0x90 => {
                self.release(tick, channel, data, 0);
                if let Some(slot) = self.held.get_mut(usize::from(channel)).and_then(|c| c.get_mut(usize::from(data))) {
                    *slot = Some((tick, value));
                }
            }
            _ => {
                let (kind, value) = match status & 0xf0 {
                    0xa0 => (CtrlKind::PolyPressure(data), i32::from(value)),
                    0xb0 => (CtrlKind::Cc(data), i32::from(value)),
                    0xc0 => (CtrlKind::Program, i32::from(data)),
                    0xd0 => (CtrlKind::ChannelPressure, i32::from(data)),
                    0xe0 => (CtrlKind::PitchBend, (i32::from(value) << 7 | i32::from(data)) - 8192),
                    _ => return,
                };
                if status & 0xf0 == 0xb0
                    && matches!(data, 64 | 121)
                    && let Some(pedal) = self.pedals.get_mut(usize::from(channel))
                {
                    *pedal = data == 64 && value >= 64;
                }
                if self.room() {
                    self.sequence.ctrls.push(CtrlEvent { tick, channel, kind, value });
                }
                if status & 0xf0 == 0xb0 && matches!(data, 120 | 123) {
                    for pitch in 0..128 {
                        self.release(tick, channel, pitch, 0);
                    }
                }
            }
        }
    }
    /// Close all unfinished notes at stop, seek, disarm or disconnect.
    pub fn boundary(&mut self, tick: i64) {
        for channel in 0..16 {
            if self.pedals.get_mut(usize::from(channel)).is_some_and(std::mem::take) && self.room() {
                self.sequence.ctrls.push(CtrlEvent { tick: tick.saturating_sub(1).max(0), channel, kind: CtrlKind::Cc(64), value: 0 });
            }
            for pitch in 0..128 {
                self.release(tick, channel, pitch, 0);
            }
        }
    }
    pub fn finish(mut self, tick: i64) -> Sequence {
        self.boundary(tick);
        self.sequence.sort();
        self.sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn send(c: &mut Capture, t: i64, bytes: &[u8]) {
        if let Some(m) = Message::parse(bytes) {
            c.push(t, m);
        }
    }
    #[test]
    fn sustain_and_channels_survive_recording() {
        let mut c = Capture::new(100);
        send(&mut c, 0, &[0x92, 60, 100]);
        send(&mut c, 5, &[0xb2, 64, 127]);
        send(&mut c, 10, &[0x82, 60, 45]);
        send(&mut c, 20, &[0xb2, 64, 0]);
        let s = c.finish(30);
        assert_eq!(s.notes[0].length, 10);
        assert_eq!(s.notes[0].release_velocity, 45);
        assert_eq!(s.notes[0].channel, 2);
        assert_eq!(s.ctrls.len(), 2);
    }
    #[test]
    fn stop_releases_a_recorded_sustain_pedal() {
        let mut c = Capture::new(100);
        send(&mut c, 0, &[0x90, 60, 100]);
        send(&mut c, 1, &[0xb0, 64, 127]);
        send(&mut c, 5, &[0x80, 60, 0]);
        let s = c.finish(20);
        assert_eq!(s.ctrls.last().unwrap().value, 0);
        assert_eq!(s.ctrls.last().unwrap().tick, 19);
    }
    #[test]
    fn repeated_notes_and_stop_close_notes() {
        let mut c = Capture::new(100);
        send(&mut c, 0, &[0x90, 60, 100]);
        send(&mut c, 10, &[0x90, 60, 80]);
        let s = c.finish(20);
        assert_eq!(s.notes.len(), 2);
        assert!(s.notes.iter().all(|n| n.length == 10));
    }
}

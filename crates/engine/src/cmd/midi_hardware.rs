//! Hardware routing and timestamped MIDI take import.
use super::{CommandSpec, always, bad, tracks_required};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use soundcraft_midi::{capture::Capture, live::Message};
use soundcraft_model::Clip;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "midi.devices", "List MIDI Hardware", [], None, "{}", always, devices),
        cmd!(
            "midi.route",
            "MIDI Hardware Routing",
            [],
            None,
            "{track, input?: device_id|null, output?: device_id|null, channel?: 1..16|null}",
            always,
            route
        ),
        cmd!(
            "midi.record_take",
            "Record MIDI Take",
            [],
            None,
            "{track, start, end, events: [{sample, bytes: [status, data1, data2?]}]}",
            always,
            record
        ),
    ]
}
fn devices(_: &mut Engine, _: &Value) -> Result<Value> {
    soundcraft_midi::live::devices().map(|d| json!(d)).map_err(|e| bad("midi.devices", e))
}
fn route(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "midi.route";
    let ids = tracks_required(e, cmd, p)?;
    for key in ["input", "output"] {
        if let Some(v) = p.get(key)
            && !v.is_null()
            && !v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control))
        {
            return Err(bad(cmd, "device ID must be a nonempty string of at most 1024 bytes, or null"));
        }
    }
    let channel = match p.get("channel") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().filter(|v| (1..=16).contains(v)).ok_or_else(|| bad(cmd, "channel must be 1..16 or null"))? as u8 - 1),
    };
    if ids.iter().any(|id| !e.session().track(*id).is_some_and(|t| t.kind.is_midi())) {
        return Err(bad(cmd, "select MIDI or instrument tracks"));
    }
    for id in ids {
        if let Some(t) = e.session_mut().track_mut(id) {
            if let Some(v) = p.get("input") {
                t.midi_input = v.as_str().map(str::to_owned);
            }
            if let Some(v) = p.get("output") {
                t.midi_output = v.as_str().map(str::to_owned);
            }
            if p.get("channel").is_some() {
                t.midi_channel = channel;
            }
        }
    }
    Ok(json!({"routed": true}))
}
fn record(e: &mut Engine, p: &Value) -> Result<Value> {
    let cmd = "midi.record_take";
    let id = tracks_required(e, cmd, p)?.first().copied().ok_or_else(|| bad(cmd, "track required"))?;
    if !e.session().track(id).is_some_and(|t| t.kind.is_midi()) {
        return Err(bad(cmd, "not a MIDI track"));
    }
    let start = p.get("start").and_then(Value::as_i64).filter(|v| *v >= 0).ok_or_else(|| bad(cmd, "nonnegative start required"))?;
    let end = p.get("end").and_then(Value::as_i64).filter(|v| *v > start).ok_or_else(|| bad(cmd, "end must follow start"))?;
    let events =
        p.get("events").and_then(Value::as_array).filter(|v| v.len() <= 100_000).ok_or_else(|| bad(cmd, "events array required (limit 100000)"))?;
    let s = e.session();
    let base = s.tempo.samples_to_ticks(start, s.sample_rate);
    let mut capture = Capture::new(100_000);
    let mut previous = start;
    for event in events {
        let at = event
            .get("sample")
            .and_then(Value::as_i64)
            .filter(|v| *v >= previous && *v <= end)
            .ok_or_else(|| bad(cmd, "event samples must be ordered and within the take"))?;
        let bytes =
            event.get("bytes").and_then(Value::as_array).filter(|v| (2..=3).contains(&v.len())).ok_or_else(|| bad(cmd, "invalid MIDI bytes"))?;
        let mut raw = [0u8; 3];
        for (dst, src) in raw.iter_mut().zip(bytes) {
            *dst = src.as_u64().filter(|v| *v <= 255).ok_or_else(|| bad(cmd, "invalid MIDI byte"))? as u8;
        }
        let message = Message::parse(&raw[..bytes.len()]).ok_or_else(|| bad(cmd, "invalid channel message"))?;
        capture.push(s.tempo.samples_to_ticks(at, s.sample_rate).saturating_sub(base), message);
        previous = at;
    }
    let sequence = capture.finish(s.tempo.samples_to_ticks(end, s.sample_rate).saturating_sub(base));
    let s = e.session_mut();
    if let Some(track) = s.track_mut(id)
        && track.clips().iter().any(|c| c.start < end && c.end() > start)
    {
        if track.playlists.len() >= 1024 {
            return Err(bad(cmd, "MIDI take playlist limit reached"));
        }
        track.playlists.push(soundcraft_model::Playlist::new(format!("MIDI take {}", track.playlists.len() + 1)));
        track.active_playlist = track.playlists.len() - 1;
    }
    let clip = s.new_clip_id();
    crate::edit::place_clip(s, id, Clip::midi(clip, "MIDI take", start, end.saturating_sub(start), sequence));
    Ok(json!({"clip": clip}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, ClipContent, TrackKind};
    #[test]
    fn routing_is_validated_and_undoable() {
        let mut e = Engine::default();
        let id = e.session_mut().add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        e.execute("midi.route", &json!({"track": id, "input": "test-port", "channel": 3})).unwrap();
        assert_eq!(e.session().track(id).unwrap().midi_channel, Some(2));
        assert!(e.execute("midi.route", &json!({"track": id, "channel": 17})).is_err());
        assert_eq!(e.session().track(id).unwrap().midi_input.as_deref(), Some("test-port"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track(id).unwrap().midi_input.is_none());
    }
    #[test]
    fn timestamped_take_preserves_controllers_and_previous_take() {
        let mut e = Engine::default();
        let id = e.session_mut().add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let p = json!({"track": id, "start": 0, "end": 48000, "events": [
            {"sample": 0, "bytes": [0x92, 60, 100]},
            {"sample": 12000, "bytes": [0xb2, 64, 127]},
            {"sample": 24000, "bytes": [0x82, 60, 45]},
            {"sample": 36000, "bytes": [0xb2, 64, 0]}
        ]});
        e.execute("midi.record_take", &p).unwrap();
        let track = e.session().track(id).unwrap();
        let ClipContent::Midi { sequence } = &track.clips()[0].content else { panic!("MIDI clip required") };
        assert_eq!(sequence.notes[0].channel, 2);
        assert_eq!(sequence.notes[0].release_velocity, 45);
        assert_eq!(sequence.ctrls.len(), 2);
        e.execute("midi.record_take", &p).unwrap();
        assert_eq!(e.session().track(id).unwrap().playlists.len(), 2);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(id).unwrap().playlists.len(), 1);
    }
}

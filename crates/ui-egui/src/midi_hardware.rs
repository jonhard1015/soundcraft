//! MIDI device selection and delivery of completed audio-thread captures to the command layer.
use crate::*;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct State {
    pub open: bool,
    devices: Option<soundcraft_midi::live::Devices>,
    error: Option<String>,
    takes: BTreeMap<TrackId, Vec<(Samples, [u8; 3])>>,
    pub discard_until_boundary: bool,
}
impl SoundApp {
    pub(crate) fn poll_midi_recording(&mut self) {
        let Some(player) = &self.player else {
            return;
        };
        let (records, lost) = player.drain_midi_recording();
        for record in records {
            match record {
                soundcraft_playback::RecordedMidi::Reset => {
                    self.midi_hardware.takes.clear();
                    self.midi_hardware.discard_until_boundary = false;
                }
                soundcraft_playback::RecordedMidi::Event { track, sample, bytes } => {
                    if self.midi_hardware.discard_until_boundary {
                        continue;
                    }
                    let events = self.midi_hardware.takes.entry(track).or_default();
                    if events.len() < 100_000 {
                        events.push((sample, bytes));
                    } else {
                        self.ui.status = "MIDI take reached its event limit; stop recording to save it.".into();
                    }
                }
                soundcraft_playback::RecordedMidi::Boundary { sample } => {
                    let takes = std::mem::take(&mut self.midi_hardware.takes);
                    if self.midi_hardware.discard_until_boundary {
                        continue;
                    }
                    for (track, mut events) in takes {
                        if events.is_empty() || self.engine.session().track(track).is_none() {
                            continue;
                        }
                        events.sort_by_key(|e| e.0);
                        let start = events.first().map_or(0, |e| e.0);
                        let end = sample.max(events.last().map_or(start, |e| e.0)).max(start.saturating_add(1));
                        let events: Vec<_> = events
                            .iter()
                            .map(|(sample, bytes)| {
                                let len = if matches!(bytes[0] & 0xf0, 0xc0 | 0xd0) { 2 } else { 3 };
                                json!({"sample": sample, "bytes": &bytes[..len]})
                            })
                            .collect();
                        match self.engine.execute("midi.record_take", &json!({"track": track, "start": start, "end": end, "events": events})) {
                            Ok(_) => self.ui.status = "Recorded MIDI take".into(),
                            Err(e) => self.ui.status = format!("Cannot place MIDI take: {e}"),
                        }
                    }
                }
            }
        }
        if lost {
            self.ui.status = "MIDI recording queue overflow: some events were lost. Stop and review the take.".into();
        }
    }
}
pub(crate) fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.midi_hardware.open {
        return;
    }
    let mut open = true;
    egui::Window::new("MIDI Hardware").open(&mut open).default_width(480.0).show(ctx, |ui| {
        if app.midi_hardware.devices.is_none() || ui.button("Refresh devices").clicked() {
            if let Some(player) = &app.player {
                player.refresh_midi(app.engine.session_arc());
            }
            match soundcraft_midi::live::devices() {
                Ok(d) => {
                    app.midi_hardware.devices = Some(d);
                    app.midi_hardware.error = None;
                }
                Err(e) => {
                    app.midi_hardware.error = Some(e);
                    app.midi_hardware.devices = Some(Default::default());
                }
            }
        }
        if let Some(error) = &app.midi_hardware.error {
            ui.label(error);
        }
        if let Some(player) = &app.player {
            for error in player.midi_errors() {
                ui.label(error);
            }
        }
        ui.label("Choose a track's ports, then arm its R button to play or record MIDI.");
        let tracks: Vec<_> = app
            .engine
            .session()
            .tracks
            .iter()
            .filter(|t| t.kind.is_midi())
            .map(|t| (t.id, t.name.clone(), t.midi_input.clone(), t.midi_output.clone(), t.midi_channel))
            .collect();
        if tracks.is_empty() {
            ui.label("Add a MIDI or instrument track to configure routing.");
        }
        let Some(devices) = app.midi_hardware.devices.as_ref() else {
            return;
        };
        if devices.inputs.is_empty() && devices.outputs.is_empty() {
            ui.label("No MIDI devices found. Connect a device, then refresh.");
        }
        egui::ScrollArea::vertical().max_height(450.0).show(ui, |ui| {
            for (id, name, input, output, channel) in tracks {
                ui.separator();
                ui.strong(name);
                for (label, key, current, ports) in [("Input", "input", input, &devices.inputs), ("Output", "output", output, &devices.outputs)] {
                    let mut chosen = current.clone();
                    let text = current.as_ref().map_or("None", |id| ports.iter().find(|p| &p.id == id).map_or("Unavailable", |p| p.name.as_str()));
                    ui.horizontal(|ui| {
                        ui.label(label);
                        craft_ui::ComboBox::from_id_salt((id.0, key)).selected_text(text).show_ui(ui, |ui| {
                            ui.selectable_value(&mut chosen, None, "None");
                            for port in ports {
                                ui.selectable_value(&mut chosen, Some(port.id.clone()), &port.name);
                            }
                        });
                    });
                    if chosen != current {
                        let mut params = json!({"track": id});
                        params[key] = json!(chosen);
                        if let Err(e) = app.engine.execute("midi.route", &params) {
                            app.ui.status = e.to_string();
                        }
                    }
                }
                let mut chosen = channel;
                craft_ui::ComboBox::from_id_salt((id.0, "channel"))
                    .selected_text(channel.map_or("All input channels".into(), |c| format!("Input channel {}", c.saturating_add(1))))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut chosen, None, "All input channels");
                        for channel in 0..16 {
                            ui.selectable_value(&mut chosen, Some(channel), format!("Channel {}", channel + 1));
                        }
                    });
                if chosen != channel {
                    let _ = app.engine.execute("midi.route", &json!({"track": id, "channel": chosen.map(|c| c.saturating_add(1))}));
                }
            }
        });
        if ui.button("All Notes Off").clicked() {
            let _ = app.engine.execute("event.all_notes_off", &json!({}));
        }
    });
    app.midi_hardware.open = open;
}

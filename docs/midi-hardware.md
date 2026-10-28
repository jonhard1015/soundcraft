# MIDI hardware in WABTY Studio

Open **Setup › MIDI Hardware**, choose input/output ports for a MIDI or instrument track,
then enable its **R** button. Instrument tracks need an instrument insert to produce audio.
Input routes accept all channels by default or one selected channel. Multiple armed tracks
can receive the same device. Output sends both armed input thru and the track's MIDI clips.
Disable an output route when wiring could feed MIDI back into the same input.

Press Record to capture MIDI; an audio input is required only when an audio track is also
armed. Stop places an undoable MIDI clip. A take overlapping existing clips gets a separate
playlist, retaining previous takes. Loop boundaries close notes and split takes. Select the
previous playlist to recover an earlier take. Refresh devices after reconnecting hardware.

Commands (also available to CLI, control channel and MCP):

- `midi.devices {}` returns native input/output names and opaque IDs.
- `midi.route {"track":"Keys","input":"<id>","output":"<id>","channel":1}` saves routes.
  Omit fields to retain them; use `null` to remove a port or accept all input channels.
- `midi.record_take` accepts ordered sample-timestamped channel messages and commits one undoable take.
- The existing All Notes Off transport command releases live instruments and hardware outputs.

Hardware callbacks validate complete MIDI 1.0 channel messages and write to bounded queues.
The audio thread routes them and maps backend microsecond timestamps to session samples;
monitoring begins at the next render block. MIDI output uses a separate worker, scheduled
at sample offsets within each rendered block. Worker wake-up and driver latency mean this
is not a hard realtime or latency-compensated hardware-output guarantee.

Recording preserves channel, note/release velocities, sustain/CC, pitch bend, program change
and channel/polyphonic pressure. Sustain defers instrument note-offs; recordings retain the
physical key release and pedal events separately. Stop, seek, loop wrap, disarm, route changes,
input disconnect, queue overflow and explicit panic reset sounding notes. Active Sensing
is monitored only after a device sends it. Output shutdown sends explicit tracked note-offs,
pedal release, All Notes Off and All Sound Off on all channels.

Limits: native MIDI only (no Web MIDI), 256 configured ports, 4096 queued messages per port,
65536 recording queue entries and 100000 events per take. Queue loss is reported. SysEx,
MIDI 2.0, clock/MTC sync, MIDI merge, automatic reconnect and hardware latency compensation
are not implemented. Instrument notes and controllers are normalized to channel 1 to match the existing plugin
note interface; external hardware output and recorded data preserve channels. Existing
Setup MIDI studio/filter flags beyond the route's channel filter remain separate settings.

Validation uses simulated hardware queues for armed routing, timestamp capture, instrument
monitoring and stop flush, plus command/sequence tests for controllers, repeated notes, limits,
undo and preservation of takes. A macOS CoreMIDI virtual-port test also passed: native discovery, input, output and explicit note-off on output shutdown. Physical keyboard/synth and non-macOS backend validation
remain outstanding; readiness percentages have not been raised.

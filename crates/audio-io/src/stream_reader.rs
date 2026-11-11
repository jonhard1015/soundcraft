use std::fs::File;
use std::path::PathBuf;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

pub fn read_frames_from_file(
    path: &PathBuf,
    start_frame: u64,
    count: usize,
    num_channels: usize,
    sample_rate: u32,
) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = (0..num_channels).map(|_| vec![0.0f32; count]).collect();

    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return out,
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format: Box<dyn FormatReader> = match symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
    {
        Ok(f) => f,
        Err(_) => return out,
    };

    let track = match format.default_track(TrackType::Audio) {
        Some(t) => t.clone(),
        None => return out,
    };

    // codec_params is Option<CodecParameters> in 0.6 — need as_ref() before .audio()
    let audio_params = match track.codec_params.as_ref().and_then(|p| p.audio()) {
        Some(p) => p.clone(),
        None => return out,
    };

    let mut decoder: Box<dyn AudioDecoder> = match symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
    {
        Ok(d) => d,
        Err(_) => return out,
    };

    let track_id = track.id;

    // Seek using Time — construct via TimeBase or use seconds
    if start_frame > 0 {
        let secs = start_frame as f64 / sample_rate as f64;
        let time = Time::from(secs as u32); // coarse seek; fine enough pre-decode
        let _ = format.seek(SeekMode::Coarse, SeekTo::Time { time, track_id: Some(track_id) });
    }

    let mut written = 0usize;
    let mut interleaved: Vec<f32> = Vec::new();

    'outer: loop {
        // next_packet returns Result<Option<Packet>> in 0.6
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            _ => break,
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let ch = decoded.num_planes();
        if ch == 0 {
            continue;
        }
        let frames = decoded.frames();
        let needed = frames * ch;
        if interleaved.len() < needed {
            interleaved.resize(needed, 0.0);
        }
        decoded.copy_to_slice_interleaved(&mut interleaved[..needed]);
        let nch = ch.min(num_channels);
        for f in 0..frames {
            if written >= count {
                break 'outer;
            }
            for c in 0..nch {
                out[c][written] = interleaved[f * ch + c];
            }
            written += 1;
        }
    }

    out
}

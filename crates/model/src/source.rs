//! Audio sources (files in the session's Audio Files folder) and their decoded data, and the
//! movies video clips reference.

use crate::SourceId;
use soundcraft_audio_io::{AudioBuffer, FileFormat, peaks::Peaks};
use std::path::PathBuf;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Metadata for an audio file used by the session (serialised).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub name: String,
    /// Path relative to the session folder (or absolute for referenced media).
    pub path: String,
    pub channels: u16,
    pub frames: u64,
    pub sample_rate: u32,
    pub format: FileFormat,
    /// Original BWF time stamp (samples), if any.
    #[serde(default)]
    pub time_reference: u64,
    /// True when the file has not been written to disk yet (recorded/rendered this session).
    #[serde(default)]
    pub unsaved: bool,
}

/// A movie used by Video track clips (serialised). The pictures themselves are never stored in
/// the session: the UI opens the movie from `path` and decodes frames on demand.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VideoSource {
    pub id: SourceId,
    pub name: String,
    /// Absolute path of the movie (referenced, never copied into the session folder).
    pub path: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Frames per second (0 = unknown).
    #[serde(default)]
    pub frame_rate: f64,
    /// Picture length in seconds.
    #[serde(default)]
    pub duration: f64,
    /// Codec id (`h264`, `prores`, `mjpeg`, `hevc`, …).
    #[serde(default)]
    pub codec: String,
    /// Why the movie is not opened, when its stored path was refused on opening the session.
    #[serde(skip)]
    pub offline: Option<String>,
}

impl VideoSource {
    /// Display aspect ratio (width / height), 16:9 when unknown.
    pub fn aspect(&self) -> f64 {
        if self.width == 0 || self.height == 0 { 16.0 / 9.0 } else { f64::from(self.width) / f64::from(self.height) }
    }
}

/// How a source's audio is held in memory.
#[derive(Debug)]
pub enum AudioData {
    /// Full file decoded into RAM (short clips, samples).
    Ram(AudioBuffer),
    /// Long file kept on disk; frames are read on demand.
    Stream {
        path: PathBuf,
        channels: u16,
        sample_rate: u32,
        frames: u64,
    },
}

impl AudioData {
    pub fn num_channels(&self) -> usize {
        match self {
            AudioData::Ram(b) => b.channels.len(),
            AudioData::Stream { channels, .. } => *channels as usize,
        }
    }
    pub fn sample_rate(&self) -> u32 {
        match self {
            AudioData::Ram(b) => b.sample_rate,
            AudioData::Stream { sample_rate, .. } => *sample_rate,
        }
    }
    pub fn frame_count(&self) -> u64 {
        match self {
            AudioData::Ram(b) => b.frames() as u64,
            AudioData::Stream { frames, .. } => *frames,
        }
    }
}

/// Decoded audio plus waveform overviews, or a reference to a file for disk streaming.
#[derive(Debug)]
pub struct SourceAudio {
    pub data: AudioData,
    pub peaks: Vec<Peaks>,
}

impl SourceAudio {
    pub fn new(buffer: AudioBuffer) -> Self {
        let peaks = buffer.channels.iter().map(|c| Peaks::build(c)).collect();
        SourceAudio { data: AudioData::Ram(buffer), peaks }
    }
    pub fn new_stream(path: PathBuf, channels: u16, sample_rate: u32, frames: u64, peaks: Vec<Peaks>) -> Self {
        SourceAudio { data: AudioData::Stream { path, channels, sample_rate, frames }, peaks }
    }
    pub fn frames(&self) -> usize {
        self.data.frame_count() as usize
    }
    pub fn channel(&self, ch: usize) -> Option<&[f32]> {
        match &self.data {
            AudioData::Ram(b) => b.channels.get(ch).map(Vec::as_slice),
            AudioData::Stream { .. } => None,
        }
    }
    pub fn num_channels(&self) -> usize {
        self.data.num_channels()
    }
}

/// Shared decoded audio, keyed by source id.
#[derive(Debug, Clone, Default)]
pub struct SourcePool {
    map: BTreeMap<SourceId, Arc<SourceAudio>>,
}

impl SourcePool {
    pub fn insert(&mut self, id: SourceId, audio: Arc<SourceAudio>) {
        self.map.insert(id, audio);
    }
    pub fn get(&self, id: SourceId) -> Option<&Arc<SourceAudio>> {
        self.map.get(&id)
    }
    pub fn remove(&mut self, id: SourceId) -> Option<Arc<SourceAudio>> {
        self.map.remove(&id)
    }
    pub fn contains(&self, id: SourceId) -> bool {
        self.map.contains_key(&id)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn ids(&self) -> impl Iterator<Item = SourceId> + '_ {
        self.map.keys().copied()
    }
}

impl PartialEq for SourcePool {
    fn eq(&self, other: &Self) -> bool {
        self.map.len() == other.map.len() && self.map.iter().zip(other.map.iter()).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
    }
}

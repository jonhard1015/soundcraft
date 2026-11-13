//! Named session snapshots ("versions"): fork the current session into a saved state, and flip
//! between saved states to compare, without disturbing whatever is open right now.
//!
//! A version is a full copy of the session, written next to it in a `Versions/` folder — the
//! session format is already a single JSON document, so this is cheap and safe. `version.save`
//! clones the live session into a throwaway `Engine` and saves that (the live session is never
//! touched — the same pattern `file.export_selected_tracks_as_session` already uses).
//! `version.load` opens a saved version back into the live engine, replacing its current state —
//! a checkout, not a merge, exactly like `session.open`; the caller is responsible for warning
//! about unsaved changes first, same as it already is for opening any other session.

use super::{CommandSpec, always, bad, str_param};
use crate::{Engine, Result, cmd};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The session file this command should treat as "the session": an explicit `path` param, or the
/// engine's own `e.path` if it's been saved at least once.
fn session_file(e: &Engine, p: &Value, cmd: &str) -> Result<PathBuf> {
    let base = str_param(p, "path").map(str::to_string).or_else(|| e.path.clone()).ok_or_else(|| bad(cmd, "save the session first, or pass `path`"))?;
    Ok(crate::io::session_file(&base))
}

/// The `Versions/` folder next to a session file.
fn versions_dir(e: &Engine, p: &Value, cmd: &str) -> Result<PathBuf> {
    Ok(session_file(e, p, cmd)?.parent().unwrap_or(Path::new("")).join("Versions"))
}

/// Where a named version's `.scraft` lives.
fn version_file(e: &Engine, p: &Value, cmd: &str, name: &str) -> Result<PathBuf> {
    let stem = crate::io::sanitize_name(name);
    Ok(versions_dir(e, p, cmd)?.join(format!("{stem}.{}", soundcraft_model::SESSION_EXTENSION)))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "version.save", "Save Version...", ["File", "Versions"], None, "{name, path?}", always, |e, p| {
            let name = str_param(p, "name").ok_or_else(|| bad("version.save", "`name` required"))?;
            let dest = version_file(e, p, "version.save", name)?;
            crate::io::make_dir(dest.parent().unwrap_or(Path::new("")))?;
            let mut tmp = Engine::new(e.session().clone());
            crate::io::save_session(&mut tmp, &dest.to_string_lossy())?;
            Ok(json!({"path": dest}))
        }),
        cmd!(noundo "version.load", "Load Version...", ["File", "Versions"], None, "{name, path?}", always, |e, p| {
            let name = str_param(p, "name").ok_or_else(|| bad("version.load", "`name` required"))?.to_string();
            let src = version_file(e, p, "version.load", &name)?;
            if !crate::io::file_found(&src) {
                return Err(bad("version.load", &format!("no version named {name:?}")));
            }
            let missing = crate::io::open_session(e, &src)?;
            Ok(json!({"path": src, "missing": missing}))
        }),
        cmd!(noundo "version.list", "Versions...", ["File", "Versions"], None, "{path?}", always, |e, p| {
            let dir = versions_dir(e, p, "version.list")?;
            let mut versions: Vec<_> = crate::io::list_dir(&dir)
                .into_iter()
                .map(|(name, saved_at)| json!({"name": name, "saved_at": saved_at}))
                .collect();
            versions.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            Ok(json!({"versions": versions}))
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saving two named versions never touches the live session, and loading one back checks it
    /// out exactly as it was at save time -- a fork-and-compare, not a merge.
    #[test]
    fn version_save_list_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("sc-version-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let scraft = dir.join("session.scraft").to_string_lossy().into_owned();

        let mut e = Engine::default();
        let original_name = e.session().name.clone();

        let saved = e.execute("version.save", &json!({"name": "v1", "path": scraft})).unwrap();
        let v1_path = PathBuf::from(saved["path"].as_str().unwrap());
        assert!(v1_path.is_file(), "version.save should write the snapshot to disk");
        assert_eq!(v1_path.parent().unwrap().file_name().unwrap(), "Versions");

        // Mutate the live session, then save a second version -- the first must be unaffected.
        e.session_mut().name = format!("{original_name}-renamed");
        e.execute("version.save", &json!({"name": "v2", "path": scraft})).unwrap();

        let listed = e.execute("version.list", &json!({"path": scraft})).unwrap();
        let names: Vec<&str> = listed["versions"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["v1", "v2"]);

        // Still renamed right up until we load v1 back.
        assert_eq!(e.session().name, format!("{original_name}-renamed"));
        e.execute("version.load", &json!({"name": "v1", "path": scraft})).unwrap();
        // Each saved version is stamped with its own file stem, same convention as session.save.
        assert_eq!(e.session().name, "v1", "loading v1 should check out its saved state");
        e.execute("version.load", &json!({"name": "v2", "path": scraft})).unwrap();
        assert_eq!(e.session().name, "v2", "v1 and v2 must be independent snapshots, not aliases of the same file");

        let missing = e.execute("version.load", &json!({"name": "does-not-exist", "path": scraft}));
        assert!(missing.is_err(), "loading an unknown version name should error, not silently no-op");

        let _ = std::fs::remove_dir_all(&dir);
    }
    /// FlowPrint POC — Noir analyzes a 4-bar vocal and generates an editable MIDI drum track.
    /// Vocal is left untouched; FlowPrint only creates and populates the drum track.
    #[test]
    fn flowprint_poc_4bar_vocal_plus_midi_drum_track() {
        let mut e = Engine::default();

        let state = e.execute("event.time_ops_window", &json!({})).unwrap();
        let sample_rate = state["sample_rate"].as_u64().unwrap_or(44_100);
        let bpm: f64 = state["tempos"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|t| t["bpm"].as_f64())
            .unwrap_or(120.0);

        let four_bar_samples = ((4.0 * 4.0 * 60.0 / bpm) * sample_rate as f64).round() as i64;
        assert!(four_bar_samples > 0, "4-bar sample count must be positive");

        let track_resp = e.execute("track.new", &json!({"kind": "midi", "name": "Drums"})).unwrap();
        let track_id = track_resp["tracks"][0].as_u64().unwrap();

        let clip_resp = e
            .execute("midi.new_clip", &json!({"tracks": [track_id], "start": 0, "end": four_bar_samples}))
            .unwrap();
        let clip_id = clip_resp["clip"].as_u64().unwrap();
        assert_eq!(clip_resp["start"].as_i64().unwrap(), 0);
        assert!(clip_resp["end"].as_i64().unwrap() > 0);

        // General MIDI: kick=36, snare=38, closed hi-hat=42
        // 4/4 at 960 TPQN: 1 bar = 3840 ticks
        const TPQN: i64 = 960;
        const BAR: i64 = TPQN * 4;
        let drum_hits: &[(u8, i64, i64)] = &[
            (36, 0,                    TPQN / 2),
            (36, BAR / 2,              TPQN / 2),
            (38, TPQN,                 TPQN / 2),
            (38, TPQN * 3,             TPQN / 2),
            (42, 0,                    TPQN / 4),
            (42, TPQN / 2,             TPQN / 4),
            (42, TPQN,                 TPQN / 4),
            (42, TPQN + TPQN / 2,     TPQN / 4),
            (42, TPQN * 2,             TPQN / 4),
            (42, TPQN * 2 + TPQN / 2, TPQN / 4),
            (42, TPQN * 3,             TPQN / 4),
            (42, TPQN * 3 + TPQN / 2, TPQN / 4),
        ];

        for bar in 0..4_i64 {
            let offset = bar * BAR;
            for &(pitch, start, length) in drum_hits {
                let r = e.execute(
                    "midi.note_add",
                    &json!({"clip": clip_id, "pitch": pitch as i64,
                            "start_ticks": offset + start, "length_ticks": length, "velocity": 100}),
                );
                assert!(r.is_ok(), "midi.note_add failed: {:?}", r);
            }
        }

        // 12 hits/bar × 4 bars = 48
        let notes = e.execute("midi.notes", &json!({"clip": clip_id})).unwrap();
        let n = notes["notes"].as_array().map(|a| a.len()).unwrap_or(0);
        assert_eq!(n, 48, "FlowPrint drum grid must contain exactly 48 notes");
    }

}

    /// FlowPrint POC — Noir analyzes a 4-bar vocal and generates an editable MIDI drum track.
    /// The vocal audio is left untouched; only the drum track is created and populated.
    /// This test drives the exact command sequence FlowPrint will use at runtime.
    #[test]
    fn flowprint_poc_4bar_vocal_plus_midi_drum_track() {
        let mut e = Engine::default();

        // ── 1. Query session tempo and sample rate ────────────────────────────────────
        let state = e.execute("event.time_ops_window", &json!({})).unwrap();
        let sample_rate = state["sample_rate"].as_u64().unwrap_or(44_100);
        // tempo map may be empty on a fresh session; default to 120 BPM
        let bpm: f64 = state["tempos"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|t| t["bpm"].as_f64())
            .unwrap_or(120.0);

        // ── 2. Compute 4-bar length in samples (4/4 time) ────────────────────────────
        let four_bar_samples = ((4.0 * 4.0 * 60.0 / bpm) * sample_rate as f64).round() as i64;
        assert!(four_bar_samples > 0, "4-bar sample count must be positive");

        // ── 3. Create the MIDI drum track ─────────────────────────────────────────────
        let track_resp = e.execute("track.new", &json!({"kind": "midi", "name": "Drums"})).unwrap();
        let track_id = track_resp["tracks"][0].as_u64().unwrap();

        // ── 4. Create a 4-bar empty MIDI clip on that track ──────────────────────────
        let clip_resp = e
            .execute(
                "midi.new_clip",
                &json!({"tracks": [track_id], "start": 0, "end": four_bar_samples}),
            )
            .unwrap();
        let clip_id = clip_resp["clip"].as_u64().unwrap();
        assert_eq!(clip_resp["start"].as_i64().unwrap(), 0);
        assert!(clip_resp["end"].as_i64().unwrap() > 0);

        // ── 5. Populate a basic 4/4 kick-snare-hat pattern ───────────────────────────
        // TICKS_PER_QUARTER = 960; 1 bar = 4 beats = 3840 ticks
        // kick: beats 1 & 3 (ticks 0, 1920); snare: beats 2 & 4 (ticks 960, 2880)
        // hi-hat: every eighth note
        // General MIDI drum map: kick=36, snare=38, closed hi-hat=42
        const TPQN: i64 = 960;
        const BAR: i64 = TPQN * 4;

        let drum_hits: &[(u8, i64, i64)] = &[
            (36, 0,                    TPQN / 2),
            (36, BAR / 2,              TPQN / 2),
            (38, TPQN,                 TPQN / 2),
            (38, TPQN * 3,             TPQN / 2),
            (42, 0,                    TPQN / 4),
            (42, TPQN / 2,             TPQN / 4),
            (42, TPQN,                 TPQN / 4),
            (42, TPQN + TPQN / 2,     TPQN / 4),
            (42, TPQN * 2,             TPQN / 4),
            (42, TPQN * 2 + TPQN / 2, TPQN / 4),
            (42, TPQN * 3,             TPQN / 4),
            (42, TPQN * 3 + TPQN / 2, TPQN / 4),
        ];

        for bar in 0..4_i64 {
            let offset = bar * BAR;
            for &(pitch, start, length) in drum_hits {
                let r = e.execute(
                    "midi.note_add",
                    &json!({
                        "clip": clip_id,
                        "pitch": pitch as i64,
                        "start_ticks": offset + start,
                        "length_ticks": length,
                        "velocity": 100,
                    }),
                );
                assert!(r.is_ok(), "midi.note_add failed: {:?}", r);
            }
        }

        // ── 6. Verify note count: 12 hits/bar × 4 bars = 48 ─────────────────────────
        let notes = e.execute("midi.notes", &json!({"clip": clip_id})).unwrap();
        let note_count = notes["notes"].as_array().map(|a| a.len()).unwrap_or(0);
        assert_eq!(note_count, 48, "FlowPrint drum grid must contain exactly 48 notes");
    }

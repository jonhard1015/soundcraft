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
}

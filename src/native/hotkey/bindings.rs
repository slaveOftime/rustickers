//! Validated shortcut syntax and atomic configuration publication.
use anyhow::{Context as _, Result, bail};
use std::{
    path::{Path, PathBuf},
    sync::{LazyLock, RwLock},
};

pub(super) const CTRL: u8 = 1;
pub(super) const ALT: u8 = 2;
pub(super) const SHIFT: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Chord {
    pub(super) modifiers: u8,
    pub(super) key: Option<u16>,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Snapshot {
    pub(super) chords: [Chord; 4],
    pub(super) revision: u64,
}
struct Configuration {
    values: [String; 4],
    snapshot: Snapshot,
    path: Option<PathBuf>,
}
static WRITE_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static CONFIGURATION: LazyLock<RwLock<Configuration>> = LazyLock::new(|| {
    let values = default_bindings();
    let chords = validate(&values).expect("default shortcuts must be valid");
    RwLock::new(Configuration {
        values,
        snapshot: Snapshot {
            chords,
            revision: 0,
        },
        path: None,
    })
});

pub fn default_bindings() -> [String; 4] {
    [
        "Ctrl+Shift+R",
        "Ctrl+Shift+V",
        "Ctrl+Alt",
        "Ctrl+Shift+Space",
    ]
    .map(str::to_string)
}
pub fn bindings() -> [String; 4] {
    CONFIGURATION
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .values
        .clone()
}
pub(super) fn snapshot() -> Snapshot {
    CONFIGURATION
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot
}
pub fn configure(path: &Path) -> Result<()> {
    let _write = WRITE_GATE.lock().unwrap_or_else(|e| e.into_inner());
    // Even a corrupt saved file must remain repairable through Settings.
    CONFIGURATION
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .path = Some(path.to_path_buf());
    let values = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<[String; 4]>(&bytes)
            .context("Could not read saved shortcuts")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => default_bindings(),
        Err(error) => return Err(error).context("Could not read saved shortcuts"),
    };
    let chords = validate(&values)?;
    let mut state = CONFIGURATION.write().unwrap_or_else(|e| e.into_inner());
    state.values = values;
    state.snapshot = Snapshot {
        chords,
        revision: state.snapshot.revision.wrapping_add(1),
    };
    state.path = Some(path.to_path_buf());
    Ok(())
}
pub fn save_bindings(values: [String; 4]) -> Result<()> {
    let chords = validate(&values)?;
    let _write = WRITE_GATE.lock().unwrap_or_else(|e| e.into_inner());
    let path = CONFIGURATION
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .path
        .clone()
        .context("Shortcut storage is unavailable. Restart Rustickers and try again.")?;
    // Never hold the runtime snapshot lock during file I/O: hooks must stay fast.
    persist(&path, &values)?;
    let mut state = CONFIGURATION.write().unwrap_or_else(|e| e.into_inner());
    // Publish only after replacement succeeds; hooks observe one coherent snapshot.
    state.values = values;
    state.snapshot = Snapshot {
        chords,
        revision: state.snapshot.revision.wrapping_add(1),
    };
    Ok(())
}
pub(super) fn validate(values: &[String; 4]) -> Result<[Chord; 4]> {
    let mut chords = [Chord {
        modifiers: 0,
        key: None,
    }; 4];
    for (ix, value) in values.iter().enumerate() {
        chords[ix] = parse(value).with_context(|| format!("Shortcut {}", ix + 1))?;
        if ix != 2 && chords[ix].key.is_none() {
            bail!("Only quick file preview may use modifiers without a key.");
        }
        if chords[..ix].contains(&chords[ix]) {
            bail!("Two actions cannot use the same shortcut.");
        }
    }
    Ok(chords)
}
fn parse(value: &str) -> Result<Chord> {
    let mut chord = Chord {
        modifiers: 0,
        key: None,
    };
    for part in value.split('+') {
        let part = part.trim().to_ascii_uppercase();
        let modifier = match part.as_str() {
            "CTRL" | "CONTROL" => CTRL,
            "ALT" | "OPTION" => ALT,
            "SHIFT" => SHIFT,
            _ => 0,
        };
        if modifier != 0 {
            if chord.modifiers & modifier != 0 {
                bail!("A modifier is repeated.");
            }
            chord.modifiers |= modifier;
        } else {
            let key = if part == "SPACE" {
                32
            } else if part.len() == 1 && part.as_bytes()[0].is_ascii_alphanumeric() {
                u16::from(part.as_bytes()[0])
            } else if let Some(number) = part
                .strip_prefix('F')
                .and_then(|n| n.parse::<u16>().ok())
                .filter(|n| (1..=12).contains(n))
            {
                111 + number
            } else {
                bail!("Use Ctrl, Alt, Shift, A–Z, 0–9, F1–F12, or Space, separated by +.");
            };
            if chord.key.replace(key).is_some() {
                bail!("A shortcut may contain only one non-modifier key.");
            }
        }
    }
    if chord.modifiers == 0 {
        bail!("Every global shortcut needs at least one modifier.");
    }
    Ok(chord)
}
fn persist(path: &Path, values: &[String; 4]) -> Result<()> {
    use std::io::Write;
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = path.with_extension(format!("{}.{}.tmp", std::process::id(), serial));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(values)?)?;
        file.sync_all()?;
        drop(file);
        replace(&temp, path)?;
        anyhow::Ok(())
    })();
    let _ = std::fs::remove_file(&temp);
    result.context("Could not save shortcuts. The previous bindings are still active.")
}
#[cfg(not(target_os = "windows"))]
fn replace(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to)?;
    Ok(())
}
#[cfg(target_os = "windows")]
fn replace(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // REPLACE_EXISTING | WRITE_THROUGH, same-directory replacement.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_custom_chords_validate() {
        assert!(validate(&default_bindings()).is_ok());
        assert!(
            validate(&["Ctrl+Shift+F2", "Alt+9", "Alt+Shift", "Ctrl+Q"].map(str::to_string))
                .is_ok()
        );
    }
    #[test]
    fn invalid_and_duplicate_chords_are_rejected() {
        for invalid in [
            "",
            "R",
            "Ctrl+Escape",
            "Ctrl+Ctrl+Q",
            "Ctrl+Q+R",
            "Ctrl+F13",
            "Super+R",
        ] {
            let mut values = default_bindings();
            values[0] = invalid.into();
            assert!(validate(&values).is_err(), "{invalid}");
        }
        let mut values = default_bindings();
        values[0] = "shift+ctrl+v".into();
        assert!(validate(&values).is_err());
        values = default_bindings();
        values[0] = "Alt+Ctrl".into();
        assert!(validate(&values).is_err());
    }
    #[test]
    fn save_reloads_and_failed_validation_or_persistence_keeps_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hotkeys.json");
        configure(&path).unwrap();
        let first = bindings();
        let mut custom = first.clone();
        custom[0] = "Alt+Shift+F3".into();
        save_bindings(custom.clone()).unwrap();
        assert_eq!(bindings(), custom);
        configure(&path).unwrap();
        assert_eq!(bindings(), custom);
        let revision = snapshot().revision;
        let mut invalid = custom.clone();
        invalid[0] = invalid[1].clone();
        assert!(save_bindings(invalid).is_err());
        assert_eq!(snapshot().revision, revision);
        assert!(configure(&temp.path().join("missing/hotkeys.json")).is_ok());
        let previous = bindings();
        let revision = snapshot().revision;
        assert!(save_bindings(custom).is_err());
        assert_eq!(bindings(), previous);
        assert_eq!(snapshot().revision, revision);
        // Reset this process-local service after the isolated test.
        let mut state = CONFIGURATION.write().unwrap();
        state.path = None;
        state.values = default_bindings();
        state.snapshot.chords = validate(&state.values).unwrap();
    }
    #[test]
    fn persistence_replaces_existing_file_and_leaves_it_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hotkeys.json");
        let first = default_bindings();
        persist(&path, &first).unwrap();
        let mut second = first.clone();
        second[0] = "Alt+Shift+F3".into();
        persist(&path, &second).unwrap();
        assert_eq!(
            serde_json::from_slice::<[String; 4]>(&std::fs::read(&path).unwrap()).unwrap(),
            second
        );
        assert!(persist(&temp.path().join("missing/hotkeys.json"), &first).is_err());
        // A non-replaceable target must survive a failed atomic replacement.
        let directory = temp.path().join("directory");
        std::fs::create_dir(&directory).unwrap();
        assert!(persist(&directory, &first).is_err());
        assert!(directory.is_dir());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
        assert_eq!(
            serde_json::from_slice::<[String; 4]>(&std::fs::read(&path).unwrap()).unwrap(),
            second
        );
    }
}

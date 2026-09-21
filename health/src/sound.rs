use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    Incident,
    Test,
}

pub trait Sound: Send {
    fn play(&self, kind: SoundKind) -> Result<(), String>;
}

pub struct SilentSound;

impl Sound for SilentSound {
    fn play(&self, _kind: SoundKind) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct RecordingSound {
    pub plays: Arc<Mutex<Vec<SoundKind>>>,
}

impl RecordingSound {
    pub fn new() -> Self {
        Self {
            plays: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn recorded(&self) -> Vec<SoundKind> {
        self.plays
            .lock()
            .map(|plays| plays.clone())
            .unwrap_or_default()
    }
}

impl Sound for RecordingSound {
    fn play(&self, kind: SoundKind) -> Result<(), String> {
        self.plays
            .lock()
            .map_err(|_| "sound recorder poisoned".to_string())?
            .push(kind);
        Ok(())
    }
}

#[cfg(windows)]
pub struct WindowsSound {
    wav_path: PathBuf,
}

#[cfg(windows)]
impl WindowsSound {
    pub fn new(wav_path: PathBuf) -> Self {
        Self { wav_path }
    }

    pub fn from_install() -> Self {
        let next_to_exe = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("two-chairs.wav")));
        let wav_path = next_to_exe.unwrap_or_else(|| PathBuf::from("two-chairs.wav"));
        Self { wav_path }
    }
}

#[cfg(windows)]
impl Sound for WindowsSound {
    fn play(&self, _kind: SoundKind) -> Result<(), String> {
        use windows_sys::Win32::Media::Audio::{
            PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT,
        };

        if !self.wav_path.is_file() {
            return Err("two-chairs wav is not installed".to_string());
        }
        let wide = crate::fsutil::wide_null(&self.wav_path);
        let ok = unsafe {
            PlaySoundW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
            )
        };
        if ok == 0 {
            Err("PlaySoundW failed".to_string())
        } else {
            Ok(())
        }
    }
}

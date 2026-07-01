//! Core contracts for Alloy modules.
//!
//! The desktop app keeps GUI, audio backend, and integration dependencies out of
//! this crate so community module authors can depend on a small API surface.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const ABI_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleCategory {
    Core,
    MusicSource,
    AudioProcessor,
    Theme,
    UiElement,
    Integration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub category: ModuleCategory,
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub enabled_by_default: bool,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl ModuleManifest {
    #[must_use]
    pub fn built_in(
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        category: ModuleCategory,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            category,
            description: description.into(),
            authors: vec!["Alloy".to_owned()],
            enabled_by_default: true,
            homepage: None,
            capabilities: Vec::new(),
        }
    }
}

pub trait AlloyModule: Send {
    fn manifest(&self) -> &ModuleManifest;

    fn enabled_by_default(&self) -> bool {
        self.manifest().enabled_by_default
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub path: PathBuf,
    #[serde(default)]
    pub cover_path: Option<PathBuf>,
    pub duration: Option<Duration>,
    #[serde(default)]
    pub source: String,
}

impl Track {
    #[must_use]
    pub fn display_artist(&self) -> &str {
        if self.artist.is_empty() {
            "Unknown Artist"
        } else {
            &self.artist
        }
    }

    #[must_use]
    pub fn display_album(&self) -> &str {
        if self.album.is_empty() {
            "Unknown Album"
        } else {
            &self.album
        }
    }
}

pub trait MusicSourceModule: AlloyModule {
    fn scan(&self, roots: &[PathBuf]) -> Result<Vec<Track>, ModuleError>;
    fn supports_path(&self, path: &Path) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioFrame<'a> {
    pub samples: &'a [f32],
    pub channels: u16,
    pub sample_rate: u32,
}

#[derive(Debug)]
pub struct AudioFrameMut<'a> {
    pub samples: &'a mut [f32],
    pub channels: u16,
    pub sample_rate: u32,
}

pub trait AudioProcessorModule: AlloyModule {
    fn process(&mut self, frame: AudioFrameMut<'_>);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeDefinition {
    pub id: String,
    pub name: String,
    pub mode: ThemeMode,
    pub colors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    Dark,
    Light,
}

pub trait ThemeModule: AlloyModule {
    fn theme(&self) -> &ThemeDefinition;
}

#[derive(Debug, Clone)]
pub enum ModuleEvent {
    PlaybackStarted {
        track: Track,
    },
    PlaybackPaused {
        track: Option<Track>,
    },
    PlaybackResumed {
        track: Option<Track>,
    },
    PlaybackStopped,
    TrackFinished {
        track: Track,
    },
    PositionChanged {
        track: Option<Track>,
        position: Duration,
    },
}

pub trait IntegrationModule: AlloyModule {
    fn on_event(&mut self, event: &ModuleEvent) -> Result<(), ModuleError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleError {
    message: String,
}

impl ModuleError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModuleError {}

impl From<std::io::Error> for ModuleError {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

#[repr(C)]
pub struct NativeAudioBuffer {
    pub samples: *mut f32,
    pub sample_count: usize,
    pub channels: u16,
    pub sample_rate: u32,
}

pub type NativeCreateFn = unsafe extern "C" fn() -> *mut c_void;
pub type NativeDestroyFn = unsafe extern "C" fn(*mut c_void);
pub type NativeProcessFn = unsafe extern "C" fn(*mut c_void, *mut NativeAudioBuffer);

#[repr(C)]
pub struct NativeAudioProcessorV1 {
    pub create: Option<NativeCreateFn>,
    pub destroy: Option<NativeDestroyFn>,
    pub process: Option<NativeProcessFn>,
}

#[repr(C)]
pub struct NativePluginDescriptorV1 {
    pub abi_version: u32,
    pub manifest_json: *const c_char,
    pub audio_processor: NativeAudioProcessorV1,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_manifest_defaults_to_enabled() {
        let manifest = ModuleManifest::built_in(
            "alloy.test",
            "Test",
            "0.1.0",
            ModuleCategory::Integration,
            "test module",
        );

        assert!(manifest.enabled_by_default);
        assert_eq!(manifest.authors, ["Alloy"]);
    }
}

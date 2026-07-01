use crate::audio::{EqBand, EqProfile, ReplayGainProfile};
use alloy_core::Track;
use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const CORE_MODULE_ID: &str = "alloy.core";
pub const LOCAL_SOURCE_MODULE_ID: &str = "alloy.sources.local";
pub const EQ_MODULE_ID: &str = "alloy.audio.eq";
pub const REPLAYGAIN_MODULE_ID: &str = "alloy.audio.replaygain";
pub const LASTFM_MODULE_ID: &str = "alloy.integrations.lastfm";
pub const DISCORD_MODULE_ID: &str = "alloy.integrations.discord";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub modules_dir: PathBuf,
    pub themes_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> anyhow::Result<Self> {
        let project = ProjectDirs::from("dev", "alloy", "Alloy")
            .context("could not determine user config directory")?;
        let config_dir = project.config_dir().to_path_buf();
        let config_file = config_dir.join("alloy.toml");
        let modules_dir = config_dir.join("modules");
        let themes_dir = modules_dir.join("themes");

        std::fs::create_dir_all(&themes_dir).context("failed to create Alloy config folders")?;

        Ok(Self {
            config_dir,
            config_file,
            modules_dir,
            themes_dir,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_theme")]
    pub active_theme: String,
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub library_paths: Vec<PathBuf>,
    #[serde(default = "default_modules")]
    pub modules: BTreeMap<String, ModuleToggle>,
    #[serde(default)]
    pub equalizer: EqualizerConfig,
    #[serde(default)]
    pub replay_gain: ReplayGainConfig,
    #[serde(default)]
    pub playback: PlaybackConfig,
    #[serde(default = "default_audio_chain")]
    pub audio_chain: Vec<AudioChainNodeConfig>,
    #[serde(default)]
    pub cover_art: CoverArtConfig,
    #[serde(default)]
    pub lastfm: LastFmConfig,
    #[serde(default)]
    pub discord: DiscordConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_theme: default_theme(),
            volume: default_volume(),
            library_paths: Vec::new(),
            modules: default_modules(),
            equalizer: EqualizerConfig::default(),
            replay_gain: ReplayGainConfig::default(),
            playback: PlaybackConfig::default(),
            audio_chain: default_audio_chain(),
            cover_art: CoverArtConfig::default(),
            lastfm: LastFmConfig::default(),
            discord: DiscordConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load(paths: &AppPaths) -> anyhow::Result<Self> {
        if !paths.config_file.exists() {
            let config = Self::default();
            config.save(paths)?;
            return Ok(config);
        }

        let raw = std::fs::read_to_string(&paths.config_file)
            .with_context(|| format!("failed to read {}", paths.config_file.display()))?;
        let mut config: Self = toml::from_str(&raw)
            .with_context(|| format!("failed to parse {}", paths.config_file.display()))?;
        config.ensure_defaults();
        Ok(config)
    }

    pub fn save(&self, paths: &AppPaths) -> anyhow::Result<()> {
        std::fs::create_dir_all(&paths.config_dir)
            .with_context(|| format!("failed to create {}", paths.config_dir.display()))?;
        let raw = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(&paths.config_file, raw)
            .with_context(|| format!("failed to write {}", paths.config_file.display()))?;
        Ok(())
    }

    pub fn module_enabled(&self, id: &str) -> bool {
        self.modules.get(id).map_or(false, |toggle| toggle.enabled)
    }

    pub fn set_module_enabled(&mut self, id: &str, enabled: bool) {
        self.modules
            .entry(id.to_owned())
            .or_insert_with(|| ModuleToggle { enabled })
            .enabled = enabled;
    }

    pub fn ensure_module_toggle(&mut self, id: &str, enabled_by_default: bool) -> bool {
        if self.modules.contains_key(id) {
            return false;
        }

        self.modules.insert(
            id.to_owned(),
            ModuleToggle {
                enabled: enabled_by_default,
            },
        );
        true
    }

    pub fn ensure_audio_chain_node(&mut self, id: &str, enabled_by_default: bool) -> bool {
        if self.audio_chain.iter().any(|node| node.id == id) {
            return false;
        }

        self.audio_chain.push(AudioChainNodeConfig {
            id: id.to_owned(),
            enabled: enabled_by_default,
        });
        true
    }

    pub fn eq_profile(&self) -> EqProfile {
        EqProfile {
            enabled: self.module_enabled(EQ_MODULE_ID) && self.equalizer.enabled,
            preamp_db: self.equalizer.preamp_db,
            bands: self
                .equalizer
                .bands
                .iter()
                .map(|band| EqBand {
                    frequency_hz: band.frequency_hz,
                    gain_db: band.gain_db,
                    q: band.q,
                })
                .collect(),
        }
    }

    pub fn replay_gain_profile(&self, track: &Track) -> ReplayGainProfile {
        let replay_gain = &track.replay_gain;
        let (gain_db, peak) = match self.replay_gain.mode {
            ReplayGainMode::Track => (
                replay_gain.track_gain_db.or(replay_gain.album_gain_db),
                replay_gain.track_peak.or(replay_gain.album_peak),
            ),
            ReplayGainMode::Album => (
                replay_gain.album_gain_db.or(replay_gain.track_gain_db),
                replay_gain.album_peak.or(replay_gain.track_peak),
            ),
        };

        ReplayGainProfile {
            enabled: self.module_enabled(REPLAYGAIN_MODULE_ID)
                && self.replay_gain.enabled
                && gain_db.is_some(),
            gain_db: gain_db.unwrap_or(0.0) + self.replay_gain.preamp_db,
            peak,
            prevent_clipping: self.replay_gain.prevent_clipping,
        }
    }

    pub fn ensure_defaults(&mut self) {
        for (id, toggle) in default_modules() {
            self.modules.entry(id).or_insert(toggle);
        }
        if self.equalizer.bands.is_empty() {
            self.equalizer.bands = default_eq_bands();
        }
        if self.audio_chain.is_empty() {
            self.audio_chain = default_audio_chain();
        } else {
            for default_node in default_audio_chain() {
                if !self
                    .audio_chain
                    .iter()
                    .any(|node| node.id == default_node.id)
                {
                    if default_node.id == REPLAYGAIN_MODULE_ID {
                        let insert_index = self
                            .audio_chain
                            .iter()
                            .position(|node| node.id == EQ_MODULE_ID)
                            .unwrap_or(self.audio_chain.len());
                        self.audio_chain.insert(insert_index, default_node);
                    } else {
                        self.audio_chain.push(default_node);
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleToggle {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqualizerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub preamp_db: f32,
    #[serde(default = "default_eq_bands")]
    pub bands: Vec<EqualizerBandConfig>,
}

impl Default for EqualizerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            preamp_db: 0.0,
            bands: default_eq_bands(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqualizerBandConfig {
    pub frequency_hz: f32,
    pub gain_db: f32,
    #[serde(default = "default_q")]
    pub q: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayGainConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: ReplayGainMode,
    #[serde(default)]
    pub preamp_db: f32,
    #[serde(default = "default_true")]
    pub prevent_clipping: bool,
}

impl Default for ReplayGainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: ReplayGainMode::default(),
            preamp_db: 0.0,
            prevent_clipping: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ReplayGainMode {
    #[default]
    Track,
    Album,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackConfig {
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: RepeatMode,
    #[serde(default)]
    pub bit_perfect: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RepeatMode {
    #[default]
    None,
    All,
    One,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioChainNodeConfig {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverArtConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub sidecar_images: bool,
}

impl Default for CoverArtConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sidecar_images: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastFmConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_secret: Option<String>,
    #[serde(default)]
    pub session_key: Option<String>,
    #[serde(default = "default_true")]
    pub scrobble: bool,
}

impl Default for LastFmConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("LASTFM_API_KEY").ok(),
            api_secret: std::env::var("LASTFM_API_SECRET").ok(),
            session_key: std::env::var("LASTFM_SESSION_KEY").ok(),
            scrobble: true,
        }
    }
}

impl LastFmConfig {
    #[must_use]
    pub fn configured(&self) -> bool {
        self.api_key
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            && self
                .api_secret
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            && self
                .session_key
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    #[serde(default)]
    pub client_id: Option<String>,
}

impl DiscordConfig {
    #[must_use]
    pub fn with_env(mut self) -> Self {
        if self.client_id.is_none() {
            self.client_id = std::env::var("DISCORD_CLIENT_ID").ok();
        }
        self
    }
}

pub fn apply_cli_library_paths(config: &mut AppConfig) {
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--library" {
            if let Some(path) = args.next() {
                push_unique_path(&mut config.library_paths, PathBuf::from(path));
            }
            continue;
        }

        let path = PathBuf::from(&arg);
        if path.exists() {
            push_unique_path(&mut config.library_paths, path);
        }
    }
}

pub fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn default_modules() -> BTreeMap<String, ModuleToggle> {
    [
        (CORE_MODULE_ID, true),
        (LOCAL_SOURCE_MODULE_ID, true),
        (REPLAYGAIN_MODULE_ID, true),
        (EQ_MODULE_ID, true),
        (LASTFM_MODULE_ID, false),
        (DISCORD_MODULE_ID, false),
    ]
    .into_iter()
    .map(|(id, enabled)| (id.to_owned(), ModuleToggle { enabled }))
    .collect()
}

fn default_audio_chain() -> Vec<AudioChainNodeConfig> {
    vec![
        AudioChainNodeConfig {
            id: REPLAYGAIN_MODULE_ID.to_owned(),
            enabled: true,
        },
        AudioChainNodeConfig {
            id: EQ_MODULE_ID.to_owned(),
            enabled: true,
        },
    ]
}

fn default_eq_bands() -> Vec<EqualizerBandConfig> {
    [
        31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
    ]
    .into_iter()
    .map(|frequency_hz| EqualizerBandConfig {
        frequency_hz,
        gain_db: 0.0,
        q: default_q(),
    })
    .collect()
}

fn default_theme() -> String {
    "graphite".to_owned()
}

const fn default_volume() -> f32 {
    0.82
}

const fn default_true() -> bool {
    true
}

const fn default_q() -> f32 {
    1.15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_contains_core_modules() {
        let config = AppConfig::default();

        assert!(config.module_enabled(CORE_MODULE_ID));
        assert!(config.module_enabled(LOCAL_SOURCE_MODULE_ID));
        assert!(config.module_enabled(REPLAYGAIN_MODULE_ID));
        assert!(config.module_enabled(EQ_MODULE_ID));
        assert!(!config.module_enabled(LASTFM_MODULE_ID));
    }

    #[test]
    fn unique_paths_are_not_duplicated() {
        let mut paths = vec![PathBuf::from("/music")];

        push_unique_path(&mut paths, PathBuf::from("/music"));

        assert_eq!(paths, [PathBuf::from("/music")]);
    }
}

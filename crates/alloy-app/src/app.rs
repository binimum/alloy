use crate::audio::{
    AudioEngine, EqProcessorFactory, PlaybackState, ReplayGainProcessorFactory,
    SampleBlockProcessorFactory,
};
use crate::config::{
    self, AppConfig, AppPaths, DISCORD_MODULE_ID, EQ_MODULE_ID, LASTFM_MODULE_ID,
    LOCAL_SOURCE_MODULE_ID, REPLAYGAIN_MODULE_ID, RepeatMode, ReplayGainMode,
};
use crate::integrations::IntegrationManager;
use crate::library::{LocalMusicSource, format_duration, tracks_from_drop};
use crate::plugins::{DiscoveredModuleKind, PluginHost};
use crate::theme::{AlloyTheme, ThemeCatalog, ThemeColors, apply_theme, configure_fonts};
use alloy_core::{ModuleCategory, ModuleEvent, MusicSourceModule, Track};
use eframe::egui;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct AlloyApp {
    paths: AppPaths,
    config: AppConfig,
    local_source: LocalMusicSource,
    library: Vec<Track>,
    selected_track: Option<usize>,
    search: String,
    path_entry: String,
    audio: Option<AudioEngine>,
    audio_error: Option<String>,
    integrations: IntegrationManager,
    themes: ThemeCatalog,
    plugins: PluginHost,
    status_line: String,
    last_position_event: Instant,
    settings_open: bool,
    settings_tab: SettingsTab,
    config_text: String,
    config_text_dirty: bool,
    settings_error: Option<String>,
    selected_chain_node: Option<String>,
    cover_cache: BTreeMap<PathBuf, Option<egui::TextureHandle>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Audio,
    Integrations,
    Modules,
    Config,
}

#[derive(Debug, Clone, Copy)]
enum TransportIcon {
    Previous,
    Play,
    Pause,
    Next,
    Stop,
    Shuffle,
    Repeat,
    RepeatOne,
}

impl AlloyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let paths = AppPaths::discover()?;
        let mut config = AppConfig::load(&paths)?;
        config.discord = config.discord.clone().with_env();
        config::apply_cli_library_paths(&mut config);
        let mut config_changed = false;

        let themes = ThemeCatalog::load(&paths.themes_dir);
        if themes.find(&config.active_theme).is_none() {
            config.active_theme = themes.default_theme().definition.id.clone();
            config_changed = true;
        }
        let active_theme = themes
            .find(&config.active_theme)
            .unwrap_or_else(|| themes.default_theme());
        configure_fonts(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx, active_theme);

        let plugins = PluginHost::load(&paths.modules_dir);
        for module in plugins.modules() {
            config_changed |= config
                .ensure_module_toggle(&module.manifest.id, module.manifest.enabled_by_default);
            if module.manifest.category == ModuleCategory::AudioProcessor {
                config_changed |= config.ensure_audio_chain_node(
                    &module.manifest.id,
                    module.manifest.enabled_by_default,
                );
            }
        }
        if config_changed {
            config.save(&paths)?;
        }

        let local_source = LocalMusicSource::default();
        let library = if config.module_enabled(LOCAL_SOURCE_MODULE_ID) {
            local_source.scan(&config.library_paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let (audio, audio_error) = match AudioEngine::new(config.volume) {
            Ok(engine) => (Some(engine), None),
            Err(err) => (None, Some(err.to_string())),
        };

        let integrations = IntegrationManager::new(&config);
        let status_line = if library.is_empty() {
            "Library ready".to_owned()
        } else {
            format!("Loaded {} tracks", library.len())
        };
        let config_text = toml::to_string_pretty(&config).unwrap_or_default();

        Ok(Self {
            paths,
            config,
            local_source,
            library,
            selected_track: None,
            search: String::new(),
            path_entry: String::new(),
            audio,
            audio_error,
            integrations,
            themes,
            plugins,
            status_line,
            last_position_event: Instant::now(),
            settings_open: false,
            settings_tab: SettingsTab::General,
            config_text,
            config_text_dirty: false,
            settings_error: None,
            selected_chain_node: Some(REPLAYGAIN_MODULE_ID.to_owned()),
            cover_cache: BTreeMap::new(),
        })
    }

    fn active_theme(&self) -> &AlloyTheme {
        self.themes
            .find(&self.config.active_theme)
            .unwrap_or_else(|| self.themes.default_theme())
    }

    fn active_colors(&self) -> ThemeColors {
        ThemeColors::from_definition(&self.active_theme().definition)
    }

    fn save_config(&mut self) {
        if let Err(err) = self.config.save(&self.paths) {
            self.status_line = format!("Could not save config: {err}");
            return;
        }

        if !self.config_text_dirty {
            self.refresh_config_text();
        }
    }

    fn refresh_config_text(&mut self) {
        match toml::to_string_pretty(&self.config) {
            Ok(raw) => {
                self.config_text = raw;
                self.config_text_dirty = false;
                self.settings_error = None;
            }
            Err(err) => {
                self.settings_error = Some(format!("Could not serialize config: {err}"));
            }
        }
    }

    fn apply_config_text(&mut self, ctx: &egui::Context) {
        match toml::from_str::<AppConfig>(&self.config_text) {
            Ok(mut config) => {
                config.ensure_defaults();
                for module in self.plugins.modules() {
                    config.ensure_module_toggle(
                        &module.manifest.id,
                        module.manifest.enabled_by_default,
                    );
                    if module.manifest.category == ModuleCategory::AudioProcessor {
                        config.ensure_audio_chain_node(
                            &module.manifest.id,
                            module.manifest.enabled_by_default,
                        );
                    }
                }
                config.discord = config.discord.clone().with_env();
                self.config = config;
                self.config_text_dirty = false;
                self.settings_error = None;
                if let Some(audio) = &mut self.audio {
                    audio.set_volume(self.config.volume);
                }
                apply_theme(ctx, self.active_theme());
                self.integrations.rebuild(&self.config);
                self.rescan_library();
                self.save_config();
                self.refresh_config_text();
                self.status_line = "Applied alloy.toml".to_owned();
            }
            Err(err) => {
                self.settings_error = Some(format!("Could not parse alloy.toml: {err}"));
            }
        }
    }

    fn rescan_library(&mut self) {
        if !self.config.module_enabled(LOCAL_SOURCE_MODULE_ID) {
            self.library.clear();
            self.status_line = "Local source module is disabled".to_owned();
            return;
        }

        match self.local_source.scan(&self.config.library_paths) {
            Ok(tracks) => {
                self.library = tracks;
                self.selected_track = self
                    .selected_track
                    .filter(|index| *index < self.library.len());
                self.status_line = format!("Loaded {} tracks", self.library.len());
            }
            Err(err) => {
                self.status_line = format!("Library scan failed: {err}");
            }
        }
    }

    fn add_path_from_entry(&mut self) {
        let trimmed = self.path_entry.trim();
        if trimmed.is_empty() {
            return;
        }
        let path = PathBuf::from(trimmed);
        if path.exists() {
            config::push_unique_path(&mut self.config.library_paths, path);
            self.path_entry.clear();
            self.save_config();
            self.rescan_library();
        } else {
            self.status_line = "Path does not exist".to_owned();
        }
    }

    fn pick_library_folder(&mut self) {
        match crate::native_file_dialog::pick_folder("Choose a music folder") {
            Ok(Some(path)) => {
                config::push_unique_path(&mut self.config.library_paths, path);
                self.save_config();
                self.rescan_library();
            }
            Ok(None) => {}
            Err(err) => {
                self.status_line = err;
            }
        }
    }

    fn pick_library_files(&mut self) {
        match crate::native_file_dialog::pick_audio_files("Choose music files") {
            Ok(paths) => {
                if paths.is_empty() {
                    return;
                }
                for path in paths {
                    config::push_unique_path(&mut self.config.library_paths, path);
                }
                self.save_config();
                self.rescan_library();
            }
            Err(err) => {
                self.status_line = err;
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }

        let paths = dropped
            .into_iter()
            .filter_map(|file| file.path)
            .collect::<Vec<_>>();
        for path in &paths {
            if path.is_dir() {
                config::push_unique_path(&mut self.config.library_paths, path.clone());
            }
        }

        let mut tracks = tracks_from_drop(&paths);
        if tracks.is_empty() {
            self.status_line = "No supported audio files in drop".to_owned();
            return;
        }

        self.library.append(&mut tracks);
        self.library.sort_by(|a, b| {
            a.display_artist()
                .cmp(b.display_artist())
                .then_with(|| a.title.cmp(&b.title))
        });
        self.library.dedup_by(|a, b| a.path == b.path);
        self.save_config();
        self.status_line = format!("Library now has {} tracks", self.library.len());
    }

    fn check_finished_track(&mut self) {
        let Some(audio) = &mut self.audio else {
            return;
        };
        let Some(track) = audio.take_finished_track() else {
            return;
        };

        self.integrations
            .dispatch(&ModuleEvent::TrackFinished { track });
        self.advance_after_finished();
    }

    fn dispatch_position_event(&mut self) {
        if self.last_position_event.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_position_event = Instant::now();

        let Some(audio) = &self.audio else {
            return;
        };
        let snapshot = audio.snapshot();
        if snapshot.state == PlaybackState::Playing {
            self.integrations.dispatch(&ModuleEvent::PositionChanged {
                track: snapshot.track,
                position: snapshot.position,
            });
        }
    }

    fn play_index(&mut self, index: usize) {
        let Some(track) = self.library.get(index).cloned() else {
            return;
        };
        self.selected_track = Some(index);

        let processor_factories = self.audio_processor_factories_for_track(&track);
        let bit_perfect = self.config.playback.bit_perfect;
        let Some(audio) = &mut self.audio else {
            self.status_line = self
                .audio_error
                .clone()
                .unwrap_or_else(|| "Audio output is not available".to_owned());
            return;
        };

        match audio.play(track.clone(), processor_factories, bit_perfect) {
            Ok(()) => {
                self.status_line = "Playback started".to_owned();
                self.integrations
                    .dispatch(&ModuleEvent::PlaybackStarted { track });
            }
            Err(err) => {
                self.status_line = format!("Playback failed: {err}");
            }
        }
    }

    fn play_next(&mut self) {
        if self.library.is_empty() {
            return;
        }
        let next = if self.config.playback.shuffle {
            self.shuffled_index()
        } else {
            self.current_index()
                .map_or(0, |index| (index + 1) % self.library.len())
        };
        self.play_index(next);
    }

    fn play_previous(&mut self) {
        if self.library.is_empty() {
            return;
        }
        let previous = self.current_index().map_or(0, |index| {
            if index == 0 {
                self.library.len() - 1
            } else {
                index - 1
            }
        });
        self.play_index(previous);
    }

    fn rewind_or_previous(&mut self) {
        let position = self
            .audio
            .as_ref()
            .map_or(Duration::ZERO, AudioEngine::position);
        if position > Duration::from_secs(3) {
            self.seek_to_position(Duration::ZERO);
            return;
        }

        self.play_previous();
    }

    fn advance_after_finished(&mut self) {
        if self.library.is_empty() {
            return;
        }

        if self.config.playback.repeat == RepeatMode::One {
            if let Some(index) = self.selected_track {
                self.play_index(index);
            }
            return;
        }

        if self.config.playback.shuffle {
            self.play_index(self.shuffled_index());
            return;
        }

        let current = self.selected_track.unwrap_or(0);
        if current + 1 < self.library.len() {
            self.play_index(current + 1);
        } else if self.config.playback.repeat == RepeatMode::All {
            self.play_index(0);
        } else {
            self.status_line = "Playback finished".to_owned();
        }
    }

    fn shuffled_index(&self) -> usize {
        let len = self.library.len();
        if len <= 1 {
            return 0;
        }

        let current = self.current_index().unwrap_or(0);
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.subsec_nanos() as usize);
        let mut next = seed % len;
        if next == current {
            next = (next + 1) % len;
        }
        next
    }

    fn toggle_playback(&mut self) {
        let state = self
            .audio
            .as_ref()
            .map_or(PlaybackState::Stopped, AudioEngine::state);
        match state {
            PlaybackState::Stopped => {
                let index = self.selected_track.unwrap_or(0);
                self.play_index(index);
            }
            PlaybackState::Playing => {
                if let Some(audio) = &mut self.audio {
                    let track = audio.current_track().cloned();
                    audio.pause();
                    self.integrations
                        .dispatch(&ModuleEvent::PlaybackPaused { track });
                }
            }
            PlaybackState::Paused => {
                if let Some(audio) = &mut self.audio {
                    let track = audio.current_track().cloned();
                    audio.resume();
                    self.integrations
                        .dispatch(&ModuleEvent::PlaybackResumed { track });
                }
            }
        }
    }

    fn stop(&mut self) {
        if let Some(audio) = &mut self.audio {
            audio.stop();
        }
        self.integrations.dispatch(&ModuleEvent::PlaybackStopped);
    }

    fn apply_eq_to_current(&mut self) {
        let Some((track, position)) = self.audio.as_ref().and_then(|audio| {
            audio
                .current_track()
                .cloned()
                .map(|track| (track, audio.position()))
        }) else {
            return;
        };

        let processor_factories = self.audio_processor_factories_for_track(&track);
        let bit_perfect = self.config.playback.bit_perfect;
        let Some(audio) = &mut self.audio else {
            return;
        };
        if let Err(err) = audio.play_from(track, position, processor_factories, bit_perfect) {
            self.status_line = format!("Could not apply audio chain: {err}");
        }
    }

    fn seek_to_fraction(&mut self, fraction: f32) {
        let Some((track, duration)) = self.audio.as_ref().and_then(|audio| {
            audio
                .current_track()
                .cloned()
                .zip(audio.snapshot().duration)
        }) else {
            return;
        };
        let target = duration.mul_f32(fraction.clamp(0.0, 1.0));
        self.seek_to_position_for_track(track, target);
    }

    fn seek_to_position(&mut self, target: Duration) {
        let Some(track) = self
            .audio
            .as_ref()
            .and_then(|audio| audio.current_track().cloned())
        else {
            return;
        };
        self.seek_to_position_for_track(track, target);
    }

    fn seek_to_position_for_track(&mut self, track: Track, target: Duration) {
        let processor_factories = self.audio_processor_factories_for_track(&track);
        let bit_perfect = self.config.playback.bit_perfect;
        let Some(audio) = &mut self.audio else {
            return;
        };
        if let Err(err) = audio.play_from(track, target, processor_factories, bit_perfect) {
            self.status_line = format!("Seek failed: {err}");
        }
    }

    fn audio_processor_factories_for_track(
        &self,
        track: &Track,
    ) -> Vec<Box<dyn SampleBlockProcessorFactory>> {
        if self.config.playback.bit_perfect {
            return Vec::new();
        }

        let mut factories: Vec<Box<dyn SampleBlockProcessorFactory>> = Vec::new();

        for node in &self.config.audio_chain {
            if !node.enabled || !self.config.module_enabled(&node.id) {
                continue;
            }

            if node.id == REPLAYGAIN_MODULE_ID {
                let profile = self.config.replay_gain_profile(track);
                if profile.enabled {
                    factories.push(Box::new(ReplayGainProcessorFactory::new(profile)));
                }
            } else if node.id == EQ_MODULE_ID {
                let profile = self.config.eq_profile();
                if profile.enabled {
                    factories.push(Box::new(EqProcessorFactory::new(profile)));
                }
            } else if let Some(factory) = self.plugins.audio_processor_factory(&node.id) {
                factories.push(factory);
            }
        }

        factories
    }

    fn available_processors(&self) -> Vec<(String, String)> {
        let mut processors = vec![
            (REPLAYGAIN_MODULE_ID.to_owned(), "ReplayGain".to_owned()),
            (EQ_MODULE_ID.to_owned(), "Equalizer".to_owned()),
        ];
        processors.extend(
            self.plugins
                .modules()
                .iter()
                .filter(|module| module.manifest.category == ModuleCategory::AudioProcessor)
                .map(|module| (module.manifest.id.clone(), module.manifest.name.clone())),
        );
        processors
    }

    fn processor_label(&self, id: &str) -> String {
        if id == REPLAYGAIN_MODULE_ID {
            return "ReplayGain".to_owned();
        }

        if id == EQ_MODULE_ID {
            return "Equalizer".to_owned();
        }

        self.plugins
            .modules()
            .iter()
            .find(|module| module.manifest.id == id)
            .map_or_else(|| id.to_owned(), |module| module.manifest.name.clone())
    }

    fn current_index(&self) -> Option<usize> {
        let current_id = self
            .audio
            .as_ref()
            .and_then(AudioEngine::current_track)
            .map(|track| track.id.as_str());

        current_id
            .and_then(|id| self.library.iter().position(|track| track.id == id))
            .or(self.selected_track)
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.search.trim().to_lowercase();
        self.library
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                if query.is_empty()
                    || track.title.to_lowercase().contains(&query)
                    || track.artist.to_lowercase().contains(&query)
                    || track.album.to_lowercase().contains(&query)
                {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    fn module_toggle_ui(&mut self, ui: &mut egui::Ui, id: &str, label: &str) {
        let mut enabled = self.config.module_enabled(id);
        if ui.checkbox(&mut enabled, label).changed() {
            self.config.set_module_enabled(id, enabled);
            self.save_config();
            if matches!(id, LASTFM_MODULE_ID | DISCORD_MODULE_ID) {
                self.integrations.rebuild(&self.config);
            }
            if id == LOCAL_SOURCE_MODULE_ID {
                self.rescan_library();
            }
        }
    }

    fn top_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.label(egui::RichText::new("Alloy").strong().size(16.0));
            ui.separator();
            if ui.button("Settings").clicked() {
                self.settings_open = true;
                self.refresh_config_text();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(&self.status_line).small());
            });
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }

        let mut open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .default_width(650.0)
            .default_height(560.0)
            .resizable(true)
            .show(ctx, |ui| self.settings_ui(ui, ctx));
        self.settings_open = open;
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            settings_tab_button(ui, &mut self.settings_tab, SettingsTab::General, "General");
            settings_tab_button(
                ui,
                &mut self.settings_tab,
                SettingsTab::Audio,
                "Audio Chain",
            );
            settings_tab_button(
                ui,
                &mut self.settings_tab,
                SettingsTab::Integrations,
                "Integrations",
            );
            settings_tab_button(ui, &mut self.settings_tab, SettingsTab::Modules, "Modules");
            settings_tab_button(ui, &mut self.settings_tab, SettingsTab::Config, "Config");
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.settings_tab {
                SettingsTab::General => self.settings_general_ui(ui, ctx),
                SettingsTab::Audio => self.settings_audio_ui(ui),
                SettingsTab::Integrations => self.settings_integrations_ui(ui),
                SettingsTab::Modules => self.settings_modules_ui(ui),
                SettingsTab::Config => self.settings_config_ui(ui, ctx),
            });
    }

    fn settings_general_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("General");
        ui.label(format!("Config: {}", self.paths.config_file.display()));
        ui.add_space(8.0);

        self.theme_ui(ui, ctx);

        ui.add_space(10.0);
        let mut changed = false;
        changed |= thin_slider(
            ui,
            egui::Slider::new(&mut self.config.volume, 0.0..=1.5)
                .text("Default volume")
                .show_value(true),
            360.0,
        )
        .changed();
        if changed {
            if let Some(audio) = &mut self.audio {
                audio.set_volume(self.config.volume);
            }
            self.save_config();
        }

        ui.separator();
        ui.label(egui::RichText::new("Album Covers").strong());
        let mut cover_changed = false;
        cover_changed |= ui
            .checkbox(&mut self.config.cover_art.enabled, "Show album covers")
            .changed();
        cover_changed |= ui
            .checkbox(
                &mut self.config.cover_art.sidecar_images,
                "Use sidecar images beside tracks",
            )
            .changed();
        if cover_changed {
            self.save_config();
        }

        ui.separator();
        ui.label(egui::RichText::new("Library Paths").strong());
        let paths = self.config.library_paths.clone();
        for (index, path) in paths.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(path.display().to_string());
                if ui.button("Remove").clicked() {
                    self.config.library_paths.remove(index);
                    self.save_config();
                    self.rescan_library();
                }
            });
        }
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.path_entry);
            if ui.button("Add").clicked() {
                self.add_path_from_entry();
            }
            if ui.button("Choose Folder...").clicked() {
                self.pick_library_folder();
            }
            if ui.button("Choose Files...").clicked() {
                self.pick_library_files();
            }
        });
    }

    fn settings_audio_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Audio Chain");
        let mut playback_changed = false;
        ui.horizontal_wrapped(|ui| {
            playback_changed |= ui
                .checkbox(&mut self.config.playback.bit_perfect, "Bit-perfect")
                .changed();
            playback_changed |= ui
                .checkbox(&mut self.config.playback.shuffle, "Shuffle")
                .changed();
            ui.label("Repeat");
            egui::ComboBox::from_id_salt("repeat-mode")
                .selected_text(match self.config.playback.repeat {
                    RepeatMode::None => "Off",
                    RepeatMode::All => "All",
                    RepeatMode::One => "One",
                })
                .show_ui(ui, |ui| {
                    playback_changed |= ui
                        .selectable_value(&mut self.config.playback.repeat, RepeatMode::None, "Off")
                        .changed();
                    playback_changed |= ui
                        .selectable_value(&mut self.config.playback.repeat, RepeatMode::All, "All")
                        .changed();
                    playback_changed |= ui
                        .selectable_value(&mut self.config.playback.repeat, RepeatMode::One, "One")
                        .changed();
                });
        });
        if playback_changed {
            self.save_config();
            self.status_line = "Playback settings saved".to_owned();
        }
        ui.separator();
        ui.label("Processors run left to right when playback starts.");
        ui.add_space(8.0);
        self.audio_chain_ui(ui);
    }

    fn audio_chain_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        if self.selected_chain_node.as_ref().is_none_or(|selected| {
            !self
                .config
                .audio_chain
                .iter()
                .any(|node| node.id == *selected)
        }) {
            self.selected_chain_node = self.config.audio_chain.first().map(|node| node.id.clone());
        }

        let chain = self.config.audio_chain.clone();
        egui::ScrollArea::horizontal()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    for (index, node) in chain.iter().enumerate() {
                        let label = self.processor_label(&node.id);
                        let selected =
                            self.selected_chain_node.as_deref() == Some(node.id.as_str());
                        let text = if node.enabled {
                            label
                        } else {
                            format!("{label} (off)")
                        };
                        if ui
                            .add_sized([142.0, 38.0], egui::Button::selectable(selected, text))
                            .on_hover_text(&node.id)
                            .clicked()
                        {
                            self.selected_chain_node = Some(node.id.clone());
                        }

                        if index + 1 < chain.len() {
                            ui.label(egui::RichText::new("->").strong());
                        }
                    }
                });
            });

        ui.add_space(10.0);
        let selected_id = self.selected_chain_node.clone();
        if let Some(selected_id) = selected_id {
            if let Some(index) = self
                .config
                .audio_chain
                .iter()
                .position(|node| node.id == selected_id)
            {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(self.processor_label(&selected_id)).strong());
                    let mut enabled = self.config.audio_chain[index].enabled;
                    if ui.checkbox(&mut enabled, "Enabled").changed() {
                        self.config.audio_chain[index].enabled = enabled;
                        changed = true;
                    }
                    if ui.button("Move Left").clicked() && index > 0 {
                        self.config.audio_chain.swap(index, index - 1);
                        changed = true;
                    }
                    if ui.button("Move Right").clicked()
                        && index + 1 < self.config.audio_chain.len()
                    {
                        self.config.audio_chain.swap(index, index + 1);
                        changed = true;
                    }
                    if selected_id != EQ_MODULE_ID
                        && selected_id != REPLAYGAIN_MODULE_ID
                        && ui.button("Remove").clicked()
                    {
                        self.config.audio_chain.remove(index);
                        self.selected_chain_node =
                            self.config.audio_chain.first().map(|node| node.id.clone());
                        changed = true;
                    }
                });
                ui.add_space(8.0);
                if selected_id == REPLAYGAIN_MODULE_ID {
                    self.replay_gain_ui(ui);
                } else if selected_id == EQ_MODULE_ID {
                    self.equalizer_ui(ui);
                } else {
                    ui.label(selected_id);
                }
            }
        }

        let available = self
            .available_processors()
            .into_iter()
            .filter(|(id, _)| !self.config.audio_chain.iter().any(|node| node.id == *id))
            .collect::<Vec<_>>();

        if !available.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("Available Processors").strong());
            for (id, name) in available {
                ui.horizontal(|ui| {
                    ui.label(name);
                    if ui.button("Add").clicked() {
                        self.config.audio_chain.push(config::AudioChainNodeConfig {
                            id: id.clone(),
                            enabled: true,
                        });
                        self.selected_chain_node = Some(id);
                        changed = true;
                    }
                });
            }
        }

        if changed {
            self.save_config();
            self.status_line = "Audio chain changes apply on next playback start".to_owned();
        }
    }

    fn settings_integrations_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Integrations");
        self.module_toggle_ui(ui, LASTFM_MODULE_ID, "Enable Last.fm");
        self.module_toggle_ui(ui, DISCORD_MODULE_ID, "Enable Discord RPC");

        ui.separator();
        ui.label(egui::RichText::new("Last.fm").strong());
        let mut changed = false;
        changed |= option_text_field(ui, "API key", &mut self.config.lastfm.api_key, false);
        changed |= option_text_field(ui, "API secret", &mut self.config.lastfm.api_secret, true);
        changed |= option_text_field(ui, "Session key", &mut self.config.lastfm.session_key, true);
        changed |= ui
            .checkbox(&mut self.config.lastfm.scrobble, "Scrobble played tracks")
            .changed();

        ui.separator();
        ui.label(egui::RichText::new("Discord").strong());
        changed |= option_text_field(ui, "Client ID", &mut self.config.discord.client_id, false);

        if changed {
            self.save_config();
            self.integrations.rebuild(&self.config);
        }
    }

    fn settings_modules_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Modules");
        self.module_toggle_ui(ui, LOCAL_SOURCE_MODULE_ID, "Local library");
        self.module_toggle_ui(ui, REPLAYGAIN_MODULE_ID, "ReplayGain");
        self.module_toggle_ui(ui, EQ_MODULE_ID, "Equalizer");
        self.module_toggle_ui(ui, LASTFM_MODULE_ID, "Last.fm");
        self.module_toggle_ui(ui, DISCORD_MODULE_ID, "Discord RPC");
        ui.separator();
        self.community_modules_ui(ui, self.active_colors());
    }

    fn settings_config_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("alloy.toml");
        ui.label(self.paths.config_file.display().to_string());
        if let Some(error) = &self.settings_error {
            ui.label(egui::RichText::new(error).color(self.active_colors().danger));
        }
        ui.horizontal(|ui| {
            if ui.button("Reload").clicked() {
                match std::fs::read_to_string(&self.paths.config_file) {
                    Ok(raw) => {
                        self.config_text = raw;
                        self.config_text_dirty = false;
                        self.settings_error = None;
                    }
                    Err(err) => {
                        self.settings_error = Some(format!("Could not reload config: {err}"));
                    }
                }
            }
            if ui.button("Apply").clicked() {
                self.apply_config_text(ctx);
            }
            if self.config_text_dirty {
                ui.label("Unsaved edits");
            }
        });
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.config_text)
                    .desired_rows(22)
                    .code_editor(),
            )
            .changed()
        {
            self.config_text_dirty = true;
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let colors = self.active_colors();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.heading(egui::RichText::new("Alloy").size(28.0).color(colors.text));
                ui.label(egui::RichText::new("Modular Rust music").color(colors.muted_text));
                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    if ui.button("Rescan").clicked() {
                        self.rescan_library();
                    }
                    if ui.button("Save").clicked() {
                        self.save_config();
                    }
                });

                ui.add_space(10.0);
                ui.label("Music path");
                let add_from_enter = ui.text_edit_singleline(&mut self.path_entry).lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.horizontal_wrapped(|ui| {
                    if add_from_enter || ui.button("Add").clicked() {
                        self.add_path_from_entry();
                    }
                    if ui.button("Choose...").clicked() {
                        self.pick_library_folder();
                    }
                });

                ui.add_space(12.0);
                ui.label(egui::RichText::new("Library").strong());
                ui.horizontal(|ui| {
                    ui.label(format!("{} tracks", self.library.len()));
                    ui.separator();
                    ui.label(format!("{} roots", self.config.library_paths.len()));
                });

                ui.add_space(12.0);
                ui.label(egui::RichText::new("Modules").strong());
                self.module_toggle_ui(ui, LOCAL_SOURCE_MODULE_ID, "Local library");
                self.module_toggle_ui(ui, REPLAYGAIN_MODULE_ID, "ReplayGain");
                self.module_toggle_ui(ui, EQ_MODULE_ID, "Equalizer");
                self.module_toggle_ui(ui, LASTFM_MODULE_ID, "Last.fm");
                self.module_toggle_ui(ui, DISCORD_MODULE_ID, "Discord RPC");

                ui.separator();
                ui.label(egui::RichText::new(&self.status_line).color(colors.muted_text));
                if let Some(error) = &self.audio_error {
                    ui.label(egui::RichText::new(error).color(colors.danger));
                }
            });
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let colors = self.active_colors();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);
                self.now_playing_ui(ui, ctx, colors);
                ui.separator();
                self.replay_gain_ui(ui);
                ui.separator();
                self.equalizer_ui(ui);
                ui.separator();
                self.theme_ui(ui, ctx);
                ui.separator();
                self.integration_ui(ui, colors);
                ui.separator();
                self.community_modules_ui(ui, colors);
            });
    }

    fn cover_texture(
        &mut self,
        ctx: &egui::Context,
        cover_path: &PathBuf,
    ) -> Option<egui::TextureHandle> {
        if let Some(cached) = self.cover_cache.get(cover_path) {
            return cached.clone();
        }

        let texture = load_cover_texture(ctx, cover_path);
        self.cover_cache.insert(cover_path.clone(), texture.clone());
        texture
    }

    fn cover_image_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        cover_path: Option<&PathBuf>,
        size: f32,
    ) {
        if !self.config.cover_art.sidecar_images {
            placeholder_cover(ui, size);
            return;
        }

        let Some(cover_path) = cover_path else {
            placeholder_cover(ui, size);
            return;
        };

        let Some(texture) = self.cover_texture(ctx, cover_path) else {
            placeholder_cover(ui, size);
            return;
        };

        let outline = if ui.visuals().dark_mode {
            egui::Color32::from_white_alpha(26)
        } else {
            egui::Color32::from_black_alpha(26)
        };
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, outline))
            .corner_radius(egui::CornerRadius::same(8))
            .show(ui, |ui| {
                ui.add(
                    egui::Image::from_texture(&texture)
                        .fit_to_exact_size(egui::vec2(size, size))
                        .corner_radius(8),
                );
            });
    }

    fn now_playing_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, colors: ThemeColors) {
        ui.label(egui::RichText::new("Now Playing").strong());
        let snapshot = self.audio.as_ref().map(AudioEngine::snapshot);
        if let Some(snapshot) = snapshot {
            if let Some(track) = snapshot.track.as_ref() {
                if self.config.cover_art.enabled {
                    self.cover_image_ui(ui, ctx, track.cover_path.as_ref(), 172.0);
                    ui.add_space(6.0);
                }
                ui.label(egui::RichText::new(&track.title).size(20.0).strong());
                ui.label(egui::RichText::new(track.display_artist()).color(colors.muted_text));
                ui.label(egui::RichText::new(track.path.display().to_string()).small());
                ui.add_space(6.0);
                ui.label(format!(
                    "{} / {}",
                    format_duration(Some(snapshot.position)),
                    format_duration(snapshot.duration)
                ));
                return;
            }
        }
        ui.label(egui::RichText::new("Idle").color(colors.muted_text));
    }

    fn replay_gain_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ReplayGain").strong());
            changed |= ui
                .checkbox(&mut self.config.replay_gain.enabled, "Enabled")
                .changed();
        });

        if !self.config.module_enabled(REPLAYGAIN_MODULE_ID) {
            ui.label("ReplayGain module disabled");
            if changed {
                self.save_config();
            }
            return;
        }

        ui.horizontal(|ui| {
            ui.label("Mode");
            egui::ComboBox::from_id_salt("replay-gain-mode")
                .selected_text(match self.config.replay_gain.mode {
                    ReplayGainMode::Track => "Track",
                    ReplayGainMode::Album => "Album",
                })
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.replay_gain.mode,
                            ReplayGainMode::Track,
                            "Track",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut self.config.replay_gain.mode,
                            ReplayGainMode::Album,
                            "Album",
                        )
                        .changed();
                });
        });
        changed |= thin_slider(
            ui,
            egui::Slider::new(&mut self.config.replay_gain.preamp_db, -12.0..=12.0)
                .text("Preamp")
                .suffix(" dB"),
            360.0,
        )
        .changed();
        changed |= ui
            .checkbox(
                &mut self.config.replay_gain.prevent_clipping,
                "Prevent clipping",
            )
            .changed();

        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                self.apply_eq_to_current();
            }
        });

        if changed {
            self.save_config();
        }
    }

    fn equalizer_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("EQ").strong());
            changed |= ui
                .checkbox(&mut self.config.equalizer.enabled, "Enabled")
                .changed();
        });

        if !self.config.module_enabled(EQ_MODULE_ID) {
            ui.label("Equalizer module disabled");
            if changed {
                self.save_config();
            }
            return;
        }

        changed |= thin_slider(
            ui,
            egui::Slider::new(&mut self.config.equalizer.preamp_db, -9.0..=9.0)
                .text("Preamp")
                .suffix(" dB"),
            360.0,
        )
        .changed();

        for band in &mut self.config.equalizer.bands {
            let label = format_band(band.frequency_hz);
            changed |= thin_slider(
                ui,
                egui::Slider::new(&mut band.gain_db, -12.0..=12.0)
                    .text(label)
                    .suffix(" dB"),
                360.0,
            )
            .changed();
        }

        ui.horizontal(|ui| {
            if ui.button("Flat").clicked() {
                self.config.equalizer.preamp_db = 0.0;
                for band in &mut self.config.equalizer.bands {
                    band.gain_db = 0.0;
                }
                changed = true;
            }
            if ui.button("Apply").clicked() {
                self.apply_eq_to_current();
            }
        });

        if changed {
            self.save_config();
        }
    }

    fn theme_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut active = self.config.active_theme.clone();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Theme").strong());
            egui::ComboBox::from_id_salt("theme-select")
                .width(190.0)
                .selected_text(
                    self.themes
                        .find(&active)
                        .map_or(active.as_str(), |theme| theme.definition.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for theme in self.themes.all() {
                        ui.selectable_value(
                            &mut active,
                            theme.definition.id.clone(),
                            theme.definition.name.as_str(),
                        )
                        .on_hover_text(theme.manifest.description.as_str());
                    }
                });
        });

        if active != self.config.active_theme {
            self.config.active_theme = active;
            apply_theme(ctx, self.active_theme());
            self.save_config();
        }
    }

    fn integration_ui(&mut self, ui: &mut egui::Ui, colors: ThemeColors) {
        ui.label(egui::RichText::new("Integrations").strong());
        for status in self.integrations.statuses() {
            ui.horizontal(|ui| {
                let color = if status.enabled && status.configured {
                    colors.accent
                } else {
                    colors.muted_text
                };
                ui.label(egui::RichText::new(status.name).color(color))
                    .on_hover_text(status.id);
                ui.label(if status.enabled {
                    "enabled"
                } else {
                    "disabled"
                });
                if status.enabled && !status.configured {
                    ui.label(egui::RichText::new("needs config").color(colors.danger));
                }
            });
        }
        if let Some(error) = self.integrations.last_error() {
            ui.label(egui::RichText::new(error).color(colors.danger));
        }
    }

    fn community_modules_ui(&mut self, ui: &mut egui::Ui, colors: ThemeColors) {
        ui.label(egui::RichText::new("Community Modules").strong());
        if self.plugins.modules().is_empty() {
            ui.label(egui::RichText::new("None installed").color(colors.muted_text));
            return;
        }

        let modules = self
            .plugins
            .modules()
            .iter()
            .map(|module| {
                (
                    module.manifest.id.clone(),
                    module.manifest.name.clone(),
                    module.manifest.version.clone(),
                    module.manifest.category,
                    module.kind,
                    module.path.display().to_string(),
                )
            })
            .collect::<Vec<_>>();

        egui::ScrollArea::vertical()
            .max_height(130.0)
            .show(ui, |ui| {
                for (id, name, version, category, kind, path) in modules {
                    let mut enabled = self.config.module_enabled(&id);
                    let kind = match kind {
                        DiscoveredModuleKind::Manifest => "manifest",
                        DiscoveredModuleKind::Native => "native",
                    };
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut enabled, "").changed() {
                            self.config.set_module_enabled(&id, enabled);
                            self.save_config();
                            self.status_line =
                                "Module changes apply on next playback start".to_owned();
                        }
                        ui.label(egui::RichText::new(name).strong())
                            .on_hover_text(path);
                    });
                    ui.label(
                        egui::RichText::new(format!(
                            "{} | {} | {}",
                            version,
                            category_name(category),
                            kind
                        ))
                        .color(colors.muted_text),
                    );
                }
            });
    }

    fn central_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, colors: ThemeColors) {
        ui.horizontal(|ui| {
            ui.heading("Library");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                search_field(ui, &mut self.search, colors);
            });
        });
        ui.add_space(8.0);

        if self.library.is_empty() {
            ui.vertical_centered_justified(|ui| {
                ui.add_space(120.0);
                ui.label(egui::RichText::new("No tracks loaded").size(22.0).strong());
                ui.label(
                    egui::RichText::new(
                        "Add a folder from the left panel or drop audio files here",
                    )
                    .color(colors.muted_text),
                );
            });
            return;
        }

        let indices = self.filtered_indices();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("track-grid")
                    .num_columns(5)
                    .spacing([16.0, 8.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Art").strong().color(colors.muted_text));
                        ui.label(
                            egui::RichText::new("Title")
                                .strong()
                                .color(colors.muted_text),
                        );
                        ui.label(
                            egui::RichText::new("Artist")
                                .strong()
                                .color(colors.muted_text),
                        );
                        ui.label(
                            egui::RichText::new("Album")
                                .strong()
                                .color(colors.muted_text),
                        );
                        ui.label(
                            egui::RichText::new("Time")
                                .strong()
                                .color(colors.muted_text),
                        );
                        ui.end_row();

                        for index in indices {
                            let (title, artist, album, duration, path, cover_path) = {
                                let track = &self.library[index];
                                (
                                    track.title.clone(),
                                    track.display_artist().to_owned(),
                                    track.display_album().to_owned(),
                                    track.duration,
                                    track.path.display().to_string(),
                                    track.cover_path.clone(),
                                )
                            };
                            let selected = self.selected_track == Some(index);
                            if self.config.cover_art.enabled {
                                self.cover_image_ui(ui, ctx, cover_path.as_ref(), 34.0);
                            } else {
                                ui.label("");
                            }
                            let response = ui.selectable_label(selected, title).on_hover_text(path);
                            if response.clicked() {
                                self.selected_track = Some(index);
                            }
                            if response.double_clicked() {
                                self.play_index(index);
                            }
                            ui.label(artist);
                            ui.label(album);
                            ui.label(format_duration(duration));
                            ui.end_row();
                        }
                    });
            });
    }

    fn transport_panel(&mut self, ui: &mut egui::Ui) {
        let snapshot = self.audio.as_ref().map(AudioEngine::snapshot);
        let state = snapshot
            .as_ref()
            .map_or(PlaybackState::Stopped, |snapshot| snapshot.state);
        let (position, duration, volume) = snapshot
            .as_ref()
            .map_or((Duration::ZERO, None, self.config.volume), |snapshot| {
                (snapshot.position, snapshot.duration, snapshot.volume)
            });

        let available_width = ui.available_width();
        let show_stop = available_width > 560.0;
        let show_volume = available_width > 860.0 && !self.config.playback.bit_perfect;
        let show_time = available_width > 520.0;
        let progress_width = if available_width > 760.0 {
            (available_width * 0.32).clamp(180.0, 360.0)
        } else if available_width > 620.0 {
            150.0
        } else {
            0.0
        };

        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ui.available_height()),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if transport_icon_button(
                    ui,
                    TransportIcon::Shuffle,
                    "Shuffle",
                    self.config.playback.shuffle,
                )
                .clicked()
                {
                    self.config.playback.shuffle = !self.config.playback.shuffle;
                    self.save_config();
                }
                let repeat_icon = if self.config.playback.repeat == RepeatMode::One {
                    TransportIcon::RepeatOne
                } else {
                    TransportIcon::Repeat
                };
                if transport_icon_button(
                    ui,
                    repeat_icon,
                    "Repeat",
                    self.config.playback.repeat != RepeatMode::None,
                )
                .clicked()
                {
                    self.config.playback.repeat = match self.config.playback.repeat {
                        RepeatMode::None => RepeatMode::All,
                        RepeatMode::All => RepeatMode::One,
                        RepeatMode::One => RepeatMode::None,
                    };
                    self.save_config();
                }

                ui.separator();
                if transport_icon_button(
                    ui,
                    TransportIcon::Previous,
                    "Restart or previous track",
                    false,
                )
                .clicked()
                {
                    self.rewind_or_previous();
                }
                let play_icon = match state {
                    PlaybackState::Playing => TransportIcon::Pause,
                    PlaybackState::Paused | PlaybackState::Stopped => TransportIcon::Play,
                };
                let play_tip = match state {
                    PlaybackState::Playing => "Pause",
                    PlaybackState::Paused | PlaybackState::Stopped => "Play",
                };
                if transport_icon_button(ui, play_icon, play_tip, false).clicked() {
                    self.toggle_playback();
                }
                if transport_icon_button(ui, TransportIcon::Next, "Next track", false).clicked() {
                    self.play_next();
                }
                if show_stop
                    && transport_icon_button(ui, TransportIcon::Stop, "Stop", false).clicked()
                {
                    self.stop();
                }

                if progress_width > 0.0 {
                    ui.separator();
                    if show_time {
                        ui.label(format_duration(Some(position)));
                    }
                    let mut progress = duration.map_or(0.0, |duration| {
                        if duration.is_zero() {
                            0.0
                        } else {
                            position.as_secs_f32() / duration.as_secs_f32()
                        }
                    });
                    let progress_response = thin_slider(
                        ui,
                        egui::Slider::new(&mut progress, 0.0..=1.0).show_value(false),
                        progress_width,
                    );
                    if progress_response.drag_stopped() || progress_response.clicked() {
                        self.seek_to_fraction(progress);
                    }
                    if show_time {
                        ui.label(format_duration(duration));
                    }
                }

                ui.separator();
                if self.config.playback.bit_perfect {
                    ui.label("Bit-perfect");
                } else if show_volume {
                    ui.label("Volume");
                    let mut new_volume = volume;
                    if thin_slider(
                        ui,
                        egui::Slider::new(&mut new_volume, 0.0..=1.5).show_value(false),
                        120.0,
                    )
                    .changed()
                    {
                        self.config.volume = new_volume;
                        if let Some(audio) = &mut self.audio {
                            audio.set_volume(new_volume);
                        }
                        self.save_config();
                    }
                } else {
                    ui.label(format!("{:.0}%", volume * 100.0));
                }
            },
        );
    }
}

impl eframe::App for AlloyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_dropped_files(ctx);
        self.check_finished_track();
        self.dispatch_position_event();

        egui::TopBottomPanel::top("alloy-top-bar")
            .resizable(false)
            .exact_height(42.0)
            .show(ctx, |ui| self.top_panel(ui));

        self.settings_window(ctx);

        egui::SidePanel::left("alloy-side-panel")
            .resizable(false)
            .exact_width(235.0)
            .show(ctx, |ui| self.side_panel(ui));

        egui::SidePanel::right("alloy-right-panel")
            .resizable(true)
            .default_width(310.0)
            .width_range(270.0..=390.0)
            .show(ctx, |ui| self.right_panel(ui, ctx));

        egui::TopBottomPanel::bottom("alloy-transport")
            .resizable(false)
            .exact_height(66.0)
            .show(ctx, |ui| self.transport_panel(ui));

        let colors = self.active_colors();
        egui::CentralPanel::default().show(ctx, |ui| self.central_panel(ui, ctx, colors));

        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn format_band(frequency_hz: f32) -> String {
    if frequency_hz >= 1_000.0 {
        format!("{:.0}k", frequency_hz / 1_000.0)
    } else {
        format!("{frequency_hz:.0}")
    }
}

fn settings_tab_button(ui: &mut egui::Ui, active: &mut SettingsTab, tab: SettingsTab, label: &str) {
    if ui.selectable_label(*active == tab, label).clicked() {
        *active = tab;
    }
}

fn thin_slider<'a>(ui: &mut egui::Ui, slider: egui::Slider<'a>, width: f32) -> egui::Response {
    ui.add_sized(
        [
            width.min(ui.available_width()).max(96.0),
            platform_slider_height(),
        ],
        slider,
    )
}

#[cfg(target_os = "macos")]
const fn platform_slider_height() -> f32 {
    22.0
}

#[cfg(not(target_os = "macos"))]
const fn platform_slider_height() -> f32 {
    18.0
}

fn search_field(ui: &mut egui::Ui, search: &mut String, colors: ThemeColors) {
    let width = ui.available_width().min(310.0).max(210.0);
    egui::Frame::new()
        .fill(colors.surface_muted)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 5))
        .show(ui, |ui| {
            ui.set_min_width(width);
            ui.horizontal_centered(|ui| {
                let (icon_rect, _) =
                    ui.allocate_exact_size(egui::vec2(18.0, 20.0), egui::Sense::hover());
                draw_search_icon(ui.painter(), icon_rect, colors.muted_text);
                ui.add_sized(
                    [ui.available_width(), 22.0],
                    egui::TextEdit::singleline(search)
                        .hint_text("Search library")
                        .frame(false),
                );
            });
        });
}

fn draw_search_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = egui::pos2(rect.left() + 7.5, rect.center().y - 1.5);
    painter.circle_stroke(center, 5.0, egui::Stroke::new(1.7, color));
    painter.line_segment(
        [
            egui::pos2(center.x + 3.8, center.y + 3.8),
            egui::pos2(rect.right() - 2.0, rect.bottom() - 3.0),
        ],
        egui::Stroke::new(1.7, color),
    );
}

fn option_text_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<String>,
    password: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        let mut text = value.clone().unwrap_or_default();
        let response = ui.add(
            egui::TextEdit::singleline(&mut text)
                .password(password)
                .desired_width(360.0),
        );
        if response.changed() {
            *value = if text.trim().is_empty() {
                None
            } else {
                Some(text)
            };
            changed = true;
        }
    });
    changed
}

fn load_cover_texture(ctx: &egui::Context, cover_path: &PathBuf) -> Option<egui::TextureHandle> {
    let image = image::ImageReader::open(cover_path).ok()?.decode().ok()?;
    let image = image.thumbnail(768, 768).to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(
        format!("cover:{}", cover_path.display()),
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

fn placeholder_cover(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let fill = ui.visuals().faint_bg_color;
    let outline = if ui.visuals().dark_mode {
        egui::Color32::from_white_alpha(26)
    } else {
        egui::Color32::from_black_alpha(26)
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(8), fill);
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(8),
        egui::Stroke::new(1.0, outline),
        egui::StrokeKind::Inside,
    );

    let center = rect.center();
    let radius = size * 0.18;
    ui.painter().circle_stroke(
        center,
        radius,
        egui::Stroke::new(2.0, ui.visuals().weak_text_color()),
    );
    ui.painter()
        .circle_filled(center, radius * 0.18, ui.visuals().weak_text_color());
}

fn transport_icon_button(
    ui: &mut egui::Ui,
    icon: TransportIcon,
    tooltip: &'static str,
    active: bool,
) -> egui::Response {
    let mut button = egui::Button::new("");
    if active {
        button = button.fill(ui.visuals().selection.bg_fill);
    }
    let response = ui.add_sized([42.0, 38.0], button).on_hover_text(tooltip);
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
    } else if active {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().text_color()
    };
    draw_transport_icon(ui.painter(), response.rect, icon, color);
    response
}

fn draw_transport_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: TransportIcon,
    color: egui::Color32,
) {
    let rect = rect.shrink2(egui::vec2(11.0, 9.0));
    let center_y = rect.center().y;
    match icon {
        TransportIcon::Play => {
            let points = vec![
                egui::pos2(rect.left() + 3.0, rect.top()),
                egui::pos2(rect.left() + 3.0, rect.bottom()),
                egui::pos2(rect.right(), center_y),
            ];
            painter.add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::NONE,
            ));
        }
        TransportIcon::Pause => {
            let bar_width = rect.width() * 0.28;
            let gap = rect.width() * 0.18;
            let left = egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.left() + bar_width, rect.bottom()),
            );
            let right = egui::Rect::from_min_max(
                egui::pos2(rect.left() + bar_width + gap, rect.top()),
                egui::pos2(rect.left() + bar_width * 2.0 + gap, rect.bottom()),
            );
            painter.rect_filled(left, 2, color);
            painter.rect_filled(right, 2, color);
        }
        TransportIcon::Stop => {
            let square = egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0));
            painter.rect_filled(square, 3, color);
        }
        TransportIcon::Previous => {
            let bar = egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.left() + 2.5, rect.bottom()),
            );
            painter.rect_filled(bar, 1, color);
            let first = vec![
                egui::pos2(rect.left() + 4.5, center_y),
                egui::pos2(rect.center().x + 1.5, rect.top()),
                egui::pos2(rect.center().x + 1.5, rect.bottom()),
            ];
            let second = vec![
                egui::pos2(rect.center().x, center_y),
                egui::pos2(rect.right(), rect.top()),
                egui::pos2(rect.right(), rect.bottom()),
            ];
            painter.add(egui::Shape::convex_polygon(
                first,
                color,
                egui::Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                second,
                color,
                egui::Stroke::NONE,
            ));
        }
        TransportIcon::Next => {
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 2.5, rect.top()),
                rect.right_bottom(),
            );
            painter.rect_filled(bar, 1, color);
            let first = vec![
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.left(), rect.bottom()),
                egui::pos2(rect.center().x, center_y),
            ];
            let second = vec![
                egui::pos2(rect.center().x - 1.5, rect.top()),
                egui::pos2(rect.center().x - 1.5, rect.bottom()),
                egui::pos2(rect.right() - 4.5, center_y),
            ];
            painter.add(egui::Shape::convex_polygon(
                first,
                color,
                egui::Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                second,
                color,
                egui::Stroke::NONE,
            ));
        }
        TransportIcon::Shuffle => {
            let stroke = egui::Stroke::new(2.0, color);
            let left_top = egui::pos2(rect.left(), rect.top() + rect.height() * 0.28);
            let left_bottom = egui::pos2(rect.left(), rect.bottom() - rect.height() * 0.28);
            let right_top = egui::pos2(rect.right() - 4.0, rect.top() + rect.height() * 0.25);
            let right_bottom = egui::pos2(rect.right() - 4.0, rect.bottom() - rect.height() * 0.25);
            painter.line_segment([left_top, rect.center()], stroke);
            painter.line_segment([rect.center(), right_bottom], stroke);
            painter.line_segment(
                [left_bottom, egui::pos2(rect.center().x, left_bottom.y)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(rect.center().x, left_bottom.y), right_top],
                stroke,
            );
            draw_arrow_head(painter, right_top, true, color);
            draw_arrow_head(painter, right_bottom, true, color);
        }
        TransportIcon::Repeat | TransportIcon::RepeatOne => {
            let stroke = egui::Stroke::new(2.0, color);
            let top_y = rect.top() + rect.height() * 0.28;
            let bottom_y = rect.bottom() - rect.height() * 0.28;
            painter.line_segment(
                [
                    egui::pos2(rect.left() + 4.0, top_y),
                    egui::pos2(rect.right() - 5.0, top_y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(rect.right() - 4.0, bottom_y),
                    egui::pos2(rect.left() + 5.0, bottom_y),
                ],
                stroke,
            );
            draw_arrow_head(painter, egui::pos2(rect.right() - 4.0, top_y), true, color);
            draw_arrow_head(
                painter,
                egui::pos2(rect.left() + 4.0, bottom_y),
                false,
                color,
            );
            if matches!(icon, TransportIcon::RepeatOne) {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "1",
                    egui::FontId::proportional(12.0),
                    color,
                );
            }
        }
    }
}

fn draw_arrow_head(
    painter: &egui::Painter,
    tip: egui::Pos2,
    points_right: bool,
    color: egui::Color32,
) {
    let direction = if points_right { 1.0 } else { -1.0 };
    let points = vec![
        tip,
        egui::pos2(tip.x - direction * 5.0, tip.y - 4.0),
        egui::pos2(tip.x - direction * 5.0, tip.y + 4.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

fn category_name(category: alloy_core::ModuleCategory) -> &'static str {
    match category {
        alloy_core::ModuleCategory::Core => "core",
        alloy_core::ModuleCategory::MusicSource => "source",
        alloy_core::ModuleCategory::AudioProcessor => "audio",
        alloy_core::ModuleCategory::Theme => "theme",
        alloy_core::ModuleCategory::UiElement => "ui",
        alloy_core::ModuleCategory::Integration => "integration",
    }
}

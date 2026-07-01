use crate::audio::{AudioEngine, EqProcessorFactory, PlaybackState, SampleBlockProcessorFactory};
use crate::config::{
    self, AppConfig, AppPaths, DISCORD_MODULE_ID, EQ_MODULE_ID, LASTFM_MODULE_ID,
    LOCAL_SOURCE_MODULE_ID,
};
use crate::integrations::IntegrationManager;
use crate::library::{LocalMusicSource, format_duration, tracks_from_drop};
use crate::plugins::{DiscoveredModuleKind, PluginHost};
use crate::theme::{AlloyTheme, ThemeCatalog, ThemeColors, apply_theme};
use alloy_core::{ModuleCategory, ModuleEvent, MusicSourceModule, Track};
use eframe::egui;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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
}

impl AlloyApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let paths = AppPaths::discover()?;
        let mut config = AppConfig::load(&paths)?;
        config.discord = config.discord.clone().with_env();
        config::apply_cli_library_paths(&mut config);

        let themes = ThemeCatalog::load(&paths.themes_dir);
        let active_theme = themes
            .find(&config.active_theme)
            .unwrap_or_else(|| themes.default_theme());
        apply_theme(&cc.egui_ctx, active_theme);

        let plugins = PluginHost::load(&paths.modules_dir);
        let mut config_changed = false;
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
        self.play_next();
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

        let processor_factories = self.audio_processor_factories();
        let Some(audio) = &mut self.audio else {
            self.status_line = self
                .audio_error
                .clone()
                .unwrap_or_else(|| "Audio output is not available".to_owned());
            return;
        };

        match audio.play(track.clone(), processor_factories) {
            Ok(()) => {
                self.status_line = format!("Playing {}", track.title);
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
        let next = self
            .current_index()
            .map_or(0, |index| (index + 1) % self.library.len());
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

        let processor_factories = self.audio_processor_factories();
        let Some(audio) = &mut self.audio else {
            return;
        };
        if let Err(err) = audio.play_from(track, position, processor_factories) {
            self.status_line = format!("Could not apply EQ: {err}");
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

        let processor_factories = self.audio_processor_factories();
        let Some(audio) = &mut self.audio else {
            return;
        };
        if let Err(err) = audio.play_from(track, target, processor_factories) {
            self.status_line = format!("Seek failed: {err}");
        }
    }

    fn audio_processor_factories(&self) -> Vec<Box<dyn SampleBlockProcessorFactory>> {
        let mut factories: Vec<Box<dyn SampleBlockProcessorFactory>> = Vec::new();

        for node in &self.config.audio_chain {
            if !node.enabled || !self.config.module_enabled(&node.id) {
                continue;
            }

            if node.id == EQ_MODULE_ID {
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
        let mut processors = vec![(EQ_MODULE_ID.to_owned(), "Equalizer".to_owned())];
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
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.volume, 0.0..=1.5)
                    .text("Default volume")
                    .show_value(true),
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
        });
    }

    fn settings_audio_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Audio Chain");
        ui.label("Processors run from top to bottom when playback starts.");
        ui.add_space(8.0);
        self.audio_chain_ui(ui);

        ui.separator();
        self.equalizer_ui(ui);
    }

    fn audio_chain_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let chain = self.config.audio_chain.clone();
        for (index, node) in chain.iter().enumerate() {
            let label = self.processor_label(&node.id);
            ui.horizontal(|ui| {
                let mut enabled = node.enabled;
                if ui.checkbox(&mut enabled, "").changed() {
                    if let Some(config_node) = self.config.audio_chain.get_mut(index) {
                        config_node.enabled = enabled;
                        changed = true;
                    }
                }
                ui.label(label).on_hover_text(&node.id);
                if ui.button("Up").clicked() && index > 0 {
                    self.config.audio_chain.swap(index, index - 1);
                    changed = true;
                }
                if ui.button("Down").clicked() && index + 1 < self.config.audio_chain.len() {
                    self.config.audio_chain.swap(index, index + 1);
                    changed = true;
                }
                if node.id != EQ_MODULE_ID && ui.button("Remove").clicked() {
                    self.config.audio_chain.remove(index);
                    changed = true;
                }
            });
        }

        let available = self
            .available_processors()
            .into_iter()
            .filter(|(id, _)| !self.config.audio_chain.iter().any(|node| node.id == *id))
            .collect::<Vec<_>>();

        if !available.is_empty() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Available Processors").strong());
            for (id, name) in available {
                ui.horizontal(|ui| {
                    ui.label(name);
                    if ui.button("Add").clicked() {
                        self.config
                            .audio_chain
                            .push(config::AudioChainNodeConfig { id, enabled: true });
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
        if add_from_enter || ui.button("Add folder or file").clicked() {
            self.add_path_from_entry();
        }

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
        self.module_toggle_ui(ui, EQ_MODULE_ID, "Equalizer");
        self.module_toggle_ui(ui, LASTFM_MODULE_ID, "Last.fm");
        self.module_toggle_ui(ui, DISCORD_MODULE_ID, "Discord RPC");

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.label(egui::RichText::new(&self.status_line).color(colors.muted_text));
            if let Some(error) = &self.audio_error {
                ui.label(egui::RichText::new(error).color(colors.danger));
            }
        });
    }

    fn right_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let colors = self.active_colors();
        ui.add_space(4.0);
        self.now_playing_ui(ui, ctx, colors);
        ui.separator();
        self.equalizer_ui(ui);
        ui.separator();
        self.theme_ui(ui, ctx);
        ui.separator();
        self.integration_ui(ui, colors);
        ui.separator();
        self.community_modules_ui(ui, colors);
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

    fn equalizer_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("EQ").strong());
            ui.checkbox(&mut self.config.equalizer.enabled, "Enabled");
        });

        if !self.config.module_enabled(EQ_MODULE_ID) {
            ui.label("Equalizer module disabled");
            return;
        }

        let mut changed = false;
        changed |= ui
            .add(
                egui::Slider::new(&mut self.config.equalizer.preamp_db, -9.0..=9.0)
                    .text("Preamp")
                    .suffix(" dB"),
            )
            .changed();

        for band in &mut self.config.equalizer.bands {
            let label = format_band(band.frequency_hz);
            changed |= ui
                .add(
                    egui::Slider::new(&mut band.gain_db, -12.0..=12.0)
                        .text(label)
                        .suffix(" dB"),
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
        ui.label(egui::RichText::new("Theme").strong());
        let mut active = self.config.active_theme.clone();
        egui::ComboBox::from_id_salt("theme-select")
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
                ui.add_sized(
                    [260.0, 30.0],
                    egui::TextEdit::singleline(&mut self.search).hint_text("Search"),
                );
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
        egui::Grid::new("track-header")
            .num_columns(5)
            .spacing([16.0, 6.0])
            .striped(false)
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
            });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("track-grid")
                    .num_columns(5)
                    .spacing([16.0, 8.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for index in indices {
                            let (title, artist, album, duration, path, cover_path) = {
                                let track = &self.library[index];
                                let playing = self
                                    .audio
                                    .as_ref()
                                    .and_then(AudioEngine::current_track)
                                    .is_some_and(|current| current.id == track.id);
                                (
                                    if playing {
                                        format!("> {}", track.title)
                                    } else {
                                        track.title.clone()
                                    },
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

        ui.add_space(5.0);
        ui.horizontal_centered(|ui| {
            if transport_icon_button(ui, TransportIcon::Previous, "Previous track").clicked() {
                self.play_previous();
            }
            let play_icon = match state {
                PlaybackState::Playing => TransportIcon::Pause,
                PlaybackState::Paused | PlaybackState::Stopped => TransportIcon::Play,
            };
            let play_tip = match state {
                PlaybackState::Playing => "Pause",
                PlaybackState::Paused | PlaybackState::Stopped => "Play",
            };
            if transport_icon_button(ui, play_icon, play_tip).clicked() {
                self.toggle_playback();
            }
            if transport_icon_button(ui, TransportIcon::Next, "Next track").clicked() {
                self.play_next();
            }
            if transport_icon_button(ui, TransportIcon::Stop, "Stop").clicked() {
                self.stop();
            }

            ui.separator();
            ui.label(format_duration(Some(position)));
            let mut progress = duration.map_or(0.0, |duration| {
                if duration.is_zero() {
                    0.0
                } else {
                    position.as_secs_f32() / duration.as_secs_f32()
                }
            });
            let progress_response = ui.add_sized(
                [360.0, 22.0],
                egui::Slider::new(&mut progress, 0.0..=1.0).show_value(false),
            );
            if progress_response.drag_stopped() || progress_response.clicked() {
                self.seek_to_fraction(progress);
            }
            ui.label(format_duration(duration));

            ui.separator();
            ui.label("Volume");
            let mut new_volume = volume;
            if ui
                .add(egui::Slider::new(&mut new_volume, 0.0..=1.5).show_value(false))
                .changed()
            {
                self.config.volume = new_volume;
                if let Some(audio) = &mut self.audio {
                    audio.set_volume(new_volume);
                }
                self.save_config();
            }
        });
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
) -> egui::Response {
    let response = ui
        .add_sized([42.0, 38.0], egui::Button::new(""))
        .on_hover_text(tooltip);
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
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
    }
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

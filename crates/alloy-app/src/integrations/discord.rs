use crate::config::DiscordConfig;
use alloy_core::{
    IntegrationModule, ModuleCategory, ModuleError, ModuleEvent, ModuleManifest, Track,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};

pub struct DiscordIntegration {
    manifest: ModuleManifest,
    client_id: String,
    client: Option<DiscordIpcClient>,
}

impl DiscordIntegration {
    #[must_use]
    pub fn new(config: DiscordConfig) -> Option<Self> {
        let client_id = config.client_id?;
        if client_id.trim().is_empty() {
            return None;
        }

        Some(Self {
            manifest: ModuleManifest {
                enabled_by_default: false,
                capabilities: vec!["rich-presence".to_owned(), "playback-state".to_owned()],
                ..ModuleManifest::built_in(
                    "alloy.integrations.discord",
                    "Discord RPC",
                    env!("CARGO_PKG_VERSION"),
                    ModuleCategory::Integration,
                    "Publishes current playback state to Discord Rich Presence.",
                )
            },
            client_id,
            client: None,
        })
    }

    fn ensure_connected(&mut self) -> Result<(), ModuleError> {
        if self.client.is_some() {
            return Ok(());
        }

        let mut client = DiscordIpcClient::new(&self.client_id)
            .map_err(|err| ModuleError::new(err.to_string()))?;
        client
            .connect()
            .map_err(|err| ModuleError::new(err.to_string()))?;
        self.client = Some(client);
        Ok(())
    }

    fn set_track(&mut self, track: &Track, paused: bool) -> Result<(), ModuleError> {
        self.ensure_connected()?;
        let state = if paused {
            "Paused"
        } else {
            track.display_artist()
        };
        let details = track.title.as_str();
        let activity = activity::Activity::new()
            .details(details)
            .state(state)
            .assets(activity::Assets::new().large_text("Alloy"));

        if let Some(client) = &mut self.client {
            client
                .set_activity(activity)
                .map_err(|err| ModuleError::new(err.to_string()))?;
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<(), ModuleError> {
        if let Some(client) = &mut self.client {
            client
                .clear_activity()
                .map_err(|err| ModuleError::new(err.to_string()))?;
        }
        Ok(())
    }
}

impl alloy_core::AlloyModule for DiscordIntegration {
    fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }
}

impl IntegrationModule for DiscordIntegration {
    fn on_event(&mut self, event: &ModuleEvent) -> Result<(), ModuleError> {
        match event {
            ModuleEvent::PlaybackStarted { track }
            | ModuleEvent::PlaybackResumed { track: Some(track) } => {
                self.set_track(track, false)?;
            }
            ModuleEvent::PlaybackPaused { track: Some(track) } => {
                self.set_track(track, true)?;
            }
            ModuleEvent::PlaybackStopped | ModuleEvent::TrackFinished { .. } => {
                self.clear()?;
            }
            ModuleEvent::PlaybackPaused { track: None }
            | ModuleEvent::PlaybackResumed { track: None }
            | ModuleEvent::PositionChanged { .. } => {}
        }
        Ok(())
    }
}

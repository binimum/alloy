mod discord;
mod lastfm;

use crate::config::{AppConfig, DISCORD_MODULE_ID, DiscordConfig, LASTFM_MODULE_ID, LastFmConfig};
use alloy_core::{IntegrationModule, ModuleEvent};

pub use discord::DiscordIntegration;
pub use lastfm::LastFmIntegration;

#[derive(Debug, Clone)]
pub struct IntegrationStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub enabled: bool,
    pub configured: bool,
}

pub struct IntegrationManager {
    modules: Vec<Box<dyn IntegrationModule>>,
    statuses: Vec<IntegrationStatus>,
    last_error: Option<String>,
}

impl IntegrationManager {
    #[must_use]
    pub fn new(config: &AppConfig) -> Self {
        let lastfm_config = config.lastfm.clone();
        let discord_config = config.discord.clone().with_env();
        let mut modules: Vec<Box<dyn IntegrationModule>> = Vec::new();
        let mut statuses = vec![
            IntegrationStatus {
                id: LASTFM_MODULE_ID,
                name: "Last.fm",
                enabled: config.module_enabled(LASTFM_MODULE_ID),
                configured: lastfm_config.configured(),
            },
            IntegrationStatus {
                id: DISCORD_MODULE_ID,
                name: "Discord RPC",
                enabled: config.module_enabled(DISCORD_MODULE_ID),
                configured: discord_config
                    .client_id
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()),
            },
        ];

        if config.module_enabled(LASTFM_MODULE_ID) && lastfm_config.configured() {
            modules.push(Box::new(LastFmIntegration::new(lastfm_config)));
        }

        if config.module_enabled(DISCORD_MODULE_ID) {
            if let Some(module) = DiscordIntegration::new(discord_config) {
                modules.push(Box::new(module));
            }
        }

        statuses.sort_by(|a, b| a.name.cmp(b.name));
        Self {
            modules,
            statuses,
            last_error: None,
        }
    }

    pub fn rebuild(&mut self, config: &AppConfig) {
        *self = Self::new(config);
    }

    pub fn dispatch(&mut self, event: &ModuleEvent) {
        for module in &mut self.modules {
            if let Err(err) = module.on_event(event) {
                self.last_error = Some(format!("{}: {err}", module.manifest().name));
            }
        }
    }

    #[must_use]
    pub fn statuses(&self) -> &[IntegrationStatus] {
        &self.statuses
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}

#[allow(dead_code)]
fn _assert_configs_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<LastFmConfig>();
    assert_send::<DiscordConfig>();
}

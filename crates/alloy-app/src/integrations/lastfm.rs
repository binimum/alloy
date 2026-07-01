use crate::config::LastFmConfig;
use alloy_core::{
    IntegrationModule, ModuleCategory, ModuleError, ModuleEvent, ModuleManifest, Track,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct LastFmIntegration {
    manifest: ModuleManifest,
    config: LastFmConfig,
    current_id: Option<String>,
    started_at_unix: Option<u64>,
    scrobbled_id: Option<String>,
}

impl LastFmIntegration {
    #[must_use]
    pub fn new(config: LastFmConfig) -> Self {
        Self {
            manifest: ModuleManifest {
                enabled_by_default: false,
                capabilities: vec![
                    "now-playing".to_owned(),
                    "scrobble".to_owned(),
                    "session-key-auth".to_owned(),
                ],
                ..ModuleManifest::built_in(
                    "alloy.integrations.lastfm",
                    "Last.fm",
                    env!("CARGO_PKG_VERSION"),
                    ModuleCategory::Integration,
                    "Updates now playing state and scrobbles completed tracks.",
                )
            },
            config,
            current_id: None,
            started_at_unix: None,
            scrobbled_id: None,
        }
    }

    fn maybe_scrobble(&mut self, track: &Track, position: Duration) {
        if !self.config.scrobble || self.scrobbled_id.as_deref() == Some(track.id.as_str()) {
            return;
        }

        let Some(duration) = track.duration else {
            return;
        };
        let threshold = std::cmp::min(duration / 2, Duration::from_secs(240));
        if position < threshold || position < Duration::from_secs(30) {
            return;
        }

        self.scrobbled_id = Some(track.id.clone());
        send_scrobble(
            self.config.clone(),
            track.clone(),
            self.started_at_unix.unwrap_or_else(now_unix),
        );
    }
}

impl alloy_core::AlloyModule for LastFmIntegration {
    fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }
}

impl IntegrationModule for LastFmIntegration {
    fn on_event(&mut self, event: &ModuleEvent) -> Result<(), ModuleError> {
        match event {
            ModuleEvent::PlaybackStarted { track } => {
                self.current_id = Some(track.id.clone());
                self.started_at_unix = Some(now_unix());
                self.scrobbled_id = None;
                send_now_playing(self.config.clone(), track.clone());
            }
            ModuleEvent::PositionChanged {
                track: Some(track),
                position,
            } => self.maybe_scrobble(track, *position),
            ModuleEvent::PlaybackStopped | ModuleEvent::TrackFinished { .. } => {
                self.current_id = None;
                self.started_at_unix = None;
            }
            ModuleEvent::PlaybackPaused { .. }
            | ModuleEvent::PlaybackResumed { .. }
            | ModuleEvent::PositionChanged { track: None, .. } => {}
        }
        Ok(())
    }
}

fn send_now_playing(config: LastFmConfig, track: Track) {
    std::thread::spawn(move || {
        let mut params = base_params(&config, "track.updateNowPlaying");
        params.push(("artist".to_owned(), track.display_artist().to_owned()));
        params.push(("track".to_owned(), track.title));
        if let Some(duration) = track.duration {
            params.push(("duration".to_owned(), duration.as_secs().to_string()));
        }
        let _ = post_signed(config, params);
    });
}

fn send_scrobble(config: LastFmConfig, track: Track, timestamp: u64) {
    std::thread::spawn(move || {
        let mut params = base_params(&config, "track.scrobble");
        params.push(("artist".to_owned(), track.display_artist().to_owned()));
        params.push(("track".to_owned(), track.title));
        params.push(("timestamp".to_owned(), timestamp.to_string()));
        if let Some(duration) = track.duration {
            params.push(("duration".to_owned(), duration.as_secs().to_string()));
        }
        let _ = post_signed(config, params);
    });
}

fn base_params(config: &LastFmConfig, method: &str) -> Vec<(String, String)> {
    vec![
        ("method".to_owned(), method.to_owned()),
        (
            "api_key".to_owned(),
            config.api_key.clone().unwrap_or_default(),
        ),
        (
            "sk".to_owned(),
            config.session_key.clone().unwrap_or_default(),
        ),
    ]
}

fn post_signed(config: LastFmConfig, mut params: Vec<(String, String)>) -> anyhow::Result<()> {
    let secret = config.api_secret.unwrap_or_default();
    params.push(("api_sig".to_owned(), api_signature(&params, &secret)));
    params.push(("format".to_owned(), "json".to_owned()));

    let form = params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let _response = ureq::post("https://ws.audioscrobbler.com/2.0/").send_form(&form)?;
    Ok(())
}

fn api_signature(params: &[(String, String)], secret: &str) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut payload = String::new();
    for (key, value) in sorted {
        if key != "format" && key != "callback" {
            payload.push_str(&key);
            payload.push_str(&value);
        }
    }
    payload.push_str(secret);
    format!("{:x}", md5::compute(payload))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_order_independent() {
        let a = vec![
            ("method".to_owned(), "track.scrobble".to_owned()),
            ("track".to_owned(), "Rae".to_owned()),
            ("artist".to_owned(), "Autechre".to_owned()),
        ];
        let b = vec![
            ("artist".to_owned(), "Autechre".to_owned()),
            ("track".to_owned(), "Rae".to_owned()),
            ("method".to_owned(), "track.scrobble".to_owned()),
        ];

        assert_eq!(api_signature(&a, "secret"), api_signature(&b, "secret"));
    }
}

use alloy_core::{ModuleCategory, ModuleManifest, MusicSourceModule, Track};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const SUPPORTED_EXTENSIONS: &[&str] = &["flac", "mp3", "ogg", "opus", "wav", "m4a", "aac"];
const COVER_STEMS: &[&str] = &["cover", "folder", "front", "album", "artwork"];
const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];

pub struct LocalMusicSource {
    manifest: ModuleManifest,
}

impl Default for LocalMusicSource {
    fn default() -> Self {
        Self {
            manifest: ModuleManifest {
                capabilities: vec![
                    "recursive-folder-scan".to_owned(),
                    "drag-and-drop-files".to_owned(),
                    "local-playback".to_owned(),
                ],
                ..ModuleManifest::built_in(
                    "alloy.sources.local",
                    "Local Library",
                    env!("CARGO_PKG_VERSION"),
                    ModuleCategory::MusicSource,
                    "Scans local folders and files into the Alloy library.",
                )
            },
        }
    }
}

impl alloy_core::AlloyModule for LocalMusicSource {
    fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }
}

impl MusicSourceModule for LocalMusicSource {
    fn scan(&self, roots: &[PathBuf]) -> Result<Vec<Track>, alloy_core::ModuleError> {
        Ok(scan_roots(roots))
    }

    fn supports_path(&self, path: &Path) -> bool {
        supported_audio_path(path)
    }
}

pub fn scan_roots(roots: &[PathBuf]) -> Vec<Track> {
    let mut tracks = Vec::new();

    for root in roots {
        if root.is_file() {
            if let Some(track) = track_from_path(root) {
                tracks.push(track);
            }
            continue;
        }

        if root.is_dir() {
            for entry in WalkDir::new(root)
                .follow_links(true)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
            {
                if let Some(track) = track_from_path(entry.path()) {
                    tracks.push(track);
                }
            }
        }
    }

    tracks.sort_by(|a, b| {
        a.display_artist()
            .cmp(b.display_artist())
            .then_with(|| a.display_album().cmp(b.display_album()))
            .then_with(|| a.title.cmp(&b.title))
    });
    tracks.dedup_by(|a, b| a.path == b.path);
    tracks
}

pub fn tracks_from_drop(paths: &[PathBuf]) -> Vec<Track> {
    scan_roots(paths)
}

pub fn supported_audio_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| supported.eq_ignore_ascii_case(extension))
        })
}

fn track_from_path(path: &Path) -> Option<Track> {
    if !supported_audio_path(path) {
        return None;
    }

    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let stem = canonical
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Untitled");
    let (artist, title) = parse_stem(stem);

    Some(Track {
        id: stable_track_id(&canonical),
        title,
        artist,
        album: String::new(),
        path: canonical.clone(),
        cover_path: find_cover_path(&canonical),
        duration: read_duration(&canonical),
        source: "alloy.sources.local".to_owned(),
    })
}

fn parse_stem(stem: &str) -> (String, String) {
    if let Some((artist, title)) = stem.split_once(" - ") {
        (artist.trim().to_owned(), title.trim().to_owned())
    } else {
        (String::new(), stem.trim().to_owned())
    }
}

fn stable_track_id(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn find_cover_path(track_path: &Path) -> Option<PathBuf> {
    let directory = track_path.parent()?;
    let track_stem = track_path.file_stem().and_then(|stem| stem.to_str());

    for extension in COVER_EXTENSIONS {
        if let Some(stem) = track_stem {
            let candidate = directory.join(format!("{stem}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        for cover_stem in COVER_STEMS {
            let candidate = directory.join(format!("{cover_stem}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn read_duration(path: &Path) -> Option<std::time::Duration> {
    use rodio::{Decoder, Source};
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(path).ok()?;
    let decoder = Decoder::new(BufReader::new(file)).ok()?;
    decoder.total_duration()
}

pub fn format_duration(duration: Option<std::time::Duration>) -> String {
    let Some(duration) = duration else {
        return "--:--".to_owned();
    };
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_audio_extensions_case_insensitively() {
        assert!(supported_audio_path(Path::new("song.FLAC")));
        assert!(supported_audio_path(Path::new("song.mp3")));
        assert!(!supported_audio_path(Path::new("cover.jpg")));
    }

    #[test]
    fn parses_artist_title_file_stem() {
        let (artist, title) = parse_stem("Autechre - Rae");

        assert_eq!(artist, "Autechre");
        assert_eq!(title, "Rae");
    }

    #[test]
    fn finds_sidecar_cover_art() {
        let temp = tempfile::tempdir().expect("tempdir");
        let track = temp.path().join("song.mp3");
        let cover = temp.path().join("cover.jpg");
        std::fs::write(&track, []).expect("track");
        std::fs::write(&cover, []).expect("cover");

        assert_eq!(find_cover_path(&track), Some(cover));
    }
}

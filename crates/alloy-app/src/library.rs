use alloy_core::{ModuleCategory, ModuleManifest, MusicSourceModule, ReplayGain, Track};
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const SUPPORTED_EXTENSIONS: &[&str] = &["flac", "mp3", "ogg", "opus", "wav", "m4a", "aac"];
const COVER_STEMS: &[&str] = &["cover", "folder", "front", "album", "artwork"];
const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
const METADATA_READ_LIMIT: u64 = 512 * 1024;
const COVER_READ_LIMIT: u64 = 8 * 1024 * 1024;

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
    let metadata = read_track_metadata(&canonical);

    Some(Track {
        id: stable_track_id(&canonical),
        title: metadata.title.unwrap_or(title),
        artist: metadata.artist.unwrap_or(artist),
        album: metadata.album.unwrap_or_default(),
        path: canonical.clone(),
        cover_path: find_cover_path(&canonical),
        replay_gain: metadata.replay_gain,
        duration: read_duration(&canonical),
        source: "alloy.sources.local".to_owned(),
    })
}

#[derive(Debug, Default)]
struct TrackMetadata {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    replay_gain: ReplayGain,
}

fn read_track_metadata(path: &Path) -> TrackMetadata {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();

    let comments = if extension.eq_ignore_ascii_case("flac") {
        read_flac_vorbis_comments(path)
    } else if extension.eq_ignore_ascii_case("ogg") || extension.eq_ignore_ascii_case("opus") {
        read_ogg_vorbis_comments(path)
    } else {
        None
    };

    comments.map_or_else(TrackMetadata::default, comments_to_metadata)
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    read_limited_to(path, METADATA_READ_LIMIT)
}

fn read_limited_to(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = file.take(limit);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn read_flac_vorbis_comments(path: &Path) -> Option<BTreeMap<String, String>> {
    let bytes = read_limited(path)?;
    if !bytes.starts_with(b"fLaC") {
        return None;
    }

    for block in flac_metadata_blocks(&bytes) {
        if block.block_type == 4 {
            return parse_vorbis_comment_block(block.payload);
        }
    }

    None
}

pub fn read_embedded_cover(path: &Path) -> Option<Vec<u8>> {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("flac"))
    {
        return None;
    }

    let bytes = read_limited_to(path, COVER_READ_LIMIT)?;
    if !bytes.starts_with(b"fLaC") {
        return None;
    }

    flac_metadata_blocks(&bytes)
        .find(|block| block.block_type == 6)
        .and_then(|block| parse_flac_picture(block.payload))
}

#[derive(Debug, Clone, Copy)]
struct FlacMetadataBlock<'a> {
    block_type: u8,
    payload: &'a [u8],
}

fn flac_metadata_blocks(bytes: &[u8]) -> impl Iterator<Item = FlacMetadataBlock<'_>> {
    let mut offset = 4;
    let mut done = false;
    std::iter::from_fn(move || {
        if done || offset + 4 > bytes.len() {
            return None;
        }

        let header = bytes.get(offset..offset + 4)?;
        done = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7f;
        let length =
            (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        offset += 4;
        let payload = bytes.get(offset..offset + length)?;
        offset += length;

        Some(FlacMetadataBlock {
            block_type,
            payload,
        })
    })
}

fn parse_flac_picture(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut offset = 0;
    let _picture_type = read_be_u32(bytes, &mut offset)?;
    let mime_length = usize::try_from(read_be_u32(bytes, &mut offset)?).ok()?;
    let mime = std::str::from_utf8(bytes.get(offset..offset + mime_length)?).ok()?;
    offset += mime_length;
    if !matches!(mime, "image/jpeg" | "image/jpg" | "image/png") {
        return None;
    }

    let description_length = usize::try_from(read_be_u32(bytes, &mut offset)?).ok()?;
    offset = offset.checked_add(description_length)?;
    let _width = read_be_u32(bytes, &mut offset)?;
    let _height = read_be_u32(bytes, &mut offset)?;
    let _depth = read_be_u32(bytes, &mut offset)?;
    let _colors = read_be_u32(bytes, &mut offset)?;
    let data_length = usize::try_from(read_be_u32(bytes, &mut offset)?).ok()?;
    Some(bytes.get(offset..offset + data_length)?.to_vec())
}

fn read_ogg_vorbis_comments(path: &Path) -> Option<BTreeMap<String, String>> {
    let bytes = read_limited(path)?;
    if let Some(position) = find_subslice(&bytes, b"\x03vorbis") {
        return parse_vorbis_comment_block(bytes.get(position + 7..)?);
    }
    if let Some(position) = find_subslice(&bytes, b"OpusTags") {
        return parse_vorbis_comment_block(bytes.get(position + 8..)?);
    }
    None
}

fn parse_vorbis_comment_block(bytes: &[u8]) -> Option<BTreeMap<String, String>> {
    let mut offset = 0;
    let vendor_length = read_le_u32(bytes, &mut offset)?;
    offset = offset.checked_add(usize::try_from(vendor_length).ok()?)?;
    if offset > bytes.len() {
        return None;
    }

    let comment_count = read_le_u32(bytes, &mut offset)?;
    let mut comments = BTreeMap::new();
    for _ in 0..comment_count {
        let length = usize::try_from(read_le_u32(bytes, &mut offset)?).ok()?;
        let value = bytes.get(offset..offset + length)?;
        offset += length;
        let value = std::str::from_utf8(value).ok()?;
        let Some((key, value)) = value.split_once('=') else {
            continue;
        };
        comments
            .entry(key.trim().to_uppercase())
            .or_insert_with(|| value.trim().to_owned());
    }

    Some(comments)
}

fn comments_to_metadata(comments: BTreeMap<String, String>) -> TrackMetadata {
    TrackMetadata {
        title: non_empty_comment(&comments, "TITLE"),
        artist: non_empty_comment(&comments, "ARTIST")
            .or_else(|| non_empty_comment(&comments, "ALBUMARTIST")),
        album: non_empty_comment(&comments, "ALBUM"),
        replay_gain: ReplayGain {
            track_gain_db: parse_gain_db(comments.get("REPLAYGAIN_TRACK_GAIN")),
            album_gain_db: parse_gain_db(comments.get("REPLAYGAIN_ALBUM_GAIN")),
            track_peak: parse_peak(comments.get("REPLAYGAIN_TRACK_PEAK")),
            album_peak: parse_peak(comments.get("REPLAYGAIN_ALBUM_PEAK")),
        },
    }
}

fn non_empty_comment(comments: &BTreeMap<String, String>, key: &str) -> Option<String> {
    comments.get(key).and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    })
}

fn parse_gain_db(value: Option<&String>) -> Option<f32> {
    value?
        .split_whitespace()
        .next()
        .and_then(|number| number.parse::<f32>().ok())
}

fn parse_peak(value: Option<&String>) -> Option<f32> {
    value?.trim().parse::<f32>().ok()
}

fn read_le_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let value = bytes.get(*offset..*offset + 4)?;
    *offset += 4;
    Some(u32::from_le_bytes(value.try_into().ok()?))
}

fn read_be_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let value = bytes.get(*offset..*offset + 4)?;
    *offset += 4;
    Some(u32::from_be_bytes(value.try_into().ok()?))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

    #[test]
    fn parses_vorbis_comments_for_ogg_and_flac_metadata() {
        let comments = parse_vorbis_comment_block(&vorbis_comment_block(&[
            ("TITLE", "Rae"),
            ("ARTIST", "Autechre"),
            ("ALBUM", "LP5"),
            ("REPLAYGAIN_TRACK_GAIN", "-6.40 dB"),
            ("REPLAYGAIN_TRACK_PEAK", "0.9412"),
        ]))
        .expect("comments");
        let metadata = comments_to_metadata(comments);

        assert_eq!(metadata.title.as_deref(), Some("Rae"));
        assert_eq!(metadata.artist.as_deref(), Some("Autechre"));
        assert_eq!(metadata.album.as_deref(), Some("LP5"));
        assert_eq!(metadata.replay_gain.track_gain_db, Some(-6.4));
        assert_eq!(metadata.replay_gain.track_peak, Some(0.9412));
    }

    #[test]
    fn parses_flac_picture_block_data() {
        let picture_bytes = [0x89, b'P', b'N', b'G'];
        let block = flac_picture_block("image/png", &picture_bytes);

        assert_eq!(parse_flac_picture(&block), Some(picture_bytes.to_vec()));
    }

    fn vorbis_comment_block(comments: &[(&str, &str)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for (key, value) in comments {
            let comment = format!("{key}={value}");
            bytes.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            bytes.extend_from_slice(comment.as_bytes());
        }
        bytes
    }

    fn flac_picture_block(mime: &str, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&3_u32.to_be_bytes());
        bytes.extend_from_slice(&(mime.len() as u32).to_be_bytes());
        bytes.extend_from_slice(mime.as_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&300_u32.to_be_bytes());
        bytes.extend_from_slice(&300_u32.to_be_bytes());
        bytes.extend_from_slice(&24_u32.to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(data);
        bytes
    }
}

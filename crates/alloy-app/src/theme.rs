use alloy_core::{ModuleCategory, ModuleManifest, ThemeDefinition, ThemeMode};
use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle, Visuals};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AlloyTheme {
    pub manifest: ModuleManifest,
    pub definition: ThemeDefinition,
}

#[derive(Debug, Clone)]
pub struct ThemeCatalog {
    themes: Vec<AlloyTheme>,
}

impl ThemeCatalog {
    pub fn load(theme_dir: &Path) -> Self {
        let mut themes = built_in_themes();
        themes.extend(load_external_themes(theme_dir));
        Self { themes }
    }

    #[must_use]
    pub fn all(&self) -> &[AlloyTheme] {
        &self.themes
    }

    #[must_use]
    pub fn find(&self, id: &str) -> Option<&AlloyTheme> {
        self.themes.iter().find(|theme| theme.definition.id == id)
    }

    #[must_use]
    pub fn default_theme(&self) -> &AlloyTheme {
        self.find("graphite")
            .or_else(|| self.themes.first())
            .expect("built-in themes are always present")
    }

    #[must_use]
    pub fn default_theme_for_mode(&self, dark_mode: bool) -> &AlloyTheme {
        let id = if dark_mode { "graphite" } else { "linen" };
        self.find(id).unwrap_or_else(|| self.default_theme())
    }
}

pub fn apply_theme(ctx: &egui::Context, theme: &AlloyTheme) {
    let colors = ThemeColors::from_definition(&theme.definition);
    let dark_mode = matches!(theme.definition.mode, ThemeMode::Dark);
    let mut visuals = match theme.definition.mode {
        ThemeMode::Dark => Visuals::dark(),
        ThemeMode::Light => Visuals::light(),
    };

    let hairline = hairline_stroke(dark_mode);
    let quiet_hairline = quiet_hairline_stroke(dark_mode);

    visuals.panel_fill = colors.background;
    visuals.window_fill = colors.panel;
    visuals.extreme_bg_color = colors.background;
    visuals.faint_bg_color = colors.surface_muted;
    visuals.text_edit_bg_color = Some(colors.surface);
    visuals.code_bg_color = colors.surface_muted;
    visuals.warn_fg_color = colors.accent;
    visuals.error_fg_color = colors.danger;
    visuals.weak_text_color = Some(colors.muted_text);

    visuals.widgets.noninteractive.bg_fill = colors.background;
    visuals.widgets.noninteractive.weak_bg_fill = colors.surface;
    visuals.widgets.noninteractive.bg_stroke = quiet_hairline;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, colors.text);

    visuals.widgets.inactive.bg_fill = colors.surface;
    visuals.widgets.inactive.weak_bg_fill = colors.panel;
    visuals.widgets.inactive.bg_stroke = quiet_hairline;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, colors.text);

    visuals.widgets.hovered.bg_fill = colors.hover;
    visuals.widgets.hovered.weak_bg_fill = colors.hover;
    visuals.widgets.hovered.bg_stroke = hairline;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, colors.text);

    visuals.widgets.active.bg_fill = colors.accent;
    visuals.widgets.active.weak_bg_fill = colors.accent;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, colors.accent);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, colors.accent_text);
    visuals.widgets.open = visuals.widgets.hovered;

    visuals.selection.bg_fill = with_alpha(colors.accent, if dark_mode { 76 } else { 56 });
    visuals.selection.stroke = egui::Stroke::new(1.0, colors.accent);
    visuals.hyperlink_color = colors.accent;
    visuals.override_text_color = Some(colors.text);
    visuals.window_stroke = hairline;
    visuals.window_shadow = window_shadow(dark_mode);
    visuals.popup_shadow = popup_shadow(dark_mode);
    visuals.button_frame = true;
    visuals.collapsing_header_frame = false;
    visuals.indent_has_left_vline = false;
    visuals.striped = false;
    visuals.slider_trailing_fill = true;
    visuals.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.42 };
    visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    visuals.disabled_alpha = 0.48;

    let mut style = egui::Style::default();
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.window_margin = egui::Margin::symmetric(14, 12);
    style.spacing.menu_margin = egui::Margin::symmetric(10, 8);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.slider_width = 170.0;
    style.spacing.slider_rail_height = 4.0;
    style.spacing.combo_width = 150.0;
    style.spacing.text_edit_width = 220.0;
    style.spacing.interact_size = egui::vec2(38.0, 32.0);
    style.spacing.icon_width = 16.0;
    style.spacing.icon_width_inner = 9.0;
    style.spacing.icon_spacing = 8.0;
    apply_type_scale(&mut style);
    for radius in [
        &mut style.visuals.widgets.inactive.corner_radius,
        &mut style.visuals.widgets.hovered.corner_radius,
        &mut style.visuals.widgets.active.corner_radius,
        &mut style.visuals.widgets.open.corner_radius,
        &mut style.visuals.widgets.noninteractive.corner_radius,
        &mut style.visuals.window_corner_radius,
        &mut style.visuals.menu_corner_radius,
    ] {
        *radius = egui::CornerRadius::same(8);
    }

    ctx.set_style(style);
}

pub fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let default_stack = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let mut body_stack = Vec::new();
    if let Some(bytes) = load_system_font() {
        fonts
            .font_data
            .insert("system-ui".to_owned(), FontData::from_owned(bytes).into());
        body_stack.push("system-ui".to_owned());
    }
    body_stack.extend(default_stack.clone());
    fonts
        .families
        .insert(FontFamily::Proportional, body_stack.clone());

    let mut heading_stack = Vec::new();
    if !BUNDLED_FUNNEL_SANS.is_empty() {
        fonts.font_data.insert(
            "funnel-sans".to_owned(),
            FontData::from_static(BUNDLED_FUNNEL_SANS).into(),
        );
        heading_stack.push("funnel-sans".to_owned());
    } else if let Some(bytes) = load_first_font(FUNNEL_SANS_FONT_CANDIDATES) {
        fonts
            .font_data
            .insert("funnel-sans".to_owned(), FontData::from_owned(bytes).into());
        heading_stack.push("funnel-sans".to_owned());
    }

    heading_stack.extend(body_stack);
    fonts.families.insert(heading_family(), heading_stack);
    ctx.set_fonts(fonts);
}

fn apply_type_scale(style: &mut egui::Style) {
    style.text_styles = [
        (TextStyle::Heading, FontId::new(23.0, heading_family())),
        (
            TextStyle::Name("section".into()),
            FontId::new(18.0, heading_family()),
        ),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
    ]
    .into();
}

fn heading_family() -> FontFamily {
    FontFamily::Name("alloy-heading".into())
}

fn load_first_font(candidates: &[&str]) -> Option<Vec<u8>> {
    candidates.iter().find_map(|path| std::fs::read(path).ok())
}

fn load_system_font() -> Option<Vec<u8>> {
    load_first_font(platform_system_font_candidates()).or_else(load_fontconfig_system_font)
}

#[cfg(target_os = "linux")]
fn load_fontconfig_system_font() -> Option<Vec<u8>> {
    ["ui-sans-serif", "system-ui", "sans-serif"]
        .iter()
        .filter_map(|family| fontconfig_match(family))
        .find(|path| use_fontconfig_body_font(path))
        .and_then(|path| std::fs::read(path).ok())
}

#[cfg(not(target_os = "linux"))]
fn load_fontconfig_system_font() -> Option<Vec<u8>> {
    None
}

#[cfg(target_os = "linux")]
fn fontconfig_match(family: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", family])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(path.trim());
    path.is_file().then_some(path)
}

#[cfg(target_os = "linux")]
fn use_fontconfig_body_font(path: &Path) -> bool {
    let name = path.to_string_lossy().to_lowercase();
    !["ubuntu", "dejavu", "liberation"]
        .iter()
        .any(|font| name.contains(font))
}

const BUNDLED_FUNNEL_SANS: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/funnel_sans_font.bin"));

const FUNNEL_SANS_FONT_CANDIDATES: &[&str] = &[
    concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/FunnelSans.ttf"),
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/fonts/FunnelSans-VariableFont_wght.ttf"
    ),
    "/usr/share/fonts/truetype/funnel-sans/FunnelSans.ttf",
    "/usr/share/fonts/truetype/funnelsans/FunnelSans.ttf",
    "/Library/Fonts/Funnel Sans.ttf",
    "C:\\Windows\\Fonts\\FunnelSans.ttf",
];

#[cfg(target_os = "macos")]
fn platform_system_font_candidates() -> &'static [&'static str] {
    &[
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSDisplay.ttf",
        "/System/Library/Fonts/Supplemental/SF Pro.ttf",
        "/System/Library/Fonts/Supplemental/SF Pro Text.ttf",
        "/System/Library/Fonts/Supplemental/SF Pro Display.ttf",
    ]
}

#[cfg(target_os = "windows")]
fn platform_system_font_candidates() -> &'static [&'static str] {
    &[
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\segoeuisl.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
    ]
}

#[cfg(target_os = "linux")]
fn platform_system_font_candidates() -> &'static [&'static str] {
    &[
        "/usr/share/fonts/truetype/adwaita/AdwaitaSans-Regular.ttf",
        "/usr/share/fonts/adwaita/AdwaitaSans-Regular.ttf",
        "/usr/share/fonts/opentype/cantarell/Cantarell-VF.otf",
        "/usr/share/fonts/truetype/cantarell/Cantarell-VF.otf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/opentype/noto/NotoSans-Regular.ttf",
    ]
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_system_font_candidates() -> &'static [&'static str] {
    &[]
}

fn hairline_stroke(dark_mode: bool) -> egui::Stroke {
    egui::Stroke::new(
        1.0,
        if dark_mode {
            Color32::from_white_alpha(28)
        } else {
            Color32::from_black_alpha(24)
        },
    )
}

fn quiet_hairline_stroke(dark_mode: bool) -> egui::Stroke {
    egui::Stroke::new(
        1.0,
        if dark_mode {
            Color32::from_white_alpha(16)
        } else {
            Color32::from_black_alpha(18)
        },
    )
}

fn window_shadow(dark_mode: bool) -> egui::Shadow {
    if dark_mode {
        egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(80),
        }
    } else {
        egui::Shadow {
            offset: [0, 10],
            blur: 26,
            spread: 0,
            color: Color32::from_black_alpha(28),
        }
    }
}

fn popup_shadow(dark_mode: bool) -> egui::Shadow {
    if dark_mode {
        egui::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(86),
        }
    } else {
        egui::Shadow {
            offset: [0, 8],
            blur: 22,
            spread: 0,
            color: Color32::from_black_alpha(34),
        }
    }
}

fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub background: Color32,
    pub surface: Color32,
    pub surface_muted: Color32,
    pub panel: Color32,
    pub text: Color32,
    pub muted_text: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub hover: Color32,
    pub danger: Color32,
}

impl ThemeColors {
    #[must_use]
    pub fn from_definition(definition: &ThemeDefinition) -> Self {
        Self {
            background: color(definition, "background", "#111318"),
            surface: color(definition, "surface", "#171a21"),
            surface_muted: color(definition, "surface-muted", "#20242d"),
            panel: color(definition, "panel", "#1b1f27"),
            text: color(definition, "text", "#f2f4f8"),
            muted_text: color(definition, "muted-text", "#aeb6c4"),
            accent: color(definition, "accent", "#6fd3c5"),
            accent_text: color(definition, "accent-text", "#061412"),
            hover: color(definition, "hover", "#29313c"),
            danger: color(definition, "danger", "#f07178"),
        }
    }
}

fn built_in_themes() -> Vec<AlloyTheme> {
    vec![
        built_in_theme(
            "graphite",
            "Graphite",
            ThemeMode::Dark,
            [
                ("background", "#101216"),
                ("surface", "#161a20"),
                ("surface-muted", "#20262e"),
                ("panel", "#1b2028"),
                ("text", "#f2f4f8"),
                ("muted-text", "#a9b2bf"),
                ("accent", "#77d5c7"),
                ("accent-text", "#061412"),
                ("hover", "#2a323c"),
                ("danger", "#ef6f75"),
            ],
        ),
        built_in_theme(
            "linen",
            "Linen",
            ThemeMode::Light,
            [
                ("background", "#f4f0e8"),
                ("surface", "#fffaf0"),
                ("surface-muted", "#e8e1d4"),
                ("panel", "#f9f5ec"),
                ("text", "#20242a"),
                ("muted-text", "#68727d"),
                ("accent", "#256f7a"),
                ("accent-text", "#f6fbfb"),
                ("hover", "#dce7e4"),
                ("danger", "#b33b45"),
            ],
        ),
        built_in_theme(
            "signal",
            "Signal",
            ThemeMode::Dark,
            [
                ("background", "#111113"),
                ("surface", "#18181b"),
                ("surface-muted", "#24242a"),
                ("panel", "#1f2026"),
                ("text", "#f7f2e8"),
                ("muted-text", "#b4b0aa"),
                ("accent", "#e7bf54"),
                ("accent-text", "#17130a"),
                ("hover", "#313036"),
                ("danger", "#ff6b62"),
            ],
        ),
    ]
}

fn built_in_theme(
    id: &str,
    name: &str,
    mode: ThemeMode,
    colors: impl IntoIterator<Item = (&'static str, &'static str)>,
) -> AlloyTheme {
    let definition = ThemeDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        mode,
        colors: colors
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect(),
    };

    AlloyTheme {
        manifest: ModuleManifest {
            capabilities: vec!["egui-visuals".to_owned()],
            ..ModuleManifest::built_in(
                format!("alloy.themes.{id}"),
                name,
                env!("CARGO_PKG_VERSION"),
                ModuleCategory::Theme,
                format!("{name} color theme for Alloy."),
            )
        },
        definition,
    }
}

fn load_external_themes(theme_dir: &Path) -> Vec<AlloyTheme> {
    let Ok(entries) = std::fs::read_dir(theme_dir) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .filter_map(|path| load_external_theme(&path).ok())
        .collect()
}

fn load_external_theme(path: &Path) -> anyhow::Result<AlloyTheme> {
    let raw = std::fs::read_to_string(path)?;
    let file: ExternalThemeFile = toml::from_str(&raw)?;
    let definition = ThemeDefinition {
        id: file.id.clone(),
        name: file.name.clone(),
        mode: file.mode,
        colors: file.colors,
    };
    Ok(AlloyTheme {
        manifest: ModuleManifest {
            authors: file.authors,
            capabilities: vec!["egui-visuals".to_owned(), "community-theme".to_owned()],
            ..ModuleManifest::built_in(
                format!("community.themes.{}", file.id),
                file.name,
                file.version.unwrap_or_else(|| "0.1.0".to_owned()),
                ModuleCategory::Theme,
                file.description
                    .unwrap_or_else(|| "Community theme module.".to_owned()),
            )
        },
        definition,
    })
}

#[derive(Debug, Deserialize)]
struct ExternalThemeFile {
    id: String,
    name: String,
    #[serde(default = "default_theme_version")]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    mode: ThemeMode,
    colors: BTreeMap<String, String>,
}

fn default_theme_version() -> Option<String> {
    Some("0.1.0".to_owned())
}

fn color(definition: &ThemeDefinition, key: &str, fallback: &str) -> Color32 {
    definition
        .colors
        .get(key)
        .and_then(|value| parse_hex(value))
        .or_else(|| parse_hex(fallback))
        .unwrap_or(Color32::WHITE)
}

fn parse_hex(value: &str) -> Option<Color32> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return None;
    }

    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color32::from_rgb(red, green, blue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_hex_color() {
        assert_eq!(parse_hex("#77d5c7"), Some(Color32::from_rgb(119, 213, 199)));
    }

    #[test]
    fn built_in_catalog_has_default_theme() {
        let catalog = ThemeCatalog {
            themes: built_in_themes(),
        };

        assert_eq!(catalog.default_theme().definition.id, "graphite");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fontconfig_body_filter_rejects_distribution_fonts() {
        assert!(!use_fontconfig_body_font(Path::new(
            "/usr/share/fonts/truetype/ubuntu/UbuntuSans.ttf"
        )));
        assert!(!use_fontconfig_body_font(Path::new(
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
        )));
        assert!(use_fontconfig_body_font(Path::new(
            "/usr/share/fonts/truetype/adwaita/AdwaitaSans-Regular.ttf"
        )));
    }
}

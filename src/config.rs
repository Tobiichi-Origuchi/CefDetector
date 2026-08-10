use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

const CONFIG_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(String);

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbaColor([u8; 4]);

impl RgbaColor {
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self([red, green, blue, 255])
    }

    pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self([red, green, blue, alpha])
    }

    fn parse(value: &str) -> Result<Self, ConfigError> {
        let digits = value
            .strip_prefix('#')
            .ok_or_else(|| ConfigError::new("colors must start with '#'"))?;
        if digits.len() != 6 && digits.len() != 8 {
            return Err(ConfigError::new("colors must use #RRGGBB or #RRGGBBAA"));
        }

        let mut channels = [0_u8; 4];
        channels[3] = 255;
        for (index, pair) in digits.as_bytes().chunks_exact(2).enumerate() {
            let pair = std::str::from_utf8(pair).expect("hex color pairs are ASCII boundaries");
            channels[index] = u8::from_str_radix(pair, 16)
                .map_err(|_| ConfigError::new(format!("invalid color {value:?}")))?;
        }
        Ok(Self(channels))
    }
}

impl fmt::Display for RgbaColor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [red, green, blue, alpha] = self.0;
        if alpha == 255 {
            write!(formatter, "#{red:02X}{green:02X}{blue:02X}")
        } else {
            write!(formatter, "#{red:02X}{green:02X}{blue:02X}{alpha:02X}")
        }
    }
}

impl Serialize for RgbaColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RgbaColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum $name {
            $($variant),+
        }
    };
}

string_enum!(SearchBackend {
    Auto,
    Index,
    Filesystem,
});
string_enum!(BackgroundFit {
    Cover,
    Contain,
    Stretch,
});
string_enum!(TextureFilter { Linear, Nearest });
string_enum!(HorizontalAlign {
    Left,
    Center,
    Right,
});
string_enum!(SortKey {
    Size,
    Name,
    Type,
    Path,
    Running,
});
string_enum!(SortOrder {
    Ascending,
    Descending,
});
string_enum!(ScrollbarMode {
    Auto,
    Always,
    Hidden,
});
string_enum!(ClickAction { Reveal, Open, None });
string_enum!(FontMode {
    Embedded,
    System,
    Custom,
});
string_enum!(GraphicsApi { Auto, Opengl, Gles });
string_enum!(LinuxDisplay { Auto, Egl, Glx });
string_enum!(CardField {
    Filename,
    Type,
    Size,
});
string_enum!(IconSource {
    Bundle,
    Pe,
    Appimage,
    Neighbor,
    Desktop,
    PackageManager,
    Builtin,
});

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub version: u32,
    pub search: SearchConfig,
    pub gui: GuiConfig,
    pub icons: IconConfig,
    pub cli: CliConfig,
    pub diagnostics: DiagnosticsConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            search: SearchConfig::default(),
            gui: GuiConfig::default(),
            icons: IconConfig::default(),
            cli: CliConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    pub backend: SearchBackend,
    pub index_fallback: bool,
    pub roots: Option<Vec<PathBuf>>,
    pub exclude_paths: Vec<PathBuf>,
    pub exclude_directory_names: Vec<String>,
    pub use_platform_excludes: bool,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    pub follow_symlinks: bool,
    pub same_filesystem: bool,
    pub walk_threads: usize,
    pub size_threads: usize,
    pub include_trash: bool,
    pub legacy_ignore_file: bool,
    pub plocate: PlocateConfig,
    pub everything: EverythingConfig,
    pub spotlight: SpotlightConfig,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            backend: SearchBackend::Auto,
            index_fallback: true,
            roots: None,
            exclude_paths: Vec::new(),
            exclude_directory_names: Vec::new(),
            use_platform_excludes: true,
            include_hidden: true,
            respect_gitignore: false,
            follow_symlinks: false,
            same_filesystem: false,
            walk_threads: 0,
            size_threads: 0,
            include_trash: false,
            legacy_ignore_file: true,
            plocate: PlocateConfig::default(),
            everything: EverythingConfig::default(),
            spotlight: SpotlightConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlocateConfig {
    pub command: PathBuf,
    pub timeout_ms: u64,
}

impl Default for PlocateConfig {
    fn default() -> Self {
        Self {
            command: "plocate".into(),
            timeout_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct EverythingConfig {
    pub send_timeout_ms: u32,
    pub reply_timeout_ms: u32,
}

impl Default for EverythingConfig {
    fn default() -> Self {
        Self {
            send_timeout_ms: 5_000,
            reply_timeout_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpotlightConfig {
    pub command: PathBuf,
    pub timeout_ms: u64,
}

impl Default for SpotlightConfig {
    fn default() -> Self {
        Self {
            command: "/usr/bin/mdfind".into(),
            timeout_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GuiConfig {
    pub window: WindowConfig,
    pub background: BackgroundConfig,
    pub status: StatusConfig,
    pub grid: GridConfig,
    pub card: CardConfig,
    pub scrolling: ScrollingConfig,
    pub scrollbar: ScrollbarConfig,
    pub size_format: SizeFormatConfig,
    pub fonts: FontConfig,
    pub footer: FooterConfig,
    pub progress: ProgressConfig,
    pub graphics: GraphicsConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub resizable: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "CEF Detector".into(),
            width: 800.0,
            height: 600.0,
            resizable: true,
            maximized: false,
            fullscreen: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackgroundConfig {
    pub path: Option<PathBuf>,
    pub fit: BackgroundFit,
    pub fallback_color: RgbaColor,
    pub filter: TextureFilter,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            path: None,
            fit: BackgroundFit::Cover,
            fallback_color: RgbaColor::rgb(0, 0, 0),
            filter: TextureFilter::Linear,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatusConfig {
    pub visible: bool,
    pub x: f32,
    pub y: f32,
    pub horizontal_align: HorizontalAlign,
    pub font_size: f32,
    pub dynamic_font_size: bool,
    pub min_font_size: f32,
    pub max_font_size: f32,
    pub searching_color: RgbaColor,
    pub success_color: RgbaColor,
    pub error_color: RgbaColor,
    pub searching_text: String,
    pub progress_text: String,
    pub success_text: String,
    pub empty_text: String,
    pub error_text: String,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            visible: true,
            x: 0.5,
            y: 0.21,
            horizontal_align: HorizontalAlign::Center,
            font_size: 18.0,
            dynamic_font_size: true,
            min_font_size: 12.0,
            max_font_size: 64.0,
            searching_color: RgbaColor::rgb(255, 255, 255),
            success_color: RgbaColor::rgb(33, 150, 243),
            error_color: RgbaColor::rgb(244, 67, 54),
            searching_text: "正在全盘搜索 CEF 应用，请耐心等待...".into(),
            progress_text: "这台电脑上已找到 {count} 个 Chromium 内核的应用 ({size}) - 搜索中..."
                .into(),
            success_text: "搜索完成！这台电脑上总共有 {count} 个 Chromium 内核的应用 ({size})"
                .into(),
            empty_text: "搜索完成！这台电脑上没有 Chromium 内核的应用".into(),
            error_text: "搜索失败：{error}".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GridConfig {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    pub min_columns: usize,
    pub sort_by: SortKey,
    pub sort_order: SortOrder,
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            x: 0.10,
            y: 0.30,
            width: 0.80,
            height: 0.60,
            cell_width: 106.0,
            cell_height: 128.0,
            min_columns: 1,
            sort_by: SortKey::Size,
            sort_order: SortOrder::Descending,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CardConfig {
    pub width: f32,
    pub height: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub corner_radius: f32,
    pub border_width: f32,
    pub background: RgbaColor,
    pub background_hover: RgbaColor,
    pub border_color: RgbaColor,
    pub border_color_hover: RgbaColor,
    pub icon_visible: bool,
    pub icon_width: f32,
    pub icon_height: f32,
    pub filename_visible: bool,
    pub filename_font_size: f32,
    pub filename_bold: bool,
    pub filename_max_width: f32,
    pub type_visible: bool,
    pub type_font_size: f32,
    pub size_visible: bool,
    pub size_font_size: f32,
    pub size_color: RgbaColor,
    pub normal_text_color: RgbaColor,
    pub running_text_color: RgbaColor,
    pub text_gap: f32,
    pub fields: Vec<CardField>,
    pub click_action: ClickAction,
}

impl Default for CardConfig {
    fn default() -> Self {
        Self {
            width: 94.0,
            height: 116.0,
            offset_x: 6.0,
            offset_y: 6.0,
            padding_x: 6.0,
            padding_y: 12.0,
            corner_radius: 4.0,
            border_width: 1.0,
            background: RgbaColor::rgba(255, 255, 255, 77),
            background_hover: RgbaColor::rgba(255, 255, 255, 140),
            border_color: RgbaColor::rgba(255, 255, 255, 77),
            border_color_hover: RgbaColor::rgba(255, 255, 255, 140),
            icon_visible: true,
            icon_width: 36.0,
            icon_height: 36.0,
            filename_visible: true,
            filename_font_size: 11.0,
            filename_bold: true,
            filename_max_width: 76.0,
            type_visible: true,
            type_font_size: 10.0,
            size_visible: true,
            size_font_size: 9.0,
            size_color: RgbaColor::rgba(0, 0, 0, 214),
            normal_text_color: RgbaColor::rgb(0, 0, 0),
            running_text_color: RgbaColor::rgb(76, 175, 80),
            text_gap: 2.0,
            fields: vec![CardField::Filename, CardField::Type, CardField::Size],
            click_action: ClickAction::Reveal,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollingConfig {
    pub wheel_speed: f32,
    pub drag_content: bool,
    pub scrollbar: ScrollbarMode,
}

impl Default for ScrollingConfig {
    fn default() -> Self {
        Self {
            wheel_speed: 1.0,
            drag_content: true,
            scrollbar: ScrollbarMode::Auto,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollbarConfig {
    pub width: f32,
    pub gap: f32,
    pub min_thumb_height: f32,
    pub corner_radius: f32,
    pub track_color: RgbaColor,
    pub thumb_color: RgbaColor,
    pub thumb_hover_color: RgbaColor,
}

impl Default for ScrollbarConfig {
    fn default() -> Self {
        Self {
            width: 8.0,
            gap: 8.0,
            min_thumb_height: 20.0,
            corner_radius: 4.0,
            track_color: RgbaColor::rgba(255, 255, 255, 26),
            thumb_color: RgbaColor::rgba(255, 255, 255, 77),
            thumb_hover_color: RgbaColor::rgba(255, 255, 255, 140),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SizeFormatConfig {
    pub base: u16,
    pub decimal_places: usize,
    pub units: Vec<String>,
}

impl Default for SizeFormatConfig {
    fn default() -> Self {
        Self {
            base: 1024,
            decimal_places: 2,
            units: ["B", "KB", "MB", "GB", "TB"].map(str::to_owned).to_vec(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    pub mode: FontMode,
    pub title: FontFaceConfig,
    pub card_regular: FontFaceConfig,
    pub card_bold: FontFaceConfig,
    pub footer: FontFaceConfig,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            mode: FontMode::Embedded,
            title: FontFaceConfig::default(),
            card_regular: FontFaceConfig::default(),
            card_bold: FontFaceConfig::default(),
            footer: FontFaceConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontFaceConfig {
    pub path: Option<PathBuf>,
    pub index: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FooterConfig {
    pub visible: bool,
    pub text: String,
    pub url: String,
    pub font_size: f32,
    pub left: f32,
    pub bottom: f32,
    pub color: RgbaColor,
    pub hover_color: RgbaColor,
}

impl Default for FooterConfig {
    fn default() -> Self {
        Self {
            visible: true,
            text: "Repo: github.com/Tobiichi-Origuchi/CefDetector (求个STAR!)".into(),
            url: "https://github.com/Tobiichi-Origuchi/CefDetector".into(),
            font_size: 12.0,
            left: 10.0,
            bottom: 32.0,
            color: RgbaColor::rgba(255, 255, 255, 204),
            hover_color: RgbaColor::rgb(255, 255, 255),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProgressConfig {
    pub batch_size: usize,
    pub batch_interval_ms: u64,
    pub show_partial_results: bool,
}

impl Default for ProgressConfig {
    fn default() -> Self {
        Self {
            batch_size: 20,
            batch_interval_ms: 50,
            show_partial_results: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GraphicsConfig {
    pub vsync: bool,
    pub api: GraphicsApi,
    pub linux_display: LinuxDisplay,
}

impl Default for GraphicsConfig {
    fn default() -> Self {
        Self {
            vsync: true,
            api: GraphicsApi::Auto,
            linux_display: LinuxDisplay::Auto,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct IconConfig {
    pub enabled: bool,
    pub decode_max_size: u32,
    pub fallback_path: Option<PathBuf>,
    pub sources: Vec<IconSource>,
    pub neighbor: NeighborIconConfig,
    pub linux: LinuxIconConfig,
}

impl Default for IconConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            decode_max_size: 64,
            fallback_path: None,
            sources: vec![
                IconSource::Bundle,
                IconSource::Pe,
                IconSource::Appimage,
                IconSource::Neighbor,
                IconSource::Desktop,
                IconSource::PackageManager,
                IconSource::Builtin,
            ],
            neighbor: NeighborIconConfig::default(),
            linux: LinuxIconConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NeighborIconConfig {
    pub directories: Vec<String>,
    pub names: Vec<String>,
}

impl Default for NeighborIconConfig {
    fn default() -> Self {
        Self {
            directories: [".", "resources", "assets"].map(str::to_owned).to_vec(),
            names: [
                "{executable}.png",
                "{executable}.svg",
                "icon.png",
                "logo.png",
                "app.png",
            ]
            .map(str::to_owned)
            .to_vec(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LinuxIconConfig {
    pub desktop_files: bool,
    pub package_managers: bool,
    pub theme_directories: Vec<PathBuf>,
}

impl Default for LinuxIconConfig {
    fn default() -> Self {
        Self {
            desktop_files: true,
            package_managers: true,
            theme_directories: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CliConfig {
    pub pretty: bool,
    pub sort_by: SortKey,
    pub sort_order: SortOrder,
    pub overwrite: bool,
    pub csv: CsvConfig,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            pretty: true,
            sort_by: SortKey::Path,
            sort_order: SortOrder::Ascending,
            overwrite: true,
            csv: CsvConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CsvConfig {
    pub header: bool,
    pub delimiter: char,
}

impl Default for CsvConfig {
    fn default() -> Self {
        Self {
            header: true,
            delimiter: ',',
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiagnosticsConfig {
    pub report_backend: bool,
    pub report_graphics: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            report_backend: true,
            report_graphics: true,
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::new(format!(
                "unsupported configuration version {}; expected {CONFIG_VERSION}",
                self.version
            )));
        }

        validate_search(&self.search)?;
        validate_gui(&self.gui)?;
        validate_icons(&self.icons)?;
        validate_cli(&self.cli)
    }
}

fn validate_search(config: &SearchConfig) -> Result<(), ConfigError> {
    if let Some(roots) = &config.roots {
        if roots.is_empty() {
            return Err(ConfigError::new("search.roots cannot be empty"));
        }
        for root in roots {
            require_absolute("search.roots", root)?;
        }
    }
    for path in &config.exclude_paths {
        require_absolute("search.exclude_paths", path)?;
    }
    if config
        .exclude_directory_names
        .iter()
        .any(|name| name.is_empty() || Path::new(name).components().count() != 1)
    {
        return Err(ConfigError::new(
            "search.exclude_directory_names entries must be nonempty directory names",
        ));
    }
    validate_thread_count("search.walk_threads", config.walk_threads)?;
    validate_thread_count("search.size_threads", config.size_threads)?;
    validate_timeout("search.plocate.timeout_ms", config.plocate.timeout_ms)?;
    validate_timeout(
        "search.everything.send_timeout_ms",
        u64::from(config.everything.send_timeout_ms),
    )?;
    validate_timeout(
        "search.everything.reply_timeout_ms",
        u64::from(config.everything.reply_timeout_ms),
    )?;
    validate_timeout("search.spotlight.timeout_ms", config.spotlight.timeout_ms)?;
    if config.plocate.command.as_os_str().is_empty()
        || config.spotlight.command.as_os_str().is_empty()
    {
        return Err(ConfigError::new("indexed backend commands cannot be empty"));
    }
    Ok(())
}

fn validate_gui(config: &GuiConfig) -> Result<(), ConfigError> {
    finite_range("gui.window.width", config.window.width, 100.0, 16_384.0)?;
    finite_range("gui.window.height", config.window.height, 100.0, 16_384.0)?;
    if config.window.title.is_empty() {
        return Err(ConfigError::new("gui.window.title cannot be empty"));
    }

    normalized("gui.status.x", config.status.x)?;
    normalized("gui.status.y", config.status.y)?;
    finite_range("gui.status.font_size", config.status.font_size, 1.0, 512.0)?;
    finite_range(
        "gui.status.min_font_size",
        config.status.min_font_size,
        1.0,
        512.0,
    )?;
    finite_range(
        "gui.status.max_font_size",
        config.status.max_font_size,
        1.0,
        512.0,
    )?;
    if config.status.min_font_size > config.status.max_font_size {
        return Err(ConfigError::new(
            "gui.status.min_font_size cannot exceed max_font_size",
        ));
    }
    validate_template(
        "gui.status.searching_text",
        &config.status.searching_text,
        &[],
    )?;
    validate_template(
        "gui.status.progress_text",
        &config.status.progress_text,
        &["count", "size"],
    )?;
    validate_template(
        "gui.status.success_text",
        &config.status.success_text,
        &["count", "size"],
    )?;
    validate_template("gui.status.empty_text", &config.status.empty_text, &[])?;
    validate_template(
        "gui.status.error_text",
        &config.status.error_text,
        &["error"],
    )?;

    normalized("gui.grid.x", config.grid.x)?;
    normalized("gui.grid.y", config.grid.y)?;
    normalized("gui.grid.width", config.grid.width)?;
    normalized("gui.grid.height", config.grid.height)?;
    if config.grid.width == 0.0 || config.grid.height == 0.0 {
        return Err(ConfigError::new(
            "gui.grid.width and gui.grid.height must be greater than zero",
        ));
    }
    positive("gui.grid.cell_width", config.grid.cell_width)?;
    positive("gui.grid.cell_height", config.grid.cell_height)?;
    if config.grid.min_columns == 0 || config.grid.min_columns > 256 {
        return Err(ConfigError::new(
            "gui.grid.min_columns must be between 1 and 256",
        ));
    }

    for (name, value) in [
        ("gui.card.width", config.card.width),
        ("gui.card.height", config.card.height),
        ("gui.card.icon_width", config.card.icon_width),
        ("gui.card.icon_height", config.card.icon_height),
        (
            "gui.card.filename_font_size",
            config.card.filename_font_size,
        ),
        (
            "gui.card.filename_max_width",
            config.card.filename_max_width,
        ),
        ("gui.card.type_font_size", config.card.type_font_size),
        ("gui.card.size_font_size", config.card.size_font_size),
    ] {
        positive(name, value)?;
    }
    for (name, value) in [
        ("gui.card.offset_x", config.card.offset_x),
        ("gui.card.offset_y", config.card.offset_y),
        ("gui.card.padding_x", config.card.padding_x),
        ("gui.card.padding_y", config.card.padding_y),
        ("gui.card.corner_radius", config.card.corner_radius),
        ("gui.card.border_width", config.card.border_width),
        ("gui.card.text_gap", config.card.text_gap),
    ] {
        nonnegative(name, value)?;
    }
    let unique_fields: HashSet<_> = config.card.fields.iter().copied().collect();
    if unique_fields.len() != config.card.fields.len() {
        return Err(ConfigError::new(
            "gui.card.fields cannot contain duplicates",
        ));
    }

    finite_range(
        "gui.scrolling.wheel_speed",
        config.scrolling.wheel_speed,
        0.01,
        100.0,
    )?;
    for (name, value) in [
        ("gui.scrollbar.width", config.scrollbar.width),
        (
            "gui.scrollbar.min_thumb_height",
            config.scrollbar.min_thumb_height,
        ),
    ] {
        positive(name, value)?;
    }
    for (name, value) in [
        ("gui.scrollbar.gap", config.scrollbar.gap),
        (
            "gui.scrollbar.corner_radius",
            config.scrollbar.corner_radius,
        ),
    ] {
        nonnegative(name, value)?;
    }

    if !matches!(config.size_format.base, 1000 | 1024) {
        return Err(ConfigError::new(
            "gui.size_format.base must be 1000 or 1024",
        ));
    }
    if config.size_format.decimal_places > 6 {
        return Err(ConfigError::new(
            "gui.size_format.decimal_places cannot exceed 6",
        ));
    }
    if config.size_format.units.is_empty() || config.size_format.units.iter().any(String::is_empty)
    {
        return Err(ConfigError::new(
            "gui.size_format.units must contain nonempty labels",
        ));
    }

    positive("gui.footer.font_size", config.footer.font_size)?;
    nonnegative("gui.footer.left", config.footer.left)?;
    nonnegative("gui.footer.bottom", config.footer.bottom)?;
    if config.footer.visible && (config.footer.text.is_empty() || config.footer.url.is_empty()) {
        return Err(ConfigError::new(
            "visible gui.footer requires nonempty text and url",
        ));
    }

    if config.progress.batch_size == 0 || config.progress.batch_size > 10_000 {
        return Err(ConfigError::new(
            "gui.progress.batch_size must be between 1 and 10000",
        ));
    }
    if !(5..=60_000).contains(&config.progress.batch_interval_ms) {
        return Err(ConfigError::new(
            "gui.progress.batch_interval_ms must be between 5 and 60000",
        ));
    }

    if config.fonts.mode == FontMode::Custom {
        for (name, face) in [
            ("title", &config.fonts.title),
            ("card_regular", &config.fonts.card_regular),
            ("card_bold", &config.fonts.card_bold),
            ("footer", &config.fonts.footer),
        ] {
            if face.path.is_none() {
                return Err(ConfigError::new(format!(
                    "gui.fonts.{name}.path is required in custom font mode"
                )));
            }
        }
    }
    Ok(())
}

fn validate_icons(config: &IconConfig) -> Result<(), ConfigError> {
    if config.decode_max_size == 0 || config.decode_max_size > 4096 {
        return Err(ConfigError::new(
            "icons.decode_max_size must be between 1 and 4096",
        ));
    }
    if config.enabled && config.sources.is_empty() {
        return Err(ConfigError::new(
            "icons.sources cannot be empty while icons are enabled",
        ));
    }
    let unique_sources: HashSet<_> = config.sources.iter().copied().collect();
    if unique_sources.len() != config.sources.len() {
        return Err(ConfigError::new("icons.sources cannot contain duplicates"));
    }
    if config
        .neighbor
        .directories
        .iter()
        .any(|directory| directory.is_empty())
        || config.neighbor.names.iter().any(String::is_empty)
    {
        return Err(ConfigError::new(
            "icon neighbor directories and names cannot be empty",
        ));
    }
    for name in &config.neighbor.names {
        validate_template("icons.neighbor.names", name, &["executable"])?;
    }
    Ok(())
}

fn validate_cli(config: &CliConfig) -> Result<(), ConfigError> {
    if matches!(config.csv.delimiter, '\r' | '\n' | '"') {
        return Err(ConfigError::new(
            "cli.csv.delimiter cannot be a quote or line break",
        ));
    }
    Ok(())
}

fn require_absolute(name: &str, path: &Path) -> Result<(), ConfigError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(ConfigError::new(format!(
            "{name} entries must be absolute paths: {}",
            path.display()
        )))
    }
}

fn validate_thread_count(name: &str, value: usize) -> Result<(), ConfigError> {
    if value <= 256 {
        Ok(())
    } else {
        Err(ConfigError::new(format!(
            "{name} cannot exceed 256; use 0 for automatic selection"
        )))
    }
}

fn validate_timeout(name: &str, value: u64) -> Result<(), ConfigError> {
    if (100..=600_000).contains(&value) {
        Ok(())
    } else {
        Err(ConfigError::new(format!(
            "{name} must be between 100 and 600000"
        )))
    }
}

fn finite_range(name: &str, value: f32, minimum: f32, maximum: f32) -> Result<(), ConfigError> {
    if value.is_finite() && (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(ConfigError::new(format!(
            "{name} must be a finite number between {minimum} and {maximum}"
        )))
    }
}

fn normalized(name: &str, value: f32) -> Result<(), ConfigError> {
    finite_range(name, value, 0.0, 1.0)
}

fn positive(name: &str, value: f32) -> Result<(), ConfigError> {
    finite_range(name, value, f32::EPSILON, 16_384.0)
}

fn nonnegative(name: &str, value: f32) -> Result<(), ConfigError> {
    finite_range(name, value, 0.0, 16_384.0)
}

fn validate_template(name: &str, template: &str, allowed: &[&str]) -> Result<(), ConfigError> {
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' if bytes.get(index + 1) == Some(&b'{') => index += 2,
            b'}' if bytes.get(index + 1) == Some(&b'}') => index += 2,
            b'{' => {
                let rest = &template[index + 1..];
                let Some(end) = rest.find('}') else {
                    return Err(ConfigError::new(format!(
                        "{name} contains an unmatched '{{'"
                    )));
                };
                let placeholder = &rest[..end];
                if !allowed.contains(&placeholder) {
                    return Err(ConfigError::new(format!(
                        "{name} contains unsupported placeholder {{{placeholder}}}"
                    )));
                }
                index += end + 2;
            }
            b'}' => {
                return Err(ConfigError::new(format!(
                    "{name} contains an unmatched '}}'"
                )));
            }
            _ => index += 1,
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    pub no_system: bool,
    pub no_user: bool,
    pub explicit_files: Vec<PathBuf>,
    pub overrides: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigPaths {
    pub system: Option<PathBuf>,
    pub user: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigSourceKind {
    System,
    User,
    Explicit,
}

impl fmt::Display for ConfigSourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::System => "system",
            Self::User => "user",
            Self::Explicit => "explicit",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigSource {
    pub kind: ConfigSourceKind,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub sources: Vec<ConfigSource>,
}

pub fn config_paths() -> ConfigPaths {
    #[cfg(target_os = "linux")]
    {
        let user = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| root.join("cefdetector/config.toml"));
        ConfigPaths {
            system: Some("/etc/cefdetector/config.toml".into()),
            user,
        }
    }

    #[cfg(target_os = "windows")]
    {
        ConfigPaths {
            system: std::env::var_os("ProgramData")
                .map(PathBuf::from)
                .map(|root| root.join("cefdetector/config.toml")),
            user: std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .map(|root| root.join("cefdetector/config.toml")),
        }
    }

    #[cfg(target_os = "macos")]
    {
        ConfigPaths {
            system: Some("/Library/Application Support/cefdetector/config.toml".into()),
            user: std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("Library/Application Support/cefdetector/config.toml")),
        }
    }
}

pub fn load(options: &LoadOptions) -> Result<LoadedConfig, ConfigError> {
    let mut effective = toml::Value::try_from(AppConfig::default())
        .map_err(|error| ConfigError::new(format!("failed to encode defaults: {error}")))?;
    let mut sources = Vec::new();
    let paths = config_paths();

    if !options.no_system
        && let Some(path) = paths.system
        && load_file(&path, ConfigSourceKind::System, false, &mut effective)?
    {
        sources.push(ConfigSource {
            kind: ConfigSourceKind::System,
            path,
        });
    }
    if !options.no_user
        && let Some(path) = paths.user
        && load_file(&path, ConfigSourceKind::User, false, &mut effective)?
    {
        sources.push(ConfigSource {
            kind: ConfigSourceKind::User,
            path,
        });
    }
    for path in &options.explicit_files {
        load_file(path, ConfigSourceKind::Explicit, true, &mut effective)?;
        sources.push(ConfigSource {
            kind: ConfigSourceKind::Explicit,
            path: path.clone(),
        });
    }
    for value in &options.overrides {
        let overlay = parse_override(value)?;
        merge_value(&mut effective, overlay);
        structurally_validate(&effective, &format!("command-line override {value:?}"))?;
    }

    let config: AppConfig = effective
        .try_into()
        .map_err(|error| ConfigError::new(format!("invalid effective configuration: {error}")))?;
    config.validate()?;
    Ok(LoadedConfig { config, sources })
}

pub fn format_effective(loaded: &LoadedConfig) -> Result<String, ConfigError> {
    let mut output = String::new();
    if loaded.sources.is_empty() {
        output.push_str("# Sources: built-in defaults only\n");
    } else {
        output.push_str("# Sources, lowest to highest precedence:\n");
        for source in &loaded.sources {
            output.push_str(&format!("# - {}: {}\n", source.kind, source.path.display()));
        }
    }
    output.push_str(
        &toml::to_string_pretty(&loaded.config).map_err(|error| {
            ConfigError::new(format!("failed to format configuration: {error}"))
        })?,
    );
    Ok(output)
}

fn load_file(
    path: &Path,
    kind: ConfigSourceKind,
    required: bool,
    effective: &mut toml::Value,
) -> Result<bool, ConfigError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => {
            return Err(ConfigError::new(format!(
                "failed to inspect {kind} config {}: {error}",
                path.display()
            )));
        }
    };
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::new(format!(
            "{kind} config {} exceeds the 1 MiB safety limit",
            path.display()
        )));
    }
    let content = std::fs::read_to_string(path).map_err(|error| {
        ConfigError::new(format!(
            "failed to read {kind} config {}: {error}",
            path.display()
        ))
    })?;
    let overlay: toml::Value = toml::from_str(&content).map_err(|error| {
        ConfigError::new(format!(
            "failed to parse {kind} config {}: {error}",
            path.display()
        ))
    })?;
    merge_value(effective, overlay);
    structurally_validate(effective, &format!("{kind} config {}", path.display()))?;
    Ok(true)
}

fn structurally_validate(value: &toml::Value, source: &str) -> Result<(), ConfigError> {
    value
        .clone()
        .try_into::<AppConfig>()
        .map(|_| ())
        .map_err(|error| ConfigError::new(format!("invalid {source}: {error}")))
}

fn merge_value(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge_value(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn parse_override(input: &str) -> Result<toml::Value, ConfigError> {
    let (path, value) = input.split_once('=').ok_or_else(|| {
        ConfigError::new(format!(
            "invalid --set value {input:?}; expected dotted.key=TOML_VALUE"
        ))
    })?;
    let parts: Vec<_> = path.split('.').collect();
    if parts.is_empty()
        || parts.iter().any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
    {
        return Err(ConfigError::new(format!(
            "invalid --set key {path:?}; use an unquoted dotted key"
        )));
    }

    let document = format!("value = {value}");
    let mut parsed: toml::Table = toml::from_str(&document).map_err(|error| {
        ConfigError::new(format!("invalid TOML value in --set {input:?}: {error}"))
    })?;
    let value = parsed
        .remove("value")
        .expect("the generated override document always contains value");
    let mut table = toml::Table::new();
    insert_override(&mut table, &parts, value);
    Ok(toml::Value::Table(table))
}

fn insert_override(table: &mut toml::Table, parts: &[&str], value: toml::Value) {
    if let [part] = parts {
        table.insert((*part).to_owned(), value);
        return;
    }
    let child = table
        .entry(parts[0].to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    insert_override(
        child
            .as_table_mut()
            .expect("new override path components are tables"),
        &parts[1..],
        value,
    );
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        AppConfig, ConfigError, LoadOptions, RgbaColor, SearchBackend, config_paths,
        format_effective, load, merge_value, parse_override,
    };

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    struct TempConfig(std::path::PathBuf);

    impl TempConfig {
        fn new(content: &str) -> Self {
            let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "cefdetector-config-{}-{sequence}.toml",
                std::process::id()
            ));
            fs::write(&path, content).unwrap();
            Self(path)
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn colors_accept_rgb_and_rgba() {
        assert_eq!(RgbaColor::parse("#2196F3").unwrap().0, [33, 150, 243, 255]);
        assert_eq!(
            RgbaColor::parse("#FFFFFF4D").unwrap().0,
            [255, 255, 255, 77]
        );
        assert!(RgbaColor::parse("red").is_err());
    }

    #[test]
    fn table_merge_is_recursive_and_arrays_replace() {
        let mut base: toml::Value = toml::from_str(
            "[search]\nbackend = 'auto'\nexclude_directory_names = ['one', 'two']\n",
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            "[search]\nbackend = 'filesystem'\nexclude_directory_names = ['three']\n",
        )
        .unwrap();
        merge_value(&mut base, overlay);
        assert_eq!(base["search"]["backend"].as_str(), Some("filesystem"));
        assert_eq!(
            base["search"]["exclude_directory_names"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn explicit_files_and_set_overrides_follow_precedence() {
        let first = TempConfig::new(
            "[search]\nbackend = 'filesystem'\nexclude_directory_names = ['first']\n",
        );
        let second = TempConfig::new("[search]\nexclude_directory_names = ['second']\n");
        let loaded = load(&LoadOptions {
            no_system: true,
            no_user: true,
            explicit_files: vec![first.0.clone(), second.0.clone()],
            overrides: vec!["search.backend=\"auto\"".into()],
        })
        .unwrap();

        assert_eq!(loaded.config.search.backend, SearchBackend::Auto);
        assert_eq!(loaded.config.search.exclude_directory_names, ["second"]);
        assert_eq!(loaded.sources.len(), 2);
    }

    #[test]
    fn unknown_keys_name_the_responsible_file() {
        let config = TempConfig::new("[search]\nunknown = true\n");
        let error = load(&LoadOptions {
            no_system: true,
            no_user: true,
            explicit_files: vec![config.0.clone()],
            overrides: Vec::new(),
        })
        .unwrap_err();
        assert!(error.to_string().contains(&config.0.display().to_string()));
        assert!(error.to_string().contains("unknown"));
    }

    #[test]
    fn missing_explicit_file_is_an_error() {
        let error = load(&LoadOptions {
            no_system: true,
            no_user: true,
            explicit_files: vec![std::env::temp_dir().join("cefdetector-definitely-missing.toml")],
            overrides: Vec::new(),
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to inspect explicit config")
        );
    }

    #[test]
    fn invalid_override_is_rejected() {
        assert!(parse_override("gui.window.width=1200").is_ok());
        assert!(parse_override("gui..width=1200").is_err());
        assert!(parse_override("gui.window.width=wide").is_err());
    }

    #[test]
    fn effective_config_round_trips() {
        let loaded = load(&LoadOptions {
            no_system: true,
            no_user: true,
            explicit_files: Vec::new(),
            overrides: Vec::new(),
        })
        .unwrap();
        let formatted = format_effective(&loaded).unwrap();
        let toml = formatted
            .lines()
            .filter(|line| !line.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let decoded: AppConfig = toml::from_str(&toml).unwrap();
        assert_eq!(decoded, AppConfig::default());
    }

    #[test]
    fn invalid_templates_and_ranges_are_rejected() {
        let mut config = AppConfig::default();
        config.gui.status.success_text = "{unknown}".into();
        assert!(config.validate().is_err());
        config.gui.status.success_text = "{count}".into();
        config.gui.window.width = f32::NAN;
        assert!(config.validate().is_err());
    }

    #[test]
    fn platform_paths_have_absolute_system_locations() {
        let paths = config_paths();
        assert!(
            paths
                .system
                .as_deref()
                .is_none_or(std::path::Path::is_absolute)
        );
        assert!(
            paths
                .user
                .as_deref()
                .is_none_or(std::path::Path::is_absolute)
        );
    }

    #[test]
    fn error_is_a_standard_error() {
        let error: Box<dyn std::error::Error> = Box::new(ConfigError::new("test"));
        assert_eq!(error.to_string(), "test");
    }
}

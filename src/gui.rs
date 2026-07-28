use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use egui::text::TextWrapping;
use egui::{
    Align2, Color32, ColorImage, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Id,
    Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, pos2, vec2,
};
use egui_glow::egui_winit::winit;
use glutin::context::PossiblyCurrentContext;
use glutin::display::Display;
use glutin::surface::{Surface, WindowSurface};
use winit::raw_window_handle::HasWindowHandle as _;

use crate::icon_finder::{RawIcon, get_app_icon};
use crate::search::core_search;

const WINDOW_TITLE: &str = "CEF Detector";
#[cfg(target_os = "linux")]
const APP_ID: &str = "cefdetector";
const REPOSITORY_URL: &str = "https://github.com/Tobiichi-Origuchi/CefDetectorLinux";

const WINDOW_WIDTH: f32 = 800.0;
const WINDOW_HEIGHT: f32 = 600.0;
const CARD_WIDTH: f32 = 94.0;
const CARD_HEIGHT: f32 = 116.0;
const CELL_WIDTH: f32 = 106.0;
const CELL_HEIGHT: f32 = 128.0;

const SEARCHING_TEXT: &str = "正在全盘搜索 CEF 应用，请耐心等待...";
const REPOSITORY_TEXT: &str = "Repo: github.com/Tobiichi-Origuchi/CefDetectorLinux (求个STAR!)";

#[derive(Debug)]
struct GuiError(String);

impl fmt::Display for GuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for GuiError {}

struct PendingItem {
    file: String,
    app_type: String,
    size: u64,
    is_running: bool,
    is_dir: bool,
    icon_raw: RawIcon,
    filename: String,
    raw_size: i32,
}

struct AppItem {
    file: String,
    app_type: String,
    size_str: String,
    is_running: bool,
    is_dir: bool,
    icon: TextureHandle,
    filename: String,
    raw_size: i32,
}

enum SearchMessage {
    Batch {
        items: Vec<PendingItem>,
        count: usize,
        total_size: u64,
    },
    Done {
        count: usize,
        total_size: u64,
    },
    Failed(String),
}

struct Frontend {
    receiver: mpsc::Receiver<SearchMessage>,
    apps: Vec<AppItem>,
    search_status: String,
    search_done: bool,
    scroll_offset: f32,
    scroll_drag_origin: Option<f32>,
    content_drag_origin: Option<(f32, f32)>,
    background: TextureHandle,
    default_icon: TextureHandle,
    decoded_icons: HashMap<u64, TextureHandle>,
    regular_font: FontFamily,
    bold_font: FontFamily,
}

impl Frontend {
    fn new(ctx: &egui::Context) -> Result<Self, GuiError> {
        let (regular_font, bold_font) = configure_fonts(ctx)?;

        let background = load_texture(
            ctx,
            "background",
            decode_raster(include_bytes!("../ui/background.webp"), false)
                .ok_or_else(|| GuiError("embedded background.webp is invalid".into()))?,
        );
        let default_icon = load_texture(
            ctx,
            "default-cef-icon",
            decode_raster(include_bytes!("../icons/default_cef_icon.ico"), true)
                .ok_or_else(|| GuiError("embedded default_cef_icon.ico is invalid".into()))?,
        );

        let (sender, receiver) = mpsc::channel();
        spawn_search(ctx.clone(), sender);

        Ok(Self {
            receiver,
            apps: Vec::new(),
            search_status: SEARCHING_TEXT.into(),
            search_done: false,
            scroll_offset: 0.0,
            scroll_drag_origin: None,
            content_drag_origin: None,
            background,
            default_icon,
            decoded_icons: HashMap::new(),
            regular_font,
            bold_font,
        })
    }

    fn receive_search_results(&mut self, ctx: &egui::Context) {
        let messages: Vec<_> = self.receiver.try_iter().collect();
        for message in messages {
            match message {
                SearchMessage::Batch {
                    items,
                    count,
                    total_size,
                } => {
                    for pending in items {
                        let icon = self.texture_for_icon(ctx, &pending.icon_raw);
                        self.apps.push(AppItem {
                            file: pending.file,
                            app_type: pending.app_type,
                            size_str: format_size(pending.size),
                            is_running: pending.is_running,
                            is_dir: pending.is_dir,
                            icon,
                            filename: pending.filename,
                            raw_size: pending.raw_size,
                        });
                    }
                    self.search_status = format!(
                        "这台电脑上已找到 {} 个 Chromium 内核的应用 ({}) - 搜索中...",
                        count,
                        format_size(total_size)
                    );
                }
                SearchMessage::Done { count, total_size } => {
                    self.apps
                        .sort_by_key(|item| std::cmp::Reverse(item.raw_size));
                    self.search_status = if count > 0 {
                        format!(
                            "搜索完成！这台电脑上总共有 {} 个 Chromium 内核的应用 ({})",
                            count,
                            format_size(total_size)
                        )
                    } else {
                        "搜索完成！这台电脑上没有 Chromium 内核的应用".into()
                    };
                    self.search_done = true;

                    // Every visible item owns a TextureHandle, so this lookup-only cache
                    // can be released once the scan is complete.
                    self.decoded_icons.clear();
                }
                SearchMessage::Failed(error) => {
                    self.search_status = format!("搜索失败：{error}");
                    self.search_done = true;
                    self.decoded_icons.clear();
                }
            }
        }
    }

    fn texture_for_icon(&mut self, ctx: &egui::Context, raw: &RawIcon) -> TextureHandle {
        let hash = hash_raw_icon(raw);
        if let Some(texture) = self.decoded_icons.get(&hash) {
            return texture.clone();
        }

        let texture = decode_icon(raw)
            .map(|image| load_texture(ctx, &format!("app-icon-{hash:016x}"), image))
            .unwrap_or_else(|| self.default_icon.clone());
        self.decoded_icons.insert(hash, texture.clone());
        texture
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.receive_search_results(ui.ctx());

        let root = ui.max_rect();
        let width = root.width();
        let height = root.height();
        let painter = ui.painter_at(root);

        paint_cover_image(&painter, root, &self.background);

        painter.text(
            pos2(root.left() + width * 0.5, root.top() + height * 0.21),
            Align2::CENTER_TOP,
            &self.search_status,
            FontId::new(18.0, self.bold_font.clone()),
            if self.search_done {
                Color32::from_rgb(33, 150, 243)
            } else {
                Color32::WHITE
            },
        );

        self.paint_grid(ui, root);
        self.paint_repository_link(ui, root);
    }

    fn paint_grid(&mut self, ui: &mut egui::Ui, root: Rect) {
        let scroll_width = root.width() * 0.80;
        let columns = ((scroll_width / CELL_WIDTH).floor() as usize).max(1);
        let rows = self.apps.len().div_ceil(columns);

        let viewport = Rect::from_min_size(
            pos2(
                root.left() + root.width() * 0.10,
                root.top() + root.height() * 0.30,
            ),
            vec2(
                (scroll_width - 16.0).max(0.0),
                (root.height() * 0.60).max(0.0),
            ),
        );
        let content_height = rows as f32 * CELL_HEIGHT;
        let max_scroll = (content_height - viewport.height()).max(0.0);
        self.scroll_offset = self.scroll_offset.clamp(0.0, max_scroll);

        let pointer_over_viewport = ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pos| viewport.contains(pos))
        });
        if pointer_over_viewport {
            let wheel = ui.input(|input| input.smooth_scroll_delta.y);
            if wheel != 0.0 {
                self.scroll_offset = (self.scroll_offset - wheel).clamp(0.0, max_scroll);
            }
        }
        let pointer = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
            )
        });
        if pointer.1
            && pointer
                .0
                .is_some_and(|position| viewport.contains(position))
            && max_scroll > 0.0
        {
            self.content_drag_origin = pointer.0.map(|position| (position.y, self.scroll_offset));
        }
        if pointer.2 {
            if let (Some(position), Some((start_y, start_scroll))) =
                (pointer.0, self.content_drag_origin)
            {
                self.scroll_offset = (start_scroll + start_y - position.y).clamp(0.0, max_scroll);
            }
        } else {
            self.content_drag_origin = None;
        }

        let clipped_painter = ui.painter().with_clip_rect(viewport);
        let first_row = (self.scroll_offset / CELL_HEIGHT).floor() as usize;
        let last_row = ((self.scroll_offset + viewport.height()) / CELL_HEIGHT).ceil() as usize + 1;
        let start_index = first_row.saturating_mul(columns);
        let end_index = last_row.saturating_mul(columns).min(self.apps.len());

        for index in start_index..end_index {
            let row = index / columns;
            let column = index % columns;
            let card = Rect::from_min_size(
                pos2(
                    viewport.left() + column as f32 * CELL_WIDTH + 6.0,
                    viewport.top() + row as f32 * CELL_HEIGHT + 6.0 - self.scroll_offset,
                ),
                vec2(CARD_WIDTH, CARD_HEIGHT),
            );
            if !card.intersects(viewport) {
                continue;
            }

            let response = ui.interact(
                card.intersect(viewport),
                Id::new(("app-card", index)),
                Sense::click(),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if response.clicked() {
                let item = &self.apps[index];
                crate::search::open_path(item.file.clone(), item.is_dir);
            }

            let card_background = if response.hovered() {
                Color32::from_white_alpha(140)
            } else {
                Color32::from_white_alpha(77)
            };
            clipped_painter.rect(
                card,
                4.0,
                card_background,
                Stroke::new(1.0, card_background),
                StrokeKind::Inside,
            );

            self.paint_card(&clipped_painter, card, &self.apps[index]);
        }

        if content_height > viewport.height() {
            self.paint_scrollbar(ui, viewport, content_height, max_scroll);
        } else {
            self.scroll_drag_origin = None;
        }
    }

    fn paint_card(&self, painter: &egui::Painter, card: Rect, item: &AppItem) {
        let running_color = if item.is_running {
            Color32::from_rgb(76, 175, 80)
        } else {
            Color32::BLACK
        };
        let filename = layout_elided(
            painter,
            &item.filename,
            FontId::new(11.0, self.bold_font.clone()),
            running_color,
            76.0,
        );
        let app_type = painter.layout_no_wrap(
            item.app_type.clone(),
            FontId::new(10.0, self.regular_font.clone()),
            running_color,
        );
        let size = painter.layout_no_wrap(
            item.size_str.clone(),
            FontId::new(9.0, self.regular_font.clone()),
            Color32::from_black_alpha(214),
        );

        let content_height =
            36.0 + filename.size().y + app_type.size().y + size.size().y + 3.0 * 2.0;
        let mut y = card.top() + 12.0 + ((CARD_HEIGHT - 24.0 - content_height) * 0.5).max(0.0);
        let center_x = card.center().x;

        // Slint's default image-fit is `fill` when both dimensions are explicit.
        let icon_rect = Rect::from_center_size(pos2(center_x, y + 18.0), vec2(36.0, 36.0));
        painter.image(
            item.icon.id(),
            icon_rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        y += 38.0;

        let text_clip = Rect::from_min_max(
            pos2(card.left() + 6.0, card.top() + 12.0),
            pos2(card.right() - 6.0, card.bottom() - 12.0),
        );
        let text_painter = painter.with_clip_rect(text_clip);

        text_painter.galley(
            pos2(center_x - filename.size().x * 0.5, y),
            filename.clone(),
            running_color,
        );
        y += filename.size().y + 2.0;
        text_painter.galley(
            pos2(center_x - app_type.size().x * 0.5, y),
            app_type.clone(),
            running_color,
        );
        y += app_type.size().y + 2.0;
        text_painter.galley(
            pos2(center_x - size.size().x * 0.5, y),
            size,
            Color32::from_black_alpha(214),
        );
    }

    fn paint_scrollbar(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        content_height: f32,
        max_scroll: f32,
    ) {
        let track = Rect::from_min_size(
            pos2(viewport.right() + 8.0, viewport.top()),
            vec2(8.0, viewport.height()),
        );
        ui.painter()
            .rect_filled(track, 4.0, Color32::from_white_alpha(26));

        let thumb_height = (track.height() * viewport.height() / content_height).max(20.0);
        let thumb_travel = (track.height() - thumb_height).max(0.0);
        let thumb_y = if max_scroll > 0.0 {
            track.top() + self.scroll_offset / max_scroll * thumb_travel
        } else {
            track.top()
        };
        let thumb = Rect::from_min_size(
            pos2(track.left(), thumb_y),
            vec2(track.width(), thumb_height),
        );

        let track_response = ui.interact(track, Id::new("app-grid-scroll-track"), Sense::hover());
        let thumb_response = ui.interact(thumb, Id::new("app-grid-scroll-thumb"), Sense::drag());

        if track_response.hovered() || thumb_response.hovered() || thumb_response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if thumb_response.drag_started() {
            self.scroll_drag_origin = Some(self.scroll_offset);
        }
        if thumb_response.dragged()
            && let Some(origin) = self.scroll_drag_origin
            && let Some(total_delta) = thumb_response.total_drag_delta()
            && thumb_travel > 0.0
        {
            self.scroll_offset =
                scroll_from_thumb_drag(origin, total_delta.y, max_scroll, thumb_travel);
        }
        if thumb_response.drag_stopped() {
            self.scroll_drag_origin = None;
        }

        let current_thumb_y = if max_scroll > 0.0 {
            track.top() + self.scroll_offset / max_scroll * thumb_travel
        } else {
            track.top()
        };
        let current_thumb = Rect::from_min_size(
            pos2(track.left(), current_thumb_y),
            vec2(track.width(), thumb_height),
        );
        let thumb_color = if track_response.hovered() || thumb_response.dragged() {
            Color32::from_white_alpha(140)
        } else {
            Color32::from_white_alpha(77)
        };
        ui.painter().rect(
            current_thumb,
            4.0,
            thumb_color,
            Stroke::new(1.0, thumb_color),
            StrokeKind::Inside,
        );
    }

    fn paint_repository_link(&self, ui: &mut egui::Ui, root: Rect) {
        let font_id = FontId::new(12.0, self.regular_font.clone());
        let normal_color = Color32::from_white_alpha(204);
        let measured_galley =
            ui.painter()
                .layout_no_wrap(REPOSITORY_TEXT.into(), font_id.clone(), normal_color);
        let position = pos2(root.left() + 10.0, root.bottom() - 32.0);
        let link_rect = Rect::from_min_size(position, measured_galley.size());
        let response = ui.interact(link_rect, Id::new("repository-link"), Sense::click());

        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if response.clicked() {
            crate::search::open_path(REPOSITORY_URL.into(), false);
        }

        let color = if response.hovered() {
            Color32::WHITE
        } else {
            normal_color
        };
        let galley = if response.hovered() {
            ui.painter()
                .layout_no_wrap(REPOSITORY_TEXT.into(), font_id, color)
        } else {
            measured_galley
        };
        ui.painter().galley(position, galley, color);
    }
}

fn spawn_search(ctx: egui::Context, sender: mpsc::Sender<SearchMessage>) {
    std::thread::spawn(move || {
        let mut count = 0;
        let mut total_size = 0;
        let mut batch = Vec::new();
        let mut last_flush = Instant::now();

        let search_result = core_search(|info| {
            count += 1;
            total_size += info.size;

            let filename = Path::new(&info.file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let size = info.size;
            batch.push(PendingItem {
                icon_raw: get_app_icon(info.file.clone()),
                file: info.file,
                app_type: info.app_type,
                size,
                is_running: info.is_running,
                is_dir: info.is_dir,
                filename,
                raw_size: (size / 1024) as i32,
            });

            if batch.len() >= 20 || last_flush.elapsed() >= Duration::from_millis(50) {
                let _ = sender.send(SearchMessage::Batch {
                    items: std::mem::take(&mut batch),
                    count,
                    total_size,
                });
                ctx.request_repaint();
                last_flush = Instant::now();
            }
        });

        if let Err(error) = search_result {
            let _ = sender.send(SearchMessage::Failed(error.to_string()));
            ctx.request_repaint();
            crate::icon_finder::clear_icon_caches();
            #[cfg(target_os = "linux")]
            crate::package_manager::clear_pm_cache();
            return;
        }

        if !batch.is_empty() {
            let _ = sender.send(SearchMessage::Batch {
                items: batch,
                count,
                total_size,
            });
            ctx.request_repaint();
        }

        let _ = sender.send(SearchMessage::Done { count, total_size });
        ctx.request_repaint();

        crate::icon_finder::clear_icon_caches();
        #[cfg(target_os = "linux")]
        crate::package_manager::clear_pm_cache();
    });
}

fn format_size(len: u64) -> String {
    if len == 0 {
        return "0.00 B".into();
    }

    let sizes = ["B", "KB", "MB", "GB", "TB"];
    let mut order = 0;
    let mut value = len as f64;
    while value >= 1024.0 && order < sizes.len() - 1 {
        order += 1;
        value /= 1024.0;
    }
    format!("{value:.2} {}", sizes[order])
}

fn scroll_from_thumb_drag(
    origin: f32,
    total_drag_y: f32,
    max_scroll: f32,
    thumb_travel: f32,
) -> f32 {
    (origin + total_drag_y * max_scroll / thumb_travel).clamp(0.0, max_scroll)
}

fn hash_raw_icon(icon: &RawIcon) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = DefaultHasher::new();
    match icon {
        RawIcon::Svg(bytes) | RawIcon::PngOrIco(bytes) => bytes.hash(&mut hasher),
        RawIcon::Empty => 0_u8.hash(&mut hasher),
    }
    hasher.finish()
}

fn load_texture(ctx: &egui::Context, name: &str, image: ColorImage) -> TextureHandle {
    ctx.load_texture(name, image, TextureOptions::LINEAR)
}

fn decode_icon(raw: &RawIcon) -> Option<ColorImage> {
    match raw {
        RawIcon::Svg(bytes) => decode_svg(bytes),
        RawIcon::PngOrIco(bytes) => decode_raster(bytes, true),
        RawIcon::Empty => None,
    }
}

fn decode_raster(bytes: &[u8], thumbnail: bool) -> Option<ColorImage> {
    let mut image = image::load_from_memory(bytes).ok()?;
    if thumbnail && (image.width() > 64 || image.height() > 64) {
        image = image.thumbnail(64, 64);
    }

    let rgba = image.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

fn decode_svg(bytes: &[u8]) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let source_size = tree.size();
    let max_side = source_size.width().max(source_size.height());
    if max_side <= 0.0 {
        return None;
    }

    let scale = 64.0 / max_side;
    let width = (source_size.width() * scale).round().max(1.0) as u32;
    let height = (source_size.height() * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    Some(ColorImage::from_rgba_premultiplied(
        [width as usize, height as usize],
        pixmap.data(),
    ))
}

fn paint_cover_image(painter: &egui::Painter, rect: Rect, texture: &TextureHandle) {
    let texture_size = texture.size_vec2();
    let texture_aspect = texture_size.x / texture_size.y;
    let target_aspect = rect.width() / rect.height().max(f32::EPSILON);
    let uv = if texture_aspect > target_aspect {
        let visible = target_aspect / texture_aspect;
        let margin = (1.0 - visible) * 0.5;
        Rect::from_min_max(pos2(margin, 0.0), pos2(1.0 - margin, 1.0))
    } else {
        let visible = texture_aspect / target_aspect;
        let margin = (1.0 - visible) * 0.5;
        Rect::from_min_max(pos2(0.0, margin), pos2(1.0, 1.0 - margin))
    };
    painter.image(texture.id(), rect, uv, Color32::WHITE);
}

fn layout_elided(
    painter: &egui::Painter,
    text: &str,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.into(), font_id, color, max_width);
    job.wrap = TextWrapping::truncate_at_width(max_width);
    painter.layout_job(job)
}

#[derive(Clone)]
struct SystemFont {
    path: PathBuf,
    index: u32,
}

#[cfg(target_os = "linux")]
fn match_system_font(pattern: &str) -> Option<SystemFont> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}\n", pattern])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let output = String::from_utf8(output.stdout).ok()?;
    let mut lines = output.lines();
    let path = PathBuf::from(lines.next()?);
    let index = lines.next()?.parse().ok()?;
    path.is_file().then_some(SystemFont { path, index })
}

#[cfg(target_os = "linux")]
fn regular_system_fonts() -> Vec<SystemFont> {
    [
        match_system_font("sans-serif"),
        match_system_font("sans-serif:lang=zh-cn"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(target_os = "linux")]
fn bold_system_fonts() -> Vec<SystemFont> {
    [
        match_system_font("sans-serif:style=bold"),
        match_system_font("sans-serif:lang=zh-cn:style=bold"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(target_os = "windows")]
fn fonts_from_windows_directory(file_names: &[&str]) -> Vec<SystemFont> {
    let windows_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts_dir = windows_dir.join("Fonts");
    file_names
        .iter()
        .map(|file_name| fonts_dir.join(file_name))
        .filter(|path| path.is_file())
        .map(|path| SystemFont { path, index: 0 })
        .collect()
}

#[cfg(target_os = "windows")]
fn regular_system_fonts() -> Vec<SystemFont> {
    fonts_from_windows_directory(&[
        "segoeui.ttf",
        "msyh.ttc",
        "msjh.ttc",
        "malgun.ttf",
        "meiryo.ttc",
    ])
}

#[cfg(target_os = "windows")]
fn bold_system_fonts() -> Vec<SystemFont> {
    fonts_from_windows_directory(&[
        "segoeuib.ttf",
        "msyhbd.ttc",
        "msjhbd.ttc",
        "malgunbd.ttf",
        "meiryob.ttc",
    ])
}

fn configure_fonts(ctx: &egui::Context) -> Result<(FontFamily, FontFamily), GuiError> {
    let regular_family = FontFamily::Name("cefdetector-regular".into());
    let bold_family = FontFamily::Name("cefdetector-bold".into());
    let mut definitions = FontDefinitions::empty();
    let mut mapped_files: HashMap<PathBuf, &'static [u8]> = HashMap::new();

    let regular_names = add_system_fonts(
        &mut definitions,
        &mut mapped_files,
        "cefdetector-system-regular",
        regular_system_fonts().into_iter(),
    );
    let mut bold_names = add_system_fonts(
        &mut definitions,
        &mut mapped_files,
        "cefdetector-system-bold",
        bold_system_fonts().into_iter(),
    );

    if regular_names.is_empty() {
        return Err(GuiError(
            "no supported system sans-serif font was found".into(),
        ));
    }
    if bold_names.is_empty() {
        bold_names.clone_from(&regular_names);
    }

    definitions
        .families
        .insert(FontFamily::Proportional, regular_names.clone());
    definitions
        .families
        .insert(FontFamily::Monospace, regular_names.clone());
    definitions
        .families
        .insert(regular_family.clone(), regular_names);
    definitions.families.insert(bold_family.clone(), bold_names);
    ctx.set_fonts(definitions);

    Ok((regular_family, bold_family))
}

fn add_system_fonts(
    definitions: &mut FontDefinitions,
    mapped_files: &mut HashMap<PathBuf, &'static [u8]>,
    name_prefix: &str,
    fonts: impl Iterator<Item = SystemFont>,
) -> Vec<String> {
    let mut names = Vec::new();
    for font in fonts {
        let name = format!("{name_prefix}-{}", names.len());
        let bytes = if let Some(bytes) = mapped_files.get(&font.path) {
            *bytes
        } else {
            let file = match std::fs::File::open(&font.path) {
                Ok(file) => file,
                Err(_) => continue,
            };
            // SAFETY: This is a read-only map of a font file. The mapping is leaked
            // deliberately because egui stores borrowed font bytes for the process
            // lifetime. This keeps large CJK fonts file-backed instead of copying
            // tens of megabytes into the heap.
            let mapping = match unsafe { memmap2::MmapOptions::new().map(&file) } {
                Ok(mapping) => Box::leak(Box::new(mapping)),
                Err(_) => continue,
            };
            let bytes: &'static [u8] = &mapping[..];
            mapped_files.insert(font.path.clone(), bytes);
            bytes
        };

        let mut data = FontData::from_static(bytes);
        data.index = font.index;
        definitions.font_data.insert(name.clone(), Arc::new(data));
        names.push(name);
    }
    names
}

fn window_icon() -> Option<winit::window::Icon> {
    let image = image::load_from_memory(include_bytes!("../icons/128x128.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    winit::window::Icon::from_rgba(image.into_raw(), width, height).ok()
}

struct EguiRenderer {
    egui_ctx: egui::Context,
    egui_winit: egui_glow::egui_winit::State,
    painter: egui_glow::Painter,
    viewport_info: egui::ViewportInfo,
    shapes: Vec<egui::epaint::ClippedShape>,
    pixels_per_point: f32,
    textures_delta: egui::TexturesDelta,
}

impl EguiRenderer {
    fn new(
        event_loop: &winit::event_loop::ActiveEventLoop,
        gl: Arc<egui_glow::glow::Context>,
    ) -> Result<Self, GuiError> {
        use egui_glow::glow::HasContext as _;

        // These three values exist since OpenGL 1.1, so they are safe to query
        // before egui_glow verifies its OpenGL 2.0 minimum.
        let graphics = unsafe {
            format!(
                "version {:?}, renderer {:?}, vendor {:?}",
                gl.get_parameter_string(egui_glow::glow::VERSION),
                gl.get_parameter_string(egui_glow::glow::RENDERER),
                gl.get_parameter_string(egui_glow::glow::VENDOR),
            )
        };
        let painter = egui_glow::Painter::new(Arc::clone(&gl), "", None, true).map_err(|error| {
            let platform_hint = if cfg!(target_os = "windows") {
                " Windows requires a graphics driver that exposes OpenGL 2.0 or newer; \
                 the Microsoft OpenGL 1.1 fallback is not sufficient."
            } else {
                ""
            };
            GuiError(format!(
                "failed to initialize the GUI renderer: {error}; detected OpenGL {graphics}.{platform_hint}"
            ))
        })?;
        let egui_ctx = egui::Context::default();
        let egui_winit = egui_glow::egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            event_loop,
            None,
            event_loop.system_theme(),
            Some(painter.max_texture_side()),
        );

        Ok(Self {
            egui_ctx,
            egui_winit,
            painter,
            viewport_info: Default::default(),
            shapes: Default::default(),
            pixels_per_point: 1.0,
            textures_delta: Default::default(),
        })
    }

    fn on_window_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> egui_glow::egui_winit::EventResponse {
        self.egui_winit.on_window_event(window, event)
    }

    fn run(&mut self, window: &winit::window::Window, run_ui: impl FnMut(&mut egui::Ui)) {
        let raw_input = self.egui_winit.take_egui_input(window);
        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            viewport_output,
        } = self.egui_ctx.run_ui(raw_input, run_ui);

        for (_, egui::ViewportOutput { commands, .. }) in viewport_output {
            let mut actions_requested = Default::default();
            egui_glow::egui_winit::process_viewport_commands(
                &self.egui_ctx,
                &mut self.viewport_info,
                commands,
                window,
                &mut actions_requested,
            );
        }
        self.egui_winit
            .handle_platform_output(window, platform_output);
        self.shapes = shapes;
        self.pixels_per_point = pixels_per_point;
        self.textures_delta.append(textures_delta);
    }

    fn paint(&mut self, window: &winit::window::Window) {
        let shapes = std::mem::take(&mut self.shapes);
        let mut textures_delta = std::mem::take(&mut self.textures_delta);

        for (id, image_delta) in textures_delta.set {
            self.painter.set_texture(id, &image_delta);
        }
        let clipped_primitives = self.egui_ctx.tessellate(shapes, self.pixels_per_point);
        self.painter.paint_primitives(
            window.inner_size().into(),
            self.pixels_per_point,
            &clipped_primitives,
        );
        for id in textures_delta.free.drain(..) {
            self.painter.free_texture(id);
        }
    }

    fn destroy(&mut self) {
        self.painter.destroy();
    }
}

struct GlutinWindow {
    window: winit::window::Window,
    context: PossiblyCurrentContext,
    display: Display,
    surface: Surface<WindowSurface>,
}

impl GlutinWindow {
    fn new(event_loop: &winit::event_loop::ActiveEventLoop) -> Result<Self, GuiError> {
        use glutin::config::GlConfig as _;
        use glutin::context::NotCurrentGlContext as _;
        use glutin::display::{GetGlDisplay as _, GlDisplay as _};
        use glutin::prelude::GlSurface as _;

        let window_attributes = winit::window::WindowAttributes::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
            .with_resizable(true)
            .with_visible(false)
            .with_window_icon(window_icon());

        #[cfg(target_os = "linux")]
        let window_attributes = {
            use winit::platform::x11::WindowAttributesExtX11 as _;
            window_attributes.with_name(APP_ID, APP_ID)
        };

        let config_template = glutin::config::ConfigTemplateBuilder::new()
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_multisampling(0)
            .with_transparency(false);

        let (mut window, config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(window_attributes.clone()))
            .build(event_loop, config_template, |configs| {
                configs
                    .reduce(|current, candidate| {
                        if candidate.hardware_accelerated() && !current.hardware_accelerated() {
                            candidate
                        } else {
                            current
                        }
                    })
                    .expect("no compatible OpenGL framebuffer configuration")
            })
            .map_err(|error| {
                GuiError(format!(
                    "failed to choose an OpenGL framebuffer configuration: {error}"
                ))
            })?;

        let display = config.display();
        let raw_window_handle = window
            .as_ref()
            .map(|window| {
                window
                    .window_handle()
                    .map(|handle| handle.as_raw())
                    .map_err(|error| GuiError(format!("failed to obtain window handle: {error}")))
            })
            .transpose()?;
        let context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::OpenGl(Some(
                glutin::context::Version::new(2, 0),
            )))
            .build(raw_window_handle);
        let fallback_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(Some(
                glutin::context::Version::new(2, 0),
            )))
            .build(raw_window_handle);

        // SAFETY: The attributes use the live window handle returned by winit and
        // the context remains owned alongside that window and display.
        let not_current = unsafe { display.create_context(&config, &context_attributes) }.or_else(
            |desktop_error| {
                // SAFETY: This uses the same live window handle and GL config.
                unsafe { display.create_context(&config, &fallback_attributes) }.map_err(
                    |gles_error| {
                        GuiError(format!(
                            "failed to create an OpenGL 2.0 context ({desktop_error}) \
                             or an OpenGL ES 2.0 context ({gles_error})"
                        ))
                    },
                )
            },
        )?;

        let window = if let Some(window) = window.take() {
            window
        } else {
            glutin_winit::finalize_window(event_loop, window_attributes, &config)
                .map_err(|error| GuiError(format!("failed to create the native window: {error}")))?
        };
        let size = window.inner_size();
        let window_handle = window
            .window_handle()
            .map_err(|error| GuiError(format!("failed to obtain window handle: {error}")))?;
        let surface_attributes = glutin::surface::SurfaceAttributesBuilder::<WindowSurface>::new()
            .build(
                window_handle.as_raw(),
                NonZeroU32::new(size.width).unwrap_or(NonZeroU32::MIN),
                NonZeroU32::new(size.height).unwrap_or(NonZeroU32::MIN),
            );

        // SAFETY: The surface uses the live window handle and matching GL config.
        let surface = unsafe {
            display
                .create_window_surface(&config, &surface_attributes)
                .map_err(|error| {
                    GuiError(format!(
                        "failed to create the OpenGL window surface: {error}"
                    ))
                })?
        };
        let context = not_current.make_current(&surface).map_err(|error| {
            GuiError(format!(
                "failed to make the OpenGL context current: {error}"
            ))
        })?;
        let _ = surface.set_swap_interval(
            &context,
            glutin::surface::SwapInterval::Wait(NonZeroU32::MIN),
        );

        Ok(Self {
            window,
            context,
            display,
            surface,
        })
    }

    fn resize(&self, size: winit::dpi::PhysicalSize<u32>) {
        use glutin::surface::GlSurface as _;

        self.surface.resize(
            &self.context,
            NonZeroU32::new(size.width).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(size.height).unwrap_or(NonZeroU32::MIN),
        );
    }

    fn swap_buffers(&self) -> Result<(), GuiError> {
        use glutin::surface::GlSurface as _;
        self.surface
            .swap_buffers(&self.context)
            .map_err(|error| GuiError(format!("failed to swap OpenGL buffers: {error}")))
    }

    fn proc_address(&self, symbol: &std::ffi::CStr) -> *const std::ffi::c_void {
        use glutin::display::GlDisplay as _;
        self.display.get_proc_address(symbol)
    }
}

#[derive(Debug)]
enum UserEvent {
    Repaint(Duration),
}

struct GlowApplication {
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    gl_window: Option<GlutinWindow>,
    gl: Option<Arc<egui_glow::glow::Context>>,
    egui: Option<EguiRenderer>,
    frontend: Option<Frontend>,
    error: Option<GuiError>,
    exit_after_first_frame: bool,
}

impl GlowApplication {
    fn new(proxy: winit::event_loop::EventLoopProxy<UserEvent>) -> Self {
        Self {
            proxy,
            gl_window: None,
            gl: None,
            egui: None,
            frontend: None,
            error: None,
            exit_after_first_frame: std::env::var_os("CEFDETECTOR_GUI_SMOKE_TEST").is_some(),
        }
    }

    fn initialize(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> Result<(), GuiError> {
        let gl_window = GlutinWindow::new(event_loop)?;
        // SAFETY: The loader resolves symbols from the current context created above.
        let gl = unsafe {
            egui_glow::glow::Context::from_loader_function(|symbol| {
                let Ok(symbol) = std::ffi::CString::new(symbol) else {
                    return std::ptr::null();
                };
                gl_window.proc_address(&symbol)
            })
        };
        let gl = Arc::new(gl);
        let mut egui = EguiRenderer::new(event_loop, Arc::clone(&gl))?;

        let proxy = self.proxy.clone();
        egui.egui_ctx.set_request_repaint_callback(move |request| {
            let _ = proxy.send_event(UserEvent::Repaint(request.delay));
        });
        let frontend = match Frontend::new(&egui.egui_ctx) {
            Ok(frontend) => frontend,
            Err(error) => {
                egui.destroy();
                return Err(error);
            }
        };

        self.frontend = Some(frontend);
        self.egui = Some(egui);
        self.gl = Some(gl);
        gl_window.window.request_redraw();
        self.gl_window = Some(gl_window);
        Ok(())
    }

    fn redraw(&mut self) -> Result<(), GuiError> {
        let gl_window = self
            .gl_window
            .as_ref()
            .ok_or_else(|| GuiError("GUI redraw requested before window initialization".into()))?;
        let frontend = self.frontend.as_mut().ok_or_else(|| {
            GuiError("GUI redraw requested before frontend initialization".into())
        })?;
        let egui = self.egui.as_mut().ok_or_else(|| {
            GuiError("GUI redraw requested before renderer initialization".into())
        })?;

        egui.run(&gl_window.window, |ui| frontend.ui(ui));

        // SAFETY: This GL context is current on the event-loop thread.
        unsafe {
            use egui_glow::glow::HasContext as _;
            let gl = self.gl.as_ref().ok_or_else(|| {
                GuiError("GUI redraw requested before OpenGL initialization".into())
            })?;
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(egui_glow::glow::COLOR_BUFFER_BIT);
        }
        egui.paint(&gl_window.window);
        gl_window.swap_buffers()?;
        gl_window.window.set_visible(true);
        Ok(())
    }

    fn fail(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, error: GuiError) {
        self.error = Some(error);
        event_loop.exit();
    }
}

impl winit::application::ApplicationHandler<UserEvent> for GlowApplication {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.gl_window.is_some() {
            return;
        }

        if let Err(error) = self.initialize(event_loop) {
            self.fail(event_loop, error);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let Some(gl_window) = self.gl_window.as_ref() else {
            return;
        };
        if gl_window.window.id() != window_id {
            return;
        }

        match &event {
            winit::event::WindowEvent::CloseRequested | winit::event::WindowEvent::Destroyed => {
                event_loop.exit();
                return;
            }
            winit::event::WindowEvent::RedrawRequested => {
                match self.redraw() {
                    Ok(()) if self.exit_after_first_frame => event_loop.exit(),
                    Ok(()) => {}
                    Err(error) => self.fail(event_loop, error),
                }
                return;
            }
            winit::event::WindowEvent::Resized(size) => gl_window.resize(*size),
            _ => {}
        }

        let Some(egui) = self.egui.as_mut() else {
            return;
        };
        let response = egui.on_window_event(&gl_window.window, &event);
        if response.repaint {
            gl_window.window.request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        let UserEvent::Repaint(delay) = event;
        if delay.is_zero() {
            if let Some(gl_window) = &self.gl_window {
                gl_window.window.request_redraw();
            }
        } else if let Some(deadline) = Instant::now().checked_add(delay) {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(deadline));
        }
    }

    fn new_events(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. })
            && let Some(gl_window) = &self.gl_window
        {
            gl_window.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Some(egui) = &mut self.egui {
            egui.destroy();
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = GlowApplication::new(proxy);
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(Box::new(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{format_size, scroll_from_thumb_drag};

    #[test]
    fn size_format_matches_the_original_ui() {
        assert_eq!(format_size(0), "0.00 B");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1536), "1.50 KB");
    }

    #[test]
    fn scrollbar_drag_uses_total_pointer_displacement() {
        assert_eq!(scroll_from_thumb_drag(20.0, 30.0, 200.0, 100.0), 80.0);
        assert_eq!(scroll_from_thumb_drag(20.0, -30.0, 200.0, 100.0), 0.0);
    }
}

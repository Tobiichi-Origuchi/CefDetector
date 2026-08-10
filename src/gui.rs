use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::text::TextWrapping;
use egui::{
    Color32, ColorImage, CursorIcon, Id, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use egui::{FontData, FontDefinitions, FontFamily, FontId};
use egui_glow::egui_winit::winit;
use glutin::context::PossiblyCurrentContext;
use glutin::display::Display;
use glutin::surface::{Surface, WindowSurface};
#[cfg(target_os = "macos")]
use std::cell::RefCell;
use winit::raw_window_handle::HasWindowHandle as _;

#[cfg(target_os = "linux")]
use crate::config::LinuxDisplay;
use crate::config::{
    AppConfig, BackgroundFit, CardField, ClickAction, FontMode, GraphicsApi, HorizontalAlign,
    RgbaColor, ScrollbarMode, SortKey, SortOrder, TextureFilter,
};
use crate::icon_finder::{RawIcon, configured_fallback_icon, get_app_icon};
use crate::search::core_search;

#[cfg(target_os = "macos")]
mod macos_text;

#[cfg(target_os = "linux")]
const APP_ID: &str = "cefdetector";

#[cfg(test)]
const SEARCHING_TEXT: &str = "正在全盘搜索 CEF 应用，请耐心等待...";
#[cfg(test)]
const REPOSITORY_TEXT: &str = "Repo: github.com/Tobiichi-Origuchi/CefDetector (求个STAR!)";

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
}

struct AppItem {
    file: String,
    app_type: String,
    size_str: String,
    is_running: bool,
    is_dir: bool,
    icon: TextureHandle,
    filename: String,
    size: u64,
}

#[derive(Clone, Copy)]
enum TextRole {
    Title,
    CardRegular,
    CardBold,
    Footer,
}

#[cfg(target_os = "macos")]
impl TextRole {
    fn is_bold(self) -> bool {
        matches!(self, Self::Title | Self::CardBold)
    }
}

struct EguiFonts {
    title: FontFamily,
    card_regular: FontFamily,
    card_bold: FontFamily,
    footer: FontFamily,
}

impl EguiFonts {
    fn family(&self, role: TextRole) -> FontFamily {
        match role {
            TextRole::Title => self.title.clone(),
            TextRole::CardRegular => self.card_regular.clone(),
            TextRole::CardBold => self.card_bold.clone(),
            TextRole::Footer => self.footer.clone(),
        }
    }
}

enum TextRenderer {
    Egui(EguiFonts),
    #[cfg(target_os = "macos")]
    CoreText(RefCell<macos_text::CoreTextRenderer>),
}

enum PreparedText {
    Egui(Arc<egui::Galley>),
    #[cfg(target_os = "macos")]
    CoreText(macos_text::NativeText),
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
    config: Arc<AppConfig>,
    receiver: mpsc::Receiver<SearchMessage>,
    apps: Vec<AppItem>,
    search_status: String,
    search_done: bool,
    search_failed: bool,
    scroll_offset: f32,
    scroll_drag_origin: Option<f32>,
    content_drag_origin: Option<(f32, f32)>,
    background: TextureHandle,
    default_icon: TextureHandle,
    decoded_icons: HashMap<u64, TextureHandle>,
    text_renderer: TextRenderer,
}

impl Frontend {
    fn new(ctx: &egui::Context, config: Arc<AppConfig>) -> Result<Self, GuiError> {
        let text_renderer = configure_text_renderer(ctx, &config.gui.fonts)?;

        let background_bytes = if let Some(path) = &config.gui.background.path {
            std::fs::read(path).map_err(|error| {
                GuiError(format!(
                    "failed to read configured background {}: {error}",
                    path.display()
                ))
            })?
        } else {
            include_bytes!("../ui/background.webp").to_vec()
        };
        let background = load_texture(
            ctx,
            "background",
            decode_raster(&background_bytes, false, u32::MAX)
                .ok_or_else(|| GuiError("configured background image is invalid".into()))?,
            texture_options(config.gui.background.filter),
        );
        let default_icon = load_texture(
            ctx,
            "default-cef-icon",
            decode_icon(
                &configured_fallback_icon(&config.icons),
                config.icons.decode_max_size,
            )
            .unwrap_or_else(|| ColorImage::from_rgba_unmultiplied([1, 1], &[0, 0, 0, 0])),
            TextureOptions::LINEAR,
        );

        let (sender, receiver) = mpsc::channel();
        spawn_search(ctx.clone(), sender, Arc::clone(&config));

        Ok(Self {
            search_status: config.gui.status.searching_text.clone(),
            config,
            receiver,
            apps: Vec::new(),
            search_done: false,
            search_failed: false,
            scroll_offset: 0.0,
            scroll_drag_origin: None,
            content_drag_origin: None,
            background,
            default_icon,
            decoded_icons: HashMap::new(),
            text_renderer,
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
                            size_str: format_size(pending.size, &self.config.gui.size_format),
                            is_running: pending.is_running,
                            is_dir: pending.is_dir,
                            icon,
                            filename: pending.filename,
                            size: pending.size,
                        });
                    }
                    self.search_status = format_status_template(
                        &self.config.gui.status.progress_text,
                        count,
                        total_size,
                        &self.config.gui.size_format,
                    );
                }
                SearchMessage::Done { count, total_size } => {
                    sort_apps(
                        &mut self.apps,
                        self.config.gui.grid.sort_by,
                        self.config.gui.grid.sort_order,
                    );
                    self.search_status = if count > 0 {
                        format_status_template(
                            &self.config.gui.status.success_text,
                            count,
                            total_size,
                            &self.config.gui.size_format,
                        )
                    } else {
                        self.config.gui.status.empty_text.clone()
                    };
                    self.search_done = true;

                    // Every visible item owns a TextureHandle, so this lookup-only cache
                    // can be released once the scan is complete.
                    self.decoded_icons.clear();
                }
                SearchMessage::Failed(error) => {
                    self.search_status =
                        self.config.gui.status.error_text.replace("{error}", &error);
                    self.search_done = true;
                    self.search_failed = true;
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

        let texture = decode_icon(raw, self.config.icons.decode_max_size)
            .map(|image| {
                load_texture(
                    ctx,
                    &format!("app-icon-{hash:016x}"),
                    image,
                    TextureOptions::LINEAR,
                )
            })
            .unwrap_or_else(|| self.default_icon.clone());
        self.decoded_icons.insert(hash, texture.clone());
        texture
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.receive_search_results(ui.ctx());

        let config = Arc::clone(&self.config);
        let root = ui.max_rect();
        let width = root.width();
        let height = root.height();
        let painter = ui.painter_at(root);

        paint_background(
            &painter,
            root,
            &self.background,
            config.gui.background.fit,
            color32(config.gui.background.fallback_color),
        );

        let status_color = if self.search_failed {
            color32(config.gui.status.error_color)
        } else if self.search_done {
            color32(config.gui.status.success_color)
        } else {
            color32(config.gui.status.searching_color)
        };
        if config.gui.status.visible {
            let status = self.layout_text(
                &painter,
                &self.search_status,
                title_font_size(root.size(), &config.gui.status, &config.gui.window),
                TextRole::Title,
                status_color,
                None,
            );
            let status_size = prepared_text_size(&status);
            let anchor_x = root.left() + width * config.gui.status.x;
            let status_x = match config.gui.status.horizontal_align {
                HorizontalAlign::Left => anchor_x,
                HorizontalAlign::Center => anchor_x - status_size.x * 0.5,
                HorizontalAlign::Right => anchor_x - status_size.x,
            };
            paint_prepared_text(
                &painter,
                pos2(status_x, root.top() + height * config.gui.status.y),
                &status,
                status_color,
            );
        }

        self.paint_grid(ui, root);
        if config.gui.footer.visible {
            self.paint_repository_link(ui, root);
        }
    }

    fn paint_grid(&mut self, ui: &mut egui::Ui, root: Rect) {
        let config = Arc::clone(&self.config);
        let grid = &config.gui.grid;
        let card_config = &config.gui.card;
        let scroll_width = root.width() * grid.width;
        let scrollbar_reserve = if config.gui.scrolling.scrollbar == ScrollbarMode::Hidden {
            0.0
        } else {
            config.gui.scrollbar.gap + config.gui.scrollbar.width
        };
        let viewport_width = (scroll_width - scrollbar_reserve).max(0.0);
        let columns = ((scroll_width / grid.cell_width).floor() as usize).max(grid.min_columns);
        let rows = self.apps.len().div_ceil(columns);

        let viewport = Rect::from_min_size(
            pos2(
                root.left() + root.width() * grid.x,
                root.top() + root.height() * grid.y,
            ),
            vec2(viewport_width, (root.height() * grid.height).max(0.0)),
        );
        let content_height = rows as f32 * grid.cell_height;
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
                self.scroll_offset = (self.scroll_offset
                    - wheel * config.gui.scrolling.wheel_speed)
                    .clamp(0.0, max_scroll);
            }
        }
        let pointer = ui.input(|input| {
            (
                input.pointer.interact_pos(),
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
            )
        });
        if config.gui.scrolling.drag_content
            && pointer.1
            && pointer
                .0
                .is_some_and(|position| viewport.contains(position))
            && max_scroll > 0.0
        {
            self.content_drag_origin = pointer.0.map(|position| (position.y, self.scroll_offset));
        }
        if config.gui.scrolling.drag_content && pointer.2 {
            if let (Some(position), Some((start_y, start_scroll))) =
                (pointer.0, self.content_drag_origin)
            {
                self.scroll_offset = (start_scroll + start_y - position.y).clamp(0.0, max_scroll);
            }
        } else {
            self.content_drag_origin = None;
        }

        let clipped_painter = ui.painter().with_clip_rect(viewport);
        let first_row = (self.scroll_offset / grid.cell_height).floor() as usize;
        let last_row =
            ((self.scroll_offset + viewport.height()) / grid.cell_height).ceil() as usize + 1;
        let start_index = first_row.saturating_mul(columns);
        let end_index = last_row.saturating_mul(columns).min(self.apps.len());

        for index in start_index..end_index {
            let row = index / columns;
            let column = index % columns;
            let card = Rect::from_min_size(
                pos2(
                    viewport.left() + column as f32 * grid.cell_width + card_config.offset_x,
                    viewport.top() + row as f32 * grid.cell_height + card_config.offset_y
                        - self.scroll_offset,
                ),
                vec2(card_config.width, card_config.height),
            );
            if !card.intersects(viewport) {
                continue;
            }

            let sense = if card_config.click_action == ClickAction::None {
                Sense::hover()
            } else {
                Sense::click()
            };
            let response = ui.interact(
                card.intersect(viewport),
                Id::new(("app-card", index)),
                sense,
            );
            if response.hovered() && card_config.click_action != ClickAction::None {
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            if response.clicked() {
                let item = &self.apps[index];
                match card_config.click_action {
                    ClickAction::Reveal => {
                        crate::search::open_path(item.file.clone(), item.is_dir, true);
                    }
                    ClickAction::Open => {
                        crate::search::open_path(item.file.clone(), item.is_dir, false);
                    }
                    ClickAction::None => {}
                }
            }

            let card_background = if response.hovered() {
                color32(card_config.background_hover)
            } else {
                color32(card_config.background)
            };
            let border_color = if response.hovered() {
                color32(card_config.border_color_hover)
            } else {
                color32(card_config.border_color)
            };
            clipped_painter.rect(
                card,
                card_config.corner_radius,
                card_background,
                Stroke::new(card_config.border_width, border_color),
                StrokeKind::Inside,
            );

            self.paint_card(&clipped_painter, card, &self.apps[index]);
        }

        let show_scrollbar = match config.gui.scrolling.scrollbar {
            ScrollbarMode::Auto => content_height > viewport.height(),
            ScrollbarMode::Always => true,
            ScrollbarMode::Hidden => false,
        };
        if show_scrollbar {
            self.paint_scrollbar(ui, viewport, content_height, max_scroll);
        } else {
            self.scroll_drag_origin = None;
        }
    }

    fn paint_card(&self, painter: &egui::Painter, card: Rect, item: &AppItem) {
        let card_config = &self.config.gui.card;
        let running_color = if item.is_running {
            color32(card_config.running_text_color)
        } else {
            color32(card_config.normal_text_color)
        };
        let size_color = color32(card_config.size_color);
        let mut text = Vec::new();
        for field in &card_config.fields {
            let prepared = match field {
                CardField::Filename if card_config.filename_visible => Some((
                    self.layout_text(
                        painter,
                        &item.filename,
                        card_config.filename_font_size,
                        if card_config.filename_bold {
                            TextRole::CardBold
                        } else {
                            TextRole::CardRegular
                        },
                        running_color,
                        Some(card_config.filename_max_width),
                    ),
                    running_color,
                )),
                CardField::Type if card_config.type_visible => Some((
                    self.layout_text(
                        painter,
                        &item.app_type,
                        card_config.type_font_size,
                        TextRole::CardRegular,
                        running_color,
                        None,
                    ),
                    running_color,
                )),
                CardField::Size if card_config.size_visible => Some((
                    self.layout_text(
                        painter,
                        &item.size_str,
                        card_config.size_font_size,
                        TextRole::CardRegular,
                        size_color,
                        None,
                    ),
                    size_color,
                )),
                _ => None,
            };
            if let Some(prepared) = prepared {
                text.push(prepared);
            }
        }

        let icon_visible = card_config.icon_visible && self.config.icons.enabled;
        let element_count = text.len() + usize::from(icon_visible);
        let content_height = text
            .iter()
            .map(|(prepared, _)| prepared_text_size(prepared).y)
            .sum::<f32>()
            + if icon_visible {
                card_config.icon_height
            } else {
                0.0
            }
            + element_count.saturating_sub(1) as f32 * card_config.text_gap;
        let available_height = (card.height() - card_config.padding_y * 2.0).max(0.0);
        let mut y = card.top()
            + card_config.padding_y
            + ((available_height - content_height) * 0.5).max(0.0);
        let center_x = card.center().x;

        if icon_visible {
            // Slint's default image-fit is `fill` when both dimensions are explicit.
            let icon_rect = Rect::from_center_size(
                pos2(center_x, y + card_config.icon_height * 0.5),
                vec2(card_config.icon_width, card_config.icon_height),
            );
            painter.image(
                item.icon.id(),
                icon_rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            y += card_config.icon_height;
            if !text.is_empty() {
                y += card_config.text_gap;
            }
        }

        let text_clip = Rect::from_min_max(
            pos2(
                card.left() + card_config.padding_x,
                card.top() + card_config.padding_y,
            ),
            pos2(
                card.right() - card_config.padding_x,
                card.bottom() - card_config.padding_y,
            ),
        );
        let text_painter = painter.with_clip_rect(text_clip);

        let text_count = text.len();
        for (index, (prepared, text_color)) in text.into_iter().enumerate() {
            let text_size = prepared_text_size(&prepared);
            paint_prepared_text(
                &text_painter,
                pos2(center_x - text_size.x * 0.5, y),
                &prepared,
                text_color,
            );
            y += text_size.y;
            if index + 1 < text_count {
                y += card_config.text_gap;
            }
        }
    }

    fn layout_text(
        &self,
        painter: &egui::Painter,
        text: &str,
        point_size: f32,
        role: TextRole,
        color: Color32,
        max_width: Option<f32>,
    ) -> PreparedText {
        match &self.text_renderer {
            TextRenderer::Egui(fonts) => {
                let font_id = FontId::new(point_size, fonts.family(role));
                let galley = if let Some(max_width) = max_width {
                    layout_elided(painter, text, font_id, color, max_width)
                } else {
                    painter.layout_no_wrap(text.to_owned(), font_id, color)
                };
                PreparedText::Egui(galley)
            }
            #[cfg(target_os = "macos")]
            TextRenderer::CoreText(renderer) => {
                PreparedText::CoreText(renderer.borrow_mut().layout(
                    painter.ctx(),
                    text,
                    point_size,
                    role.is_bold(),
                    color,
                    max_width,
                ))
            }
        }
    }

    fn paint_scrollbar(
        &mut self,
        ui: &mut egui::Ui,
        viewport: Rect,
        content_height: f32,
        max_scroll: f32,
    ) {
        let scrollbar = &self.config.gui.scrollbar;
        let track = Rect::from_min_size(
            pos2(viewport.right() + scrollbar.gap, viewport.top()),
            vec2(scrollbar.width, viewport.height()),
        );
        ui.painter().rect_filled(
            track,
            scrollbar.corner_radius,
            color32(scrollbar.track_color),
        );

        let thumb_height = (track.height() * viewport.height()
            / content_height.max(viewport.height()).max(f32::EPSILON))
        .max(scrollbar.min_thumb_height)
        .min(track.height());
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
            color32(scrollbar.thumb_hover_color)
        } else {
            color32(scrollbar.thumb_color)
        };
        ui.painter().rect(
            current_thumb,
            scrollbar.corner_radius,
            thumb_color,
            Stroke::new(1.0, thumb_color),
            StrokeKind::Inside,
        );
    }

    fn paint_repository_link(&self, ui: &mut egui::Ui, root: Rect) {
        let footer = &self.config.gui.footer;
        let normal_color = color32(footer.color);
        let measured_text = self.layout_text(
            ui.painter(),
            &footer.text,
            footer.font_size,
            TextRole::Footer,
            normal_color,
            None,
        );
        let position = pos2(root.left() + footer.left, root.bottom() - footer.bottom);
        let link_rect = Rect::from_min_size(position, prepared_text_size(&measured_text));
        let response = ui.interact(link_rect, Id::new("repository-link"), Sense::click());

        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if response.clicked() {
            crate::search::open_path(footer.url.clone(), false, false);
        }

        let color = if response.hovered() {
            color32(footer.hover_color)
        } else {
            normal_color
        };
        let text = if response.hovered() {
            self.layout_text(
                ui.painter(),
                &footer.text,
                footer.font_size,
                TextRole::Footer,
                color,
                None,
            )
        } else {
            measured_text
        };
        paint_prepared_text(ui.painter(), position, &text, color);
    }
}

fn prepared_text_size(text: &PreparedText) -> Vec2 {
    match text {
        PreparedText::Egui(text) => text.size(),
        #[cfg(target_os = "macos")]
        PreparedText::CoreText(text) => text.size,
    }
}

fn paint_prepared_text(
    painter: &egui::Painter,
    position: Pos2,
    text: &PreparedText,
    color: Color32,
) {
    match text {
        PreparedText::Egui(text) => {
            painter.galley(position, text.clone(), color);
        }
        #[cfg(target_os = "macos")]
        PreparedText::CoreText(text) => {
            painter.image(
                text.texture.id(),
                Rect::from_min_size(position, text.size),
                Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}

fn spawn_search(
    ctx: egui::Context,
    sender: mpsc::Sender<SearchMessage>,
    config: Arc<crate::config::AppConfig>,
) {
    std::thread::spawn(move || {
        let mut count = 0;
        let mut total_size = 0;
        let mut batch = Vec::new();
        let mut last_flush = Instant::now();

        let search_result = core_search(&config, |info| {
            count += 1;
            total_size += info.size;

            let filename = Path::new(&info.file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let size = info.size;
            batch.push(PendingItem {
                icon_raw: get_app_icon(info.file.clone(), &config.icons),
                file: info.file,
                app_type: info.app_type,
                size,
                is_running: info.is_running,
                is_dir: info.is_dir,
                filename,
            });

            if config.gui.progress.show_partial_results
                && (batch.len() >= config.gui.progress.batch_size
                    || last_flush.elapsed()
                        >= Duration::from_millis(config.gui.progress.batch_interval_ms))
            {
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

fn format_size(len: u64, config: &crate::config::SizeFormatConfig) -> String {
    if len == 0 {
        return format!("{:.*} {}", config.decimal_places, 0.0, config.units[0]);
    }

    let mut order = 0;
    let mut value = len as f64;
    let base = f64::from(config.base);
    while value >= base && order < config.units.len() - 1 {
        order += 1;
        value /= base;
    }
    format!(
        "{:.*} {}",
        config.decimal_places, value, config.units[order]
    )
}

fn format_status_template(
    template: &str,
    count: usize,
    size: u64,
    size_config: &crate::config::SizeFormatConfig,
) -> String {
    template
        .replace("{count}", &count.to_string())
        .replace("{size}", &format_size(size, size_config))
}

fn sort_apps(apps: &mut [AppItem], key: SortKey, order: SortOrder) {
    apps.sort_by(|left, right| {
        let ordering = match key {
            SortKey::Size => left.size.cmp(&right.size),
            SortKey::Name => left.filename.cmp(&right.filename),
            SortKey::Type => left.app_type.cmp(&right.app_type),
            SortKey::Path => left.file.cmp(&right.file),
            SortKey::Running => left.is_running.cmp(&right.is_running),
        };
        match order {
            SortOrder::Ascending => ordering,
            SortOrder::Descending => ordering.reverse(),
        }
    });
}

fn title_font_size(
    window_size: Vec2,
    status: &crate::config::StatusConfig,
    window: &crate::config::WindowConfig,
) -> f32 {
    if !status.dynamic_font_size {
        return status.font_size;
    }
    let width_scale = window_size.x / window.width;
    let height_scale = window_size.y / window.height;
    (status.font_size * width_scale.min(height_scale))
        .clamp(status.min_font_size, status.max_font_size)
        .round()
}

fn color32(color: RgbaColor) -> Color32 {
    let [red, green, blue, alpha] = color.channels();
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
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

fn texture_options(filter: TextureFilter) -> TextureOptions {
    match filter {
        TextureFilter::Linear => TextureOptions::LINEAR,
        TextureFilter::Nearest => TextureOptions::NEAREST,
    }
}

fn load_texture(
    ctx: &egui::Context,
    name: &str,
    image: ColorImage,
    options: TextureOptions,
) -> TextureHandle {
    ctx.load_texture(name, image, options)
}

fn decode_icon(raw: &RawIcon, max_size: u32) -> Option<ColorImage> {
    match raw {
        RawIcon::Svg(bytes) => decode_svg(bytes, max_size),
        RawIcon::PngOrIco(bytes) => decode_raster(bytes, true, max_size),
        RawIcon::Empty => None,
    }
}

fn decode_raster(bytes: &[u8], thumbnail: bool, max_size: u32) -> Option<ColorImage> {
    let mut image = image::load_from_memory(bytes).ok()?;
    if thumbnail && (image.width() > max_size || image.height() > max_size) {
        image = image.thumbnail(max_size, max_size);
    }

    let rgba = image.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

fn decode_svg(bytes: &[u8], max_size: u32) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let source_size = tree.size();
    let max_side = source_size.width().max(source_size.height());
    if max_side <= 0.0 {
        return None;
    }

    let scale = max_size as f32 / max_side;
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

fn paint_background(
    painter: &egui::Painter,
    rect: Rect,
    texture: &TextureHandle,
    fit: BackgroundFit,
    fallback: Color32,
) {
    painter.rect_filled(rect, 0.0, fallback);
    if fit == BackgroundFit::Stretch {
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        return;
    }
    let texture_size = texture.size_vec2();
    let texture_aspect = texture_size.x / texture_size.y;
    let target_aspect = rect.width() / rect.height().max(f32::EPSILON);
    if fit == BackgroundFit::Contain {
        let size = if texture_aspect > target_aspect {
            vec2(rect.width(), rect.width() / texture_aspect)
        } else {
            vec2(rect.height() * texture_aspect, rect.height())
        };
        painter.image(
            texture.id(),
            Rect::from_center_size(rect.center(), size),
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        return;
    }
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

#[cfg(not(target_os = "macos"))]
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

fn configure_text_renderer(
    ctx: &egui::Context,
    config: &crate::config::FontConfig,
) -> Result<TextRenderer, GuiError> {
    match config.mode {
        FontMode::Embedded => Ok(TextRenderer::Egui(configure_embedded_fonts(ctx))),
        FontMode::Custom => configure_custom_fonts(ctx, config).map(TextRenderer::Egui),
        FontMode::System => {
            #[cfg(target_os = "macos")]
            return Ok(TextRenderer::CoreText(RefCell::new(
                macos_text::CoreTextRenderer::new(),
            )));

            #[cfg(not(target_os = "macos"))]
            return configure_system_fonts(ctx).map(TextRenderer::Egui);
        }
    }
}

fn configure_custom_fonts(
    ctx: &egui::Context,
    config: &crate::config::FontConfig,
) -> Result<EguiFonts, GuiError> {
    let roles = [
        ("title", config.title.as_slice()),
        ("card-regular", config.card_regular.as_slice()),
        ("card-bold", config.card_bold.as_slice()),
        ("footer", config.footer.as_slice()),
    ];
    let mut definitions = FontDefinitions::empty();
    let mut families = Vec::new();
    let mut proportional_fallback = Vec::new();
    let mut loaded_fonts: HashMap<(PathBuf, u32), String> = HashMap::new();
    for (role, chain) in roles {
        let family = FontFamily::Name(format!("cefdetector-custom-{role}").into());
        let mut data_names = Vec::new();
        for (index, face) in chain.iter().enumerate() {
            let path = face.path.as_ref().ok_or_else(|| {
                GuiError(format!(
                    "custom font {role}[{index}] has no path after configuration validation"
                ))
            })?;
            let key = (path.clone(), face.index);
            let data_name = if let Some(data_name) = loaded_fonts.get(&key) {
                data_name.clone()
            } else {
                let bytes = std::fs::read(path).map_err(|error| {
                    GuiError(format!(
                        "failed to read custom font {}: {error}",
                        path.display()
                    ))
                })?;
                let data_name = format!("cefdetector-custom-font-{}", loaded_fonts.len());
                let mut data = FontData::from_owned(bytes);
                data.index = face.index;
                definitions
                    .font_data
                    .insert(data_name.clone(), Arc::new(data));
                loaded_fonts.insert(key, data_name.clone());
                data_name
            };
            data_names.push(data_name);
        }
        if role == "card-regular" {
            proportional_fallback.clone_from(&data_names);
        }
        definitions.families.insert(family.clone(), data_names);
        families.push(family);
    }
    definitions
        .families
        .insert(FontFamily::Proportional, proportional_fallback.clone());
    definitions
        .families
        .insert(FontFamily::Monospace, proportional_fallback);
    ctx.set_fonts(definitions);

    Ok(EguiFonts {
        title: families.remove(0),
        card_regular: families.remove(0),
        card_bold: families.remove(0),
        footer: families.remove(0),
    })
}

fn configure_embedded_fonts(ctx: &egui::Context) -> EguiFonts {
    const TITLE_NAME: &str = "cefdetector-embedded-title";
    const CARD_REGULAR_NAME: &str = "cefdetector-embedded-card-regular";
    const CARD_BOLD_NAME: &str = "cefdetector-embedded-card-bold";

    let title = FontFamily::Name("cefdetector-title".into());
    let card_regular = FontFamily::Name("cefdetector-card-regular".into());
    let card_bold = FontFamily::Name("cefdetector-card-bold".into());
    let footer = FontFamily::Name("cefdetector-footer".into());
    let mut definitions = FontDefinitions::empty();
    definitions.font_data.insert(
        TITLE_NAME.into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../fonts/title-subset.ttf"
        ))),
    );
    definitions.font_data.insert(
        CARD_REGULAR_NAME.into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../fonts/card-regular-subset.ttf"
        ))),
    );
    definitions.font_data.insert(
        CARD_BOLD_NAME.into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../fonts/card-bold-subset.ttf"
        ))),
    );

    definitions.families.insert(
        FontFamily::Proportional,
        vec![CARD_REGULAR_NAME.into(), TITLE_NAME.into()],
    );
    definitions
        .families
        .insert(FontFamily::Monospace, vec![CARD_REGULAR_NAME.into()]);
    definitions
        .families
        .insert(title.clone(), vec![TITLE_NAME.into()]);
    definitions
        .families
        .insert(card_regular.clone(), vec![CARD_REGULAR_NAME.into()]);
    definitions
        .families
        .insert(card_bold.clone(), vec![CARD_BOLD_NAME.into()]);
    definitions.families.insert(
        footer.clone(),
        vec![CARD_REGULAR_NAME.into(), TITLE_NAME.into()],
    );
    ctx.set_fonts(definitions);

    EguiFonts {
        title,
        card_regular,
        card_bold,
        footer,
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_system_fonts(ctx: &egui::Context) -> Result<EguiFonts, GuiError> {
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

    Ok(EguiFonts {
        title: bold_family.clone(),
        card_regular: regular_family.clone(),
        card_bold: bold_family,
        footer: regular_family,
    })
}

#[cfg(not(target_os = "macos"))]
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
        report_graphics: bool,
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
        if report_graphics {
            eprintln!("cefdetector-graphics={graphics}");
        }
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
    // Rust drops fields in declaration order. GPU resources must be released
    // before their display server window (notably EGL surfaces on Wayland).
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    display: Display,
    window: winit::window::Window,
}

impl GlutinWindow {
    fn new(
        event_loop: &winit::event_loop::ActiveEventLoop,
        config: &AppConfig,
    ) -> Result<Self, GuiError> {
        use glutin::config::GlConfig as _;
        use glutin::context::NotCurrentGlContext as _;
        use glutin::display::{GetGlDisplay as _, GlDisplay as _};
        use glutin::prelude::GlSurface as _;

        let window_attributes = winit::window::WindowAttributes::default()
            .with_title(&config.gui.window.title)
            .with_inner_size(winit::dpi::LogicalSize::new(
                config.gui.window.width,
                config.gui.window.height,
            ))
            .with_resizable(config.gui.window.resizable)
            .with_maximized(config.gui.window.maximized)
            .with_fullscreen(
                config
                    .gui
                    .window
                    .fullscreen
                    .then_some(winit::window::Fullscreen::Borderless(None)),
            )
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
            .with_transparency(false);

        let display_builder = glutin_winit::DisplayBuilder::new();
        #[cfg(target_os = "linux")]
        let display_builder =
            display_builder.with_preference(match config.gui.graphics.linux_display {
                LinuxDisplay::Egl => glutin_winit::ApiPreference::PreferEgl,
                LinuxDisplay::Auto | LinuxDisplay::Glx => glutin_winit::ApiPreference::FallbackEgl,
            });
        let (mut window, gl_config) = display_builder
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

        let display = gl_config.display();
        let raw_window_handle = window
            .as_ref()
            .map(|window| {
                window
                    .window_handle()
                    .map(|handle| handle.as_raw())
                    .map_err(|error| GuiError(format!("failed to obtain window handle: {error}")))
            })
            .transpose()?;
        #[cfg(not(target_os = "macos"))]
        let desktop_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::OpenGl(Some(
                glutin::context::Version::new(2, 0),
            )))
            .build(raw_window_handle);
        #[cfg(target_os = "macos")]
        let context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::OpenGl(Some(
                glutin::context::Version::new(3, 2),
            )))
            .with_profile(glutin::context::GlProfile::Core)
            .build(raw_window_handle);
        #[cfg(not(target_os = "macos"))]
        let gles_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(Some(
                glutin::context::Version::new(2, 0),
            )))
            .build(raw_window_handle);

        // SAFETY: The attributes use the live window handle returned by winit and
        // the context remains owned alongside that window and display.
        #[cfg(not(target_os = "macos"))]
        let not_current = match config.gui.graphics.api {
            GraphicsApi::Opengl => {
                // SAFETY: The attributes use the live window handle and GL config.
                unsafe { display.create_context(&gl_config, &desktop_attributes) }.map_err(
                    |error| GuiError(format!("failed to create an OpenGL 2.0 context: {error}")),
                )?
            }
            GraphicsApi::Gles => {
                // SAFETY: The attributes use the live window handle and GL config.
                unsafe { display.create_context(&gl_config, &gles_attributes) }.map_err(
                    |error| {
                        GuiError(format!(
                            "failed to create an OpenGL ES 2.0 context: {error}"
                        ))
                    },
                )?
            }
            GraphicsApi::Auto => {
                // SAFETY: Both attribute sets use the live window handle and GL config.
                unsafe { display.create_context(&gl_config, &desktop_attributes) }.or_else(
                    |desktop_error| {
                        // SAFETY: This uses the same live window handle and GL config.
                        unsafe { display.create_context(&gl_config, &gles_attributes) }.map_err(
                            |gles_error| {
                                GuiError(format!(
                                    "failed to create an OpenGL 2.0 context ({desktop_error}) \
                                     or an OpenGL ES 2.0 context ({gles_error})"
                                ))
                            },
                        )
                    },
                )?
            }
        };
        #[cfg(target_os = "macos")]
        // SAFETY: The attributes use the live AppKit window handle returned by
        // winit, and the CGL context remains owned alongside that window.
        let not_current = {
            if config.gui.graphics.api == GraphicsApi::Gles {
                return Err(GuiError(
                    "OpenGL ES contexts are not supported by the macOS GUI backend".into(),
                ));
            }
            unsafe { display.create_context(&gl_config, &context_attributes) }.map_err(|error| {
                GuiError(format!(
                    "failed to create an OpenGL 3.2 Core context: {error}"
                ))
            })?
        };

        let window = if let Some(window) = window.take() {
            window
        } else {
            glutin_winit::finalize_window(event_loop, window_attributes, &gl_config)
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
                .create_window_surface(&gl_config, &surface_attributes)
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
        let swap_interval = if config.gui.graphics.vsync {
            glutin::surface::SwapInterval::Wait(NonZeroU32::MIN)
        } else {
            glutin::surface::SwapInterval::DontWait
        };
        let _ = surface.set_swap_interval(&context, swap_interval);

        Ok(Self {
            surface,
            context,
            display,
            window,
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
    config: Arc<crate::config::AppConfig>,
}

impl GlowApplication {
    fn new(
        proxy: winit::event_loop::EventLoopProxy<UserEvent>,
        config: crate::config::AppConfig,
    ) -> Self {
        Self {
            proxy,
            gl_window: None,
            gl: None,
            egui: None,
            frontend: None,
            error: None,
            exit_after_first_frame: std::env::var_os("CEFDETECTOR_GUI_SMOKE_TEST").is_some(),
            config: Arc::new(config),
        }
    }

    fn initialize(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> Result<(), GuiError> {
        let gl_window = GlutinWindow::new(event_loop, &self.config)?;
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
        let mut egui = EguiRenderer::new(
            event_loop,
            Arc::clone(&gl),
            self.config.diagnostics.report_graphics,
        )?;

        let proxy = self.proxy.clone();
        egui.egui_ctx.set_request_repaint_callback(move |request| {
            let _ = proxy.send_event(UserEvent::Repaint(request.delay));
        });
        let frontend = match Frontend::new(&egui.egui_ctx, Arc::clone(&self.config)) {
            Ok(frontend) => frontend,
            Err(error) => {
                egui.destroy();
                return Err(error);
            }
        };

        self.frontend = Some(frontend);
        self.egui = Some(egui);
        self.gl = Some(gl);
        // A hidden Win32 window does not reliably receive its initial paint
        // request. Reveal it only after initialization has succeeded, then
        // request the first frame.
        gl_window.window.set_visible(true);
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
        // Drop UI textures while the GL context is still current, then release
        // the renderer, GL loader, native surface, and window in that order.
        self.frontend.take();
        if let Some(mut egui) = self.egui.take() {
            egui.destroy();
        }
        self.gl.take();
        self.gl_window.take();
    }
}

pub fn run(config: crate::config::AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = GlowApplication::new(proxy, config);
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(Box::new(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        REPOSITORY_TEXT, SEARCHING_TEXT, TextRole, configure_embedded_fonts, format_size,
        scroll_from_thumb_drag, title_font_size,
    };

    #[test]
    fn embedded_font_subsets_stay_small() {
        assert!(include_bytes!("../fonts/title-subset.ttf").len() <= 100 * 1024);
        assert!(include_bytes!("../fonts/card-regular-subset.ttf").len() <= 20 * 1024);
        assert!(include_bytes!("../fonts/card-bold-subset.ttf").len() <= 20 * 1024);
    }

    #[test]
    fn embedded_fonts_cover_fixed_ui_text_but_not_cjk_card_names() {
        let ctx = egui::Context::default();
        let configured = configure_embedded_fonts(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let title = egui::FontId::new(18.0, configured.family(TextRole::Title));
        let card = egui::FontId::new(11.0, configured.family(TextRole::CardRegular));
        let footer = egui::FontId::new(12.0, configured.family(TextRole::Footer));

        ctx.fonts_mut(|fonts| {
            assert!(fonts.has_glyphs(&title, SEARCHING_TEXT));
            assert!(fonts.has_glyphs(
                &title,
                "搜索完成！这台电脑上总共有 0123456789 个 Chromium 内核的应用 (1.23 GB)"
            ));
            assert!(fonts.has_glyphs(&footer, REPOSITORY_TEXT));
            assert!(fonts.has_glyphs(
                &card,
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789.… "
            ));
            assert!(!fonts.has_glyph(&card, '中'));
        });
    }

    #[test]
    fn truncated_card_names_keep_their_layout_width() {
        let ctx = egui::Context::default();
        let configured = configure_embedded_fonts(&ctx);
        let mut size = egui::Vec2::ZERO;
        let mut elided = false;
        let mut final_character = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let galley = super::layout_elided(
                ui.painter(),
                "CefSharp.BrowserSubprocess.exe",
                egui::FontId::new(11.0, configured.family(TextRole::CardBold)),
                egui::Color32::BLACK,
                76.0,
            );
            size = galley.size();
            elided = galley.elided;
            final_character = galley
                .rows
                .last()
                .and_then(|row| row.row.glyphs.last())
                .map(|glyph| glyph.chr);
        });

        assert!(elided);
        assert_eq!(final_character, Some('…'));
        assert!(size.x > 0.0);
        assert!(size.x <= 76.0);
    }

    #[test]
    fn size_format_matches_the_original_ui() {
        let config = crate::config::SizeFormatConfig::default();
        assert_eq!(format_size(0, &config), "0.00 B");
        assert_eq!(format_size(1024, &config), "1.00 KB");
        assert_eq!(format_size(1536, &config), "1.50 KB");
    }

    #[test]
    fn title_font_tracks_proportional_window_growth() {
        let status = crate::config::StatusConfig::default();
        let window = crate::config::WindowConfig::default();
        let size = |width, height| title_font_size(egui::vec2(width, height), &status, &window);
        assert_eq!(size(800.0, 600.0), 18.0);
        assert_eq!(size(1_600.0, 1_200.0), 36.0);
        assert_eq!(size(1_600.0, 600.0), 18.0);
        assert_eq!(size(400.0, 300.0), 12.0);
        assert_eq!(size(8_000.0, 6_000.0), 64.0);
    }

    #[test]
    fn scrollbar_drag_uses_total_pointer_displacement() {
        assert_eq!(scroll_from_thumb_drag(20.0, 30.0, 200.0, 100.0), 80.0);
        assert_eq!(scroll_from_thumb_drag(20.0, -30.0, 200.0, 100.0), 0.0);
    }
}

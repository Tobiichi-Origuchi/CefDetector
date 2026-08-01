use std::collections::HashMap;
use std::ffi::{c_double, c_long, c_uchar, c_uint, c_void};

use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions, Vec2};

const SYSTEM_FONT: u32 = 2;
const EMPHASIZED_SYSTEM_FONT: u32 = 3;
const UTF8_ENCODING: u32 = 0x0800_0100;
const TRUNCATE_END: u32 = 1;
const BITMAP_RGBA_PREMULTIPLIED: u32 = (4 << 12) | 1;
const TEXTURE_PADDING: usize = 2;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFAttributedStringRef = *const c_void;
type CTFontRef = *const c_void;
type CTLineRef = *const c_void;
type CGColorRef = *const c_void;
type CGColorSpaceRef = *const c_void;
type CGContextRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: c_double,
    b: c_double,
    c: c_double,
    d: c_double,
    tx: c_double,
    ty: c_double,
}

const IDENTITY: CGAffineTransform = CGAffineTransform {
    a: 1.0,
    b: 0.0,
    c: 0.0,
    d: 1.0,
    tx: 0.0,
    ty: 0.0,
};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_uchar;
    static kCFTypeDictionaryValueCallBacks: c_uchar;

    fn CFRelease(value: CFTypeRef);
    fn CFStringCreateWithBytes(
        allocator: CFTypeRef,
        bytes: *const u8,
        count: c_long,
        encoding: c_uint,
        is_external_representation: c_uchar,
    ) -> CFStringRef;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        count: c_long,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFAttributedStringCreate(
        allocator: CFTypeRef,
        text: CFStringRef,
        attributes: CFDictionaryRef,
    ) -> CFAttributedStringRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorCreateGenericRGB(
        red: c_double,
        green: c_double,
        blue: c_double,
        alpha: c_double,
    ) -> CGColorRef;
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        color_space: CGColorSpaceRef,
        bitmap_info: c_uint,
    ) -> CGContextRef;
    fn CGContextSetTextMatrix(context: CGContextRef, transform: CGAffineTransform);
    fn CGContextSetTextPosition(context: CGContextRef, x: c_double, y: c_double);
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    static kCTFontAttributeName: CFStringRef;
    static kCTForegroundColorAttributeName: CFStringRef;

    fn CTFontCreateUIFontForLanguage(
        ui_type: c_uint,
        size: c_double,
        language: CFStringRef,
    ) -> CTFontRef;
    fn CTLineCreateWithAttributedString(text: CFAttributedStringRef) -> CTLineRef;
    fn CTLineCreateTruncatedLine(
        line: CTLineRef,
        width: c_double,
        truncation_type: c_uint,
        truncation_token: CTLineRef,
    ) -> CTLineRef;
    fn CTLineGetTypographicBounds(
        line: CTLineRef,
        ascent: *mut c_double,
        descent: *mut c_double,
        leading: *mut c_double,
    ) -> c_double;
    fn CTLineDraw(line: CTLineRef, context: CGContextRef);
}

struct OwnedCf(CFTypeRef);

impl OwnedCf {
    fn new(value: CFTypeRef, description: &str) -> Result<Self, String> {
        if value.is_null() {
            Err(format!("CoreText failed to create {description}"))
        } else {
            Ok(Self(value))
        }
    }

    fn as_ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: Every OwnedCf is created by a Core Foundation create/copy
        // function and owns exactly one retain count.
        unsafe { CFRelease(self.0) };
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct TextKey {
    text: String,
    point_size: u32,
    bold: bool,
    color: [u8; 4],
    max_width: Option<u32>,
    pixels_per_point: u32,
}

#[derive(Clone)]
pub(super) struct NativeText {
    pub(super) texture: TextureHandle,
    pub(super) size: Vec2,
}

pub(super) struct CoreTextRenderer {
    cache: HashMap<TextKey, NativeText>,
    next_texture_id: u64,
}

impl CoreTextRenderer {
    pub(super) fn new() -> Self {
        Self {
            cache: HashMap::new(),
            next_texture_id: 0,
        }
    }

    pub(super) fn layout(
        &mut self,
        ctx: &Context,
        text: &str,
        point_size: f32,
        bold: bool,
        color: Color32,
        max_width: Option<f32>,
    ) -> NativeText {
        let pixels_per_point = ctx.pixels_per_point().max(1.0);
        let key = TextKey {
            text: text.to_owned(),
            point_size: point_size.to_bits(),
            bold,
            color: color.to_array(),
            max_width: max_width.map(f32::to_bits),
            pixels_per_point: pixels_per_point.to_bits(),
        };
        if let Some(text) = self.cache.get(&key) {
            return text.clone();
        }

        let (image, size) =
            match render_text(text, point_size, bold, color, max_width, pixels_per_point) {
                Ok(rendered) => rendered,
                Err(error) => {
                    eprintln!("{error}");
                    (ColorImage::filled([1, 1], Color32::TRANSPARENT), Vec2::ZERO)
                }
            };
        let texture = ctx.load_texture(
            format!("coretext-{}", self.next_texture_id),
            image,
            TextureOptions::LINEAR,
        );
        self.next_texture_id += 1;
        let rendered = NativeText { texture, size };
        self.cache.insert(key, rendered.clone());
        rendered
    }
}

fn render_text(
    text: &str,
    point_size: f32,
    bold: bool,
    color: Color32,
    max_width: Option<f32>,
    pixels_per_point: f32,
) -> Result<(ColorImage, Vec2), String> {
    let scale = pixels_per_point as f64;
    let pixel_size = point_size as f64 * scale;
    let max_pixel_width = max_width.map(|width| width.max(0.0) as f64 * scale);

    let text = create_string(text)?;
    let font_type = if bold {
        EMPHASIZED_SYSTEM_FONT
    } else {
        SYSTEM_FONT
    };
    // SAFETY: Arguments follow CTFontCreateUIFontForLanguage's contract. A
    // null language asks CoreText to use the current system preferences.
    let font = unsafe {
        OwnedCf::new(
            CTFontCreateUIFontForLanguage(font_type, pixel_size, std::ptr::null()),
            "system UI font",
        )?
    };
    let rgba = color.to_array().map(|channel| channel as f64 / 255.0);
    // SAFETY: Component values are finite and in the documented 0...1 range.
    let foreground = unsafe {
        OwnedCf::new(
            CGColorCreateGenericRGB(rgba[0], rgba[1], rgba[2], rgba[3]),
            "foreground color",
        )?
    };

    let keys = unsafe { [kCTFontAttributeName, kCTForegroundColorAttributeName] };
    let values = [font.as_ptr(), foreground.as_ptr()];
    // SAFETY: Keys and values are valid Core Foundation objects, and the
    // standard callbacks retain them for the dictionary lifetime.
    let attributes = unsafe {
        OwnedCf::new(
            CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                keys.len() as c_long,
                std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            ),
            "text attributes",
        )?
    };
    let attributed = unsafe {
        OwnedCf::new(
            CFAttributedStringCreate(std::ptr::null(), text.as_ptr(), attributes.as_ptr()),
            "attributed string",
        )?
    };
    let full_line = unsafe {
        OwnedCf::new(
            CTLineCreateWithAttributedString(attributed.as_ptr()),
            "text line",
        )?
    };

    let ellipsis = create_string("…")?;
    let ellipsis_attributed = unsafe {
        OwnedCf::new(
            CFAttributedStringCreate(std::ptr::null(), ellipsis.as_ptr(), attributes.as_ptr()),
            "ellipsis attributed string",
        )?
    };
    let ellipsis_line = unsafe {
        OwnedCf::new(
            CTLineCreateWithAttributedString(ellipsis_attributed.as_ptr()),
            "ellipsis line",
        )?
    };

    let truncated_line = max_pixel_width.and_then(|width| {
        // SAFETY: Both line references are live and width is non-negative.
        let line = unsafe {
            CTLineCreateTruncatedLine(
                full_line.as_ptr(),
                width,
                TRUNCATE_END,
                ellipsis_line.as_ptr(),
            )
        };
        (!line.is_null()).then_some(OwnedCf(line))
    });
    let line = truncated_line.as_ref().unwrap_or(&full_line);

    let mut ascent = 0.0;
    let mut descent = 0.0;
    let mut leading = 0.0;
    // SAFETY: The output pointers are valid and line owns a live CTLine.
    let typographic_width = unsafe {
        CTLineGetTypographicBounds(line.as_ptr(), &mut ascent, &mut descent, &mut leading)
    };
    let logical_pixel_width = max_pixel_width
        .map(|maximum| typographic_width.min(maximum))
        .unwrap_or(typographic_width)
        .max(0.0);
    let width = logical_pixel_width.ceil() as usize + TEXTURE_PADDING * 2;
    let height = (ascent + descent + leading).ceil() as usize + TEXTURE_PADDING * 2;
    let width = width.max(1);
    let height = height.max(1);
    let bytes_per_row = width * 4;
    let mut pixels = vec![0_u8; bytes_per_row * height];

    // SAFETY: The bitmap context borrows the stable allocation in `pixels` and
    // is released before that allocation is accessed or dropped.
    let color_space = unsafe { OwnedCf::new(CGColorSpaceCreateDeviceRGB(), "RGB color space")? };
    let bitmap = unsafe {
        OwnedCf::new(
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                bytes_per_row,
                color_space.as_ptr(),
                BITMAP_RGBA_PREMULTIPLIED,
            )
            .cast_const(),
            "bitmap context",
        )?
    };
    let context = bitmap.as_ptr().cast_mut();
    unsafe {
        CGContextSetTextMatrix(context, IDENTITY);
        CGContextSetTextPosition(
            context,
            TEXTURE_PADDING as f64,
            TEXTURE_PADDING as f64 + descent,
        );
        CTLineDraw(line.as_ptr(), context);
    }
    drop(bitmap);

    // The bitmap stores premultiplied RGBA; egui expects unmultiplied sRGBA.
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha != 0 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }

    let image = ColorImage::from_rgba_unmultiplied([width, height], &pixels);
    let size = Vec2::new(
        width as f32 / pixels_per_point,
        height as f32 / pixels_per_point,
    );
    Ok((image, size))
}

fn create_string(text: &str) -> Result<OwnedCf, String> {
    // SAFETY: The byte pointer and length describe the live UTF-8 string.
    unsafe {
        OwnedCf::new(
            CFStringCreateWithBytes(
                std::ptr::null(),
                text.as_ptr(),
                text.len() as c_long,
                UTF8_ENCODING,
                0,
            ),
            "UTF-8 string",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::render_text;
    use egui::Color32;

    #[test]
    fn core_text_renders_mixed_chinese_and_latin_glyphs() {
        let (image, size) =
            render_text("搜索完成 Chromium", 18.0, true, Color32::WHITE, None, 1.0).unwrap();

        assert!(size.x > 100.0);
        assert!(size.y > 10.0);
        assert!(image.pixels.iter().any(|pixel| pixel.a() != 0));
    }

    #[test]
    fn core_text_truncates_to_the_requested_width() {
        let (_, size) = render_text(
            "A very long application filename.app",
            11.0,
            true,
            Color32::BLACK,
            Some(76.0),
            1.0,
        )
        .unwrap();

        assert!(size.x <= 80.0);
    }
}

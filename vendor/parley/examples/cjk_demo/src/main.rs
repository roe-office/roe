// CJK 断行 + 合成斜体/下划线/删除线测试 demo：用 swash 渲染管线验证 Parley 真实字形效果
#![allow(clippy::all)]

use image::codecs::png::PngEncoder;
use image::{Pixel, Rgba, RgbaImage};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontStyle, GenericFamily, Glyph, GlyphRun,
    Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use std::fs::File;
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Scaler, Source, StrikeWith};
use swash::zeno;
use zeno::{Format, Transform, Vector};

#[derive(Clone, Copy, Debug, PartialEq)]
struct ColorBrush {
    color: Rgba<u8>,
}
impl Default for ColorBrush {
    fn default() -> Self {
        Self { color: Rgba([0, 0, 0, 255]) }
    }
}

/// 把「第几个char」转换成字节偏移，避免UTF-8边界问题
fn char_idx_to_byte(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(text.len())
}

fn main() {
    let mut text = std::fs::read_to_string("/tmp/parley_demo/sample_text.txt")
        .expect("找不到 /tmp/parley_demo/sample_text.txt");
    let western_test_str = " ENGLISH ITALIC TEST Hello World Testing 123";
    let western_start = text.len();
    text.push_str(western_test_str);
    let western_end = text.len();

    let display_scale = 1.0;
    let quantize = true;
    let max_advance = Some(468.0);
    let foreground_color = Rgba([0u8, 0, 0, 255]);
    let background_color = Rgba([255u8, 255, 255, 255]);
    let padding = 20u32;

    let mut font_cx = FontContext::new();
    let mut layout_cx = LayoutContext::new();
    let mut scale_cx = ScaleContext::new();

    let mut builder = layout_cx.ranged_builder(&mut font_cx, &text, display_scale, quantize);

    let brush_style = StyleProperty::Brush(ColorBrush { color: foreground_color });
    builder.push_default(brush_style);
    builder.push_default(GenericFamily::SystemUi);
    builder.push_default(LineHeight::FontSizeRelative(1.3));
    builder.push_default(StyleProperty::FontSize(16.0));

    // === 三段独立样式测试区间，均取自纯中文段落，互不重叠 ===
    // 1) 0..60 字符：斜体（核心验证目标：CJK合成斜体）
    let italic_start = char_idx_to_byte(&text, 0);
    let italic_end = char_idx_to_byte(&text, 60);
    builder.push(StyleProperty::FontStyle(FontStyle::Italic), italic_start..italic_end);
    println!("斜体区间: char 0..60 -> byte {}..{}", italic_start, italic_end);

    // === 西文斜体对照测试。在文本末尾追加的英文段，单独设置斜体，
    //     用于排除"西文本身也不斜"这种更基础的问题 ===
    builder.push(StyleProperty::FontStyle(FontStyle::Italic), western_start..western_end);
    println!("西文斜体对照区间: byte {}..{}", western_start, western_end);

    // 2) 65..85 字符：下划线
    let underline_start = char_idx_to_byte(&text, 65);
    let underline_end = char_idx_to_byte(&text, 85);
    builder.push(StyleProperty::Underline(true), underline_start..underline_end);
    println!("下划线区间: char 65..85 -> byte {}..{}", underline_start, underline_end);

    // 3) 90..110 字符：删除线
    let strike_start = char_idx_to_byte(&text, 90);
    let strike_end = char_idx_to_byte(&text, 110);
    builder.push(StyleProperty::Strikethrough(true), strike_start..strike_end);
    println!("删除线区间: char 90..110 -> byte {}..{}", strike_start, strike_end);

    let mut layout: Layout<ColorBrush> = builder.build(&text);
    layout.break_all_lines(max_advance);
    layout.align(Alignment::Start, AlignmentOptions::default());

    let width = layout.width().ceil() as u32 + padding * 2;
    let height = layout.height().ceil() as u32 + padding * 2;
    let mut img = RgbaImage::from_pixel(width, height, background_color);

    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                render_glyph_run(&mut scale_cx, &glyph_run, &mut img, padding);
            }
        }
    }

    let output_path = "/tmp/parley_demo/cjk_output_swash.png";
    let output_file = File::create(output_path).unwrap();
    let png_encoder = PngEncoder::new(output_file);
    img.write_with_encoder(png_encoder).unwrap();
    println!("渲染完成: {} ({}x{})", output_path, width, height);
}

fn render_glyph_run(
    context: &mut ScaleContext,
    glyph_run: &GlyphRun<'_, ColorBrush>,
    img: &mut RgbaImage,
    padding: u32,
) {
    let mut run_x = glyph_run.offset();
    let run_y = glyph_run.baseline();
    let style = glyph_run.style();
    let color = style.brush;

    let run = glyph_run.run();
    let font = run.font();
    let font_size = run.font_size();
    let normalized_coords = run.normalized_coords();

    // === 核心：读取 Parley/fontique 建议的合成样式（skew用于faux italic） ===
    let synthesis = run.synthesis();
    let skew_transform: Option<Transform> = synthesis.skew().map(|angle_degrees| {
        let radians = angle_degrees.to_radians();
        // 标准 faux italic 斜切矩阵：沿x方向按y坐标做水平斜切
        Transform {
            xx: 1.0,
            xy: -radians.tan(),
            yx: 0.0,
            yy: 1.0,
            x: 0.0,
            y: 0.0,
        }
    });
    eprintln!(
        "[DEBUG skew] run.offset={} synthesis.any()={} skew_angle={:?} skew_transform={:?}",
        glyph_run.offset(), synthesis.any(), synthesis.skew(), skew_transform
    );

    let font_ref = FontRef::from_index(font.font.data.as_ref(), font.font.index as usize).unwrap();

    let mut scaler = context
        .builder(font_ref)
        .size(font_size)
        .hint(true)
        .normalized_coords(normalized_coords.iter().map(|c| c.to_bits()))
        .build();

    for glyph in glyph_run.glyphs() {
        let glyph_x = run_x + glyph.x + (padding as f32);
        let glyph_y = run_y + glyph.y + (padding as f32);
        run_x += glyph.advance;

        render_glyph(img, &mut scaler, color, glyph, glyph_x, glyph_y, skew_transform);
    }

    // 绘制下划线/删除线装饰
    let run_metrics = run.font_metrics();
    if let Some(decoration) = &style.underline {
        let offset = decoration.offset.unwrap_or(run_metrics.underline_offset);
        let size = decoration.size.unwrap_or(run_metrics.underline_size);
        render_decoration(img, glyph_run, decoration.brush, offset, size, padding);
    }
    if let Some(decoration) = &style.strikethrough {
        let offset = decoration.offset.unwrap_or(run_metrics.strikethrough_offset);
        let size = decoration.size.unwrap_or(run_metrics.strikethrough_size);
        render_decoration(img, glyph_run, decoration.brush, offset, size, padding);
    }
}

fn render_decoration(
    img: &mut RgbaImage,
    glyph_run: &GlyphRun<'_, ColorBrush>,
    brush: ColorBrush,
    offset: f32,
    width: f32,
    padding: u32,
) {
    let y = glyph_run.baseline() - offset;
    let img_w = img.width();
    let img_h = img.height();
    let x_start = glyph_run.offset() as u32 + padding;
    let x_end = (glyph_run.offset() + glyph_run.advance()) as u32 + padding;
    let y_start = y as u32 + padding;
    // 用 ceil 保证至少 1px 高，修掉之前 underline 因取整巧合导致空区间的问题
    let y_end = (y + width).ceil() as u32 + padding;
    for pixel_y in y_start..y_end.max(y_start + 1) {
        for pixel_x in x_start..x_end {
            if pixel_x < img_w && pixel_y < img_h {
                img.get_pixel_mut(pixel_x, pixel_y).blend(&brush.color);
            }
        }
    }
}

fn render_glyph(
    img: &mut RgbaImage,
    scaler: &mut Scaler<'_>,
    brush: ColorBrush,
    glyph: Glyph,
    glyph_x: f32,
    glyph_y: f32,
    skew_transform: Option<Transform>,
) {
    let offset = Vector::new(glyph_x.fract(), glyph_y.fract());

    let rendered_glyph = Render::new(&[
        Source::Outline,
    ])
    .format(Format::Alpha)
    .offset(offset)
    .transform(skew_transform)
    .render(scaler, glyph.id as u16)
    .unwrap();

    let glyph_width = rendered_glyph.placement.width;
    let glyph_height = rendered_glyph.placement.height;
    let glyph_x = (glyph_x.floor() as i32 + rendered_glyph.placement.left) as u32;
    let glyph_y = (glyph_y.floor() as i32 - rendered_glyph.placement.top) as u32;

    match rendered_glyph.content {
        Content::Mask => {
            let mut i = 0;
            let bc = brush.color;
            for pixel_y in 0..glyph_height {
                for pixel_x in 0..glyph_width {
                    let x = glyph_x + pixel_x;
                    let y = glyph_y + pixel_y;
                    let alpha = rendered_glyph.data[i];
                    let color = Rgba([bc[0], bc[1], bc[2], alpha]);
                    img.get_pixel_mut(x, y).blend(&color);
                    i += 1;
                }
            }
        }
        Content::SubpixelMask => unimplemented!(),
        Content::Color => {
            let row_size = glyph_width as usize * 4;
            for (pixel_y, row) in rendered_glyph.data.chunks_exact(row_size).enumerate() {
                for (pixel_x, pixel) in row.chunks_exact(4).enumerate() {
                    let x = glyph_x + pixel_x as u32;
                    let y = glyph_y + pixel_y as u32;
                    let color = Rgba(pixel.try_into().expect("Not RGBA"));
                    img.get_pixel_mut(x, y).blend(&color);
                }
            }
        }
    }
}

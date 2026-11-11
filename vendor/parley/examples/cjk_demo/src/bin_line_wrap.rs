// 逐行变宽度最小demo：验证 BreakLines::break_next() + set_line_max_advance()
// 能否在同一个 Layout 内，让不同的行使用不同的最大宽度（模拟环绕排版挤占效果）
#![allow(clippy::all)]

use image::codecs::png::PngEncoder;
use image::{Pixel, Rgba, RgbaImage};
use parley::{
    Alignment, AlignmentOptions, FontContext, GenericFamily, Glyph, GlyphRun,
    Layout, LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use std::fs::File;
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Scaler, Source, StrikeWith};
use swash::zeno;
use zeno::{Format, Vector};

#[derive(Clone, Copy, Debug, PartialEq)]
struct ColorBrush {
    color: Rgba<u8>,
}
impl Default for ColorBrush {
    fn default() -> Self {
        Self { color: Rgba([0, 0, 0, 255]) }
    }
}

fn main() {
    let text = std::fs::read_to_string("/tmp/parley_demo/sample_text.txt")
        .expect("找不到 /tmp/parley_demo/sample_text.txt");

    let display_scale = 1.0;
    let quantize = true;
    let full_width = 468.0f32;
    // 模拟一张"图片"占据 y: 100..300 这段范围，挤占左侧空间，
    // 该区间内可用宽度收窄到 250（相当于图片宽度约218pt+间距）
    let obstacle_y_start = 100.0f64;
    let obstacle_y_end = 300.0f64;
    let narrowed_width = 250.0f32;

    let foreground_color = Rgba([0u8, 0, 0, 255]);
    let background_color = Rgba([255u8, 255, 255, 255]);
    let padding = 20u32;

    let mut font_cx = FontContext::new();
    let mut layout_cx = LayoutContext::new();
    let mut scale_cx = ScaleContext::new();

    let mut builder = layout_cx.ranged_builder(&mut font_cx, &text, display_scale, quantize);
    builder.push_default(StyleProperty::Brush(ColorBrush { color: foreground_color }));
    builder.push_default(GenericFamily::SystemUi);
    builder.push_default(LineHeight::FontSizeRelative(1.3));
    builder.push_default(StyleProperty::FontSize(16.0));

    let mut layout: Layout<ColorBrush> = builder.build(&text);

    // === 核心：手动驱动 BreakLines 状态机，逐行判断该用哪个宽度 ===
    {
        let mut breaker = layout.break_lines();
        // 关键修正：必须先设置全局宽度上限，逐行的 set_line_max_advance
        // 只能在这个上限以内收窄，不能超过（否则触发断言panic）
        breaker.state_mut().set_layout_max_advance(full_width);
        let mut line_count = 0usize;
        loop {
            let current_y = breaker.committed_y();
            let use_narrow = current_y >= obstacle_y_start && current_y < obstacle_y_end;
            let width_for_this_line = if use_narrow { narrowed_width } else { full_width };
            breaker.state_mut().set_line_max_advance(width_for_this_line);

            eprintln!(
                "[DEBUG line_wrap] line#{} committed_y={:.1} use_narrow={} width={}",
                line_count, current_y, use_narrow, width_for_this_line
            );

            match breaker.break_next() {
                Some(_yield_data) => {
                    line_count += 1;
                }
                None => break,
            }
        }
        breaker.finish();
        println!("总行数: {}", line_count);
    }

    layout.align(Alignment::Start, AlignmentOptions::default());

    let width = layout.width().ceil() as u32 + padding * 2;
    let height = layout.height().ceil() as u32 + padding * 2;
    let mut img = RgbaImage::from_pixel(width, height, background_color);

    // 画一个半透明灰框标出"障碍物"区域，方便肉眼核对哪几行确实变窄了
    for y in (obstacle_y_start as u32 + padding)..(obstacle_y_end as u32 + padding).min(height) {
        for x in padding..(narrowed_width as u32 + 30 + padding).min(width) {
            if x >= (narrowed_width as u32 + padding) {
                img.get_pixel_mut(x, y).blend(&Rgba([200, 200, 200, 120]));
            }
        }
    }

    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                render_glyph_run(&mut scale_cx, &glyph_run, &mut img, padding);
            }
        }
    }

    let output_path = "/tmp/parley_demo/line_wrap_output.png";
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

        render_glyph(img, &mut scaler, color, glyph, glyph_x, glyph_y);
    }
}

fn render_glyph(
    img: &mut RgbaImage,
    scaler: &mut Scaler<'_>,
    brush: ColorBrush,
    glyph: Glyph,
    glyph_x: f32,
    glyph_y: f32,
) {
    let offset = Vector::new(glyph_x.fract(), glyph_y.fract());

    let rendered_glyph = Render::new(&[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ])
    .format(Format::Alpha)
    .offset(offset)
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
                    if x < img.width() && y < img.height() {
                        img.get_pixel_mut(x, y).blend(&color);
                    }
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
                    if x < img.width() && y < img.height() {
                        img.get_pixel_mut(x, y).blend(&color);
                    }
                }
            }
        }
    }
}

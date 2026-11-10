//! 内核级"文档导出为 PNG"：Writer 每页一张、Impress 每张幻灯片一张、Calc 按行列范围一张。
//! 只负责出图并返回 PNG 字节，不写文件；写入 exports 目录由上层（脚本后处理）负责。
//! 所有尺寸都有硬上限：单图边长 MAX_EDGE_PX，单次张数 MAX_IMAGES，Calc 100 行 × 30 列。

use crate::handle::{self, DocumentEntry};
use roe_impress::slide_renderer::SlideRenderer;
use roe_parser::RoeDocument;
use roe_writer::font_registry::FontRegistry; // 路径待编译核对
use roe_writer::tile::WriterTileRenderer;

pub const MAX_IMAGES: usize = 20;
pub const MAX_EDGE_PX: u32 = 2400;
pub const CALC_DEFAULT_ROWS: u32 = 40;
pub const CALC_DEFAULT_COLS: u32 = 20;
pub const CALC_MAX_ROWS: u32 = 100;
pub const CALC_MAX_COLS: u32 = 30;

#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// 像素倍率，默认 1.5，上限 3.0，并受 MAX_EDGE_PX 约束。
    pub scale: f32,
    /// Writer 页码 / Impress 幻灯片序号范围，1 起，闭区间。
    pub from: usize,
    pub to: usize,
    /// Calc：工作表序号（0 起）。
    pub sheet: usize,
    /// Calc：行数、列数（从 A1 起），会被夹到硬上限内。
    pub rows: u32,
    pub cols: u32,
    /// Calc：是否画行列号。
    pub headers: bool,
    /// Calc：true 时严格按 rows x cols 截取,不抬高下限、不自动扩到浮动对象。
    pub exact: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            scale: 1.5,
            from: 1,
            to: usize::MAX,
            sheet: 0,
            rows: CALC_DEFAULT_ROWS,
            cols: CALC_DEFAULT_COLS,
            headers: true,
            exact: false,
        }
    }
}

/// Calc 图里实际画出的行列数(从 A1 起)。truncated=true 表示比请求的少(受单图尺寸上限收缩)。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CalcRange {
    pub rows: u32,
    pub cols: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ExportedImage {
    pub calc_range: Option<CalcRange>,
    /// 供上层命名用的短标签，如 page1 / slide2 / sheet1_Sheet1。
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// RGBA（预乘）缓冲 -> PNG。Writer / Impress / Calc 共用。
pub fn encode_png(buf: Vec<u8>, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let size = tiny_skia::IntSize::from_wh(w, h).ok_or("图片尺寸无效")?;
    let pm = tiny_skia::Pixmap::from_vec(buf, size).ok_or("缓冲区长度与尺寸不符")?;
    pm.encode_png().map_err(|e| format!("PNG 编码失败: {e}"))
}

/// 把倍率夹到 (0, 3.0]，并保证 w*scale、h*scale 都不超过 MAX_EDGE_PX。
pub fn clamp_scale(scale: f32, w: f32, h: f32) -> f32 {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.5 };
    let m = w.max(h).max(1.0);
    s.min(3.0).min(MAX_EDGE_PX as f32 / m)
}

/// 1 起闭区间 [from,to] -> 0 起半开区间 [start,end)，并限制张数不超过 MAX_IMAGES。
pub fn page_window(total: usize, from: usize, to: usize) -> (usize, usize) {
    let start = from.max(1) - 1;
    let end = to.min(total).min(start.saturating_add(MAX_IMAGES));
    (start.min(end), end)
}

fn short_name(name: &str) -> String {
    name.chars()
        .take(30)
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn export_writer(entry: &mut DocumentEntry, o: &ExportOptions) -> Result<Vec<ExportedImage>, String> {
    let RoeDocument::Writer(doc) = &entry.doc else {
        return Err("非 Writer 文档".into());
    };
    let cache = entry.writer_layout_cache.as_ref().ok_or("布局缓存缺失")?;
    let arena = &cache.arena;
    let (s, e) = page_window(arena.page_order.len(), o.from, o.to);
    let font_context = entry
        .writer_parley_font_context
        .take()
        .unwrap_or_else(FontRegistry::build_font_context_with_katex);
    roe_writer::tile::set_show_marks(false);
    let mut renderer = WriterTileRenderer::new_with_fonts(font_context, ());
    let mut out = Vec::new();
    let mut err: Option<String> = None;
    for i in s..e {
        let pid = arena.page_order[i];
        let page = &arena.pages[pid];
        let sc = clamp_scale(o.scale, page.width, page.height);
        let pw = (page.width * sc) as u32;
        let ph = (page.height * sc) as u32;
        if pw == 0 || ph == 0 {
            continue;
        }
        let mut buf = vec![0u8; (pw as usize) * (ph as usize) * 4];
        let vp_y = arena.page_global_y(pid);
        if let Err(er) = renderer.render_viewport(arena, doc, 0.0, vp_y, page.width, page.height, pw, ph, &mut buf) {
            err = Some(format!("第 {} 页渲染失败: {:?}", i + 1, er));
            break;
        }
        match encode_png(buf, pw, ph) {
            Ok(png) => out.push(ExportedImage { label: format!("page{}", i + 1), width: pw, height: ph, png, calc_range: None }),
            Err(er) => {
                err = Some(er);
                break;
            }
        }
    }
    let (fc, _) = renderer.take_fonts();
    entry.writer_parley_font_context = Some(fc);
    roe_writer::tile::set_show_marks(entry.writer_options.show_marks);
    match err {
        Some(er) => Err(er),
        None => Ok(out),
    }
}

fn export_impress(entry: &mut DocumentEntry, o: &ExportOptions) -> Result<Vec<ExportedImage>, String> {
    let RoeDocument::Impress(p) = &entry.doc else {
        return Err("非 Impress 文档".into());
    };
    let (sw, sh) = p.slide_size;
    let (s, e) = page_window(p.slides.len(), o.from, o.to);
    let sc = clamp_scale(o.scale, sw, sh);
    let pw = (sw * sc) as u32;
    let ph = (sh * sc) as u32;
    if pw == 0 || ph == 0 {
        return Err("幻灯片尺寸无效".into());
    }
    let font_context = entry.impress_font_registry.take().unwrap_or_else(parley::FontContext::new);
    let mut renderer = SlideRenderer::new_with_fonts(sw, sh, font_context);
    let mut out = Vec::new();
    let mut err: Option<String> = None;
    for i in s..e {
        let mut buf = vec![0u8; (pw as usize) * (ph as usize) * 4];
        // 动画运行态传 None：导出的是静态最终画面
        if let Err(er) = renderer.render_slide(
            &p.slides[i],
            &p.styles,
            None,
            sc,
            &mut buf,
            &p.theme_colors,
            Some(&p.master_slides),
        ) {
            err = Some(format!("第 {} 张幻灯片渲染失败: {er}", i + 1));
            break;
        }
        match encode_png(buf, pw, ph) {
            Ok(png) => out.push(ExportedImage { label: format!("slide{}", i + 1), width: pw, height: ph, png, calc_range: None }),
            Err(er) => {
                err = Some(er);
                break;
            }
        }
    }
    entry.impress_font_registry = Some(renderer.take_fonts());
    match err {
        Some(er) => Err(er),
        None => Ok(out),
    }
}

fn shrink_to_fit(
    sheet: &roe_model::calc::Sheet,
    hw: f32,
    hh: f32,
    rows: &mut u32,
    cols: &mut u32,
    cfg: &roe_core::tile_geometry::CalcRenderConfig,
) {
    use roe_core::tile_geometry::{col_x_px, row_y_px};
    loop {
        let w = (hw + col_x_px(sheet, *cols, cfg)) as u32 + 1;
        let h = (hh + row_y_px(sheet, *rows, cfg)) as u32 + 1;
        if w <= MAX_EDGE_PX && h <= MAX_EDGE_PX {
            break;
        }
        if w > MAX_EDGE_PX && *cols > 1 {
            *cols -= 1;
        } else if h > MAX_EDGE_PX && *rows > 1 {
            *rows -= 1;
        } else {
            break;
        }
    }
}

/// 浮动对象(图表/统计图/图片/形状)右下角所需的行列数。返回 (行数, 列数, 是否超出上限)。
fn expand_for_objects(
    sheet: &roe_model::calc::Sheet,
    cfg: &roe_core::tile_geometry::CalcRenderConfig,
) -> (u32, u32, bool) {
    use roe_core::tile_geometry::{col_x_px, row_y_px};
    const PT_TO_PX: f32 = 96.0 / 72.0;
    let (mut right, mut bottom) = (0f32, 0f32);
    let mut add = |x: f32, y: f32, w: f32, h: f32| {
        right = right.max((x + w) * PT_TO_PX);
        bottom = bottom.max((y + h) * PT_TO_PX);
    };
    for c in &sheet.floating_charts { add(c.x_pt, c.y_pt, c.width_pt, c.height_pt); }
    for c in &sheet.floating_stat_charts { add(c.x_pt, c.y_pt, c.width_pt, c.height_pt); }
    for i in &sheet.floating_images { add(i.x_pt, i.y_pt, i.width_pt, i.height_pt); }
    for s in &sheet.floating_shapes { add(s.x_pt, s.y_pt, s.width_pt, s.height_pt); }
    let mut cols = (CALC_MAX_COLS, true);
    for n in 1..=CALC_MAX_COLS {
        if col_x_px(sheet, n, cfg) >= right - 1.0 { cols = (n, false); break; }
    }
    let mut rows = (CALC_MAX_ROWS, true);
    for n in 1..=CALC_MAX_ROWS {
        if row_y_px(sheet, n, cfg) >= bottom - 1.0 { rows = (n, false); break; }
    }
    (rows.0, cols.0, rows.1 || cols.1)
}

fn export_calc(entry: &mut DocumentEntry, o: &ExportOptions) -> Result<Vec<ExportedImage>, String> {
    use roe_core::row_height_measure::layout_measure_viewport;
    use roe_core::tile_geometry::{col_x_px, row_y_px};
    let RoeDocument::Calc(wb) = &mut entry.doc else {
        return Err("非 Calc 文档".into());
    };
    if o.sheet >= wb.sheets.len() {
        return Err(format!("工作表序号 {} 超出范围（共 {} 张）", o.sheet, wb.sheets.len()));
    }
    let cfg = Default::default();
    let (hw, hh) = if o.headers { (40.0f32, 22.0f32) } else { (0.0, 0.0) };
    // 最小 40x20,rows/cols 只能往大设;再自动扩到包含所有浮动对象
    let (min_r, min_c) = if o.exact { (1, 1) } else { (CALC_DEFAULT_ROWS, CALC_DEFAULT_COLS) };
    let mut rows = o.rows.max(min_r).clamp(1, CALC_MAX_ROWS);
    let mut cols = o.cols.max(min_c).clamp(1, CALC_MAX_COLS);
    let mut obj_trunc = false;
    if o.exact {
        // 严格模式:只判断浮动对象是否被截掉,如实报告
        let (nr, nc, _) = expand_for_objects(&wb.sheets[o.sheet], &cfg);
        obj_trunc = nr > rows || nc > cols;
    }
    if !o.exact {
        let (nr, nc, t) = expand_for_objects(&wb.sheets[o.sheet], &cfg);
        rows = rows.max(nr);
        cols = cols.max(nc);
        obj_trunc |= t;
    }
    let (mut req_rows, mut req_cols) = (rows, cols);

    // 先按 100% 算尺寸，超过单图上限就收缩行列
    shrink_to_fit(&wb.sheets[o.sheet], hw, hh, &mut rows, &mut cols, &cfg);

    // 阶段一：测量自动行高（与 roe_render_sheet_viewport 一致，写回 wb）
    {
        let sheet = &mut wb.sheets[o.sheet];
        let logical_h = row_y_px(sheet, rows, &cfg);
        let _ = layout_measure_viewport(sheet, &cfg, 0.0, logical_h * (72.0 / 96.0));
    }

    // 测量后行高可能变化,先按新行高再扩一次,然后收缩
    if !o.exact {
        let (nr, nc, t) = expand_for_objects(&wb.sheets[o.sheet], &cfg);
        rows = rows.max(nr);
        cols = cols.max(nc);
        obj_trunc |= t;
        req_rows = req_rows.max(rows);
        req_cols = req_cols.max(cols);
    }
    // 测量后行高可能变化，再收缩一次
    shrink_to_fit(&wb.sheets[o.sheet], hw, hh, &mut rows, &mut cols, &cfg);
    let sheet = &wb.sheets[o.sheet];
    let base_w = hw + col_x_px(sheet, cols, &cfg);
    let base_h = hh + row_y_px(sheet, rows, &cfg);
    let s = clamp_scale(o.scale, base_w + 1.0, base_h + 1.0).max(1.0);
    let zoom = ((s * 100.0).floor() as u32).clamp(100, 300);
    let sf = zoom as f32 / 100.0;
    let vw = (base_w * sf) as u32 + 1;
    let vh = (base_h * sf) as u32 + 1;
    let sheet_name = short_name(&sheet.name);

    // 阶段二：绘制
    let wbr: &roe_model::calc::Workbook = &*wb;
    let evaluator = Box::new(roe_calc::CachedEvaluator::new(wbr));
    let mut renderer = roe_core::tile::CalcTileRenderer::new(wbr).with_formula_evaluator(evaluator);
    renderer.sheet_idx = o.sheet;
    let mut buf = vec![0u8; (vw as usize) * (vh as usize) * 4];
    if !renderer.render_viewport_into(0.0, 0.0, vw, vh, zoom, hw, hh, &mut buf) {
        return Err("Calc 渲染失败".into());
    }
    let png = encode_png(buf, vw, vh)?;
    Ok(vec![ExportedImage {
        label: format!("sheet{}_{}", o.sheet + 1, sheet_name),
        width: vw,
        height: vh,
        png,
        calc_range: Some(CalcRange { rows, cols, truncated: obj_trunc || rows < req_rows || cols < req_cols }),
    }])
}

/// 对一个文档句柄出图。Writer 每页一张，Impress 每张幻灯片一张，Calc 一张。
pub fn export_images(handle_id: u64, o: &ExportOptions) -> Result<Vec<ExportedImage>, String> {
    let kind = handle::with_entry(handle_id, |e| match &e.doc {
        RoeDocument::Writer(_) => 1u8,
        RoeDocument::Impress(_) => 2,
        RoeDocument::Calc(_) => 3,
        _ => 0,
    })
    .ok_or("无效句柄")?;
    if kind == 1 && !handle::ensure_writer_layout(handle_id) {
        return Err("Writer 布局建立失败".into());
    }
    handle::with_entry(handle_id, |entry| match kind {
        1 => export_writer(entry, o),
        2 => export_impress(entry, o),
        3 => export_calc(entry, o),
        _ => Err("该文档类型不支持导出图片".to_string()),
    })
    .ok_or("无效句柄")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_png_header_and_size() {
        let png = encode_png(vec![255u8; 4 * 3 * 2], 3, 2).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(u32::from_be_bytes([png[16], png[17], png[18], png[19]]), 3);
        assert_eq!(u32::from_be_bytes([png[20], png[21], png[22], png[23]]), 2);
    }

    #[test]
    fn encode_png_rejects_bad_len() {
        assert!(encode_png(vec![0u8; 5], 3, 2).is_err());
        assert!(encode_png(vec![], 0, 0).is_err());
    }

    #[test]
    fn page_window_cases() {
        assert_eq!(page_window(10, 1, usize::MAX), (0, 10));
        assert_eq!(page_window(10, 3, 5), (2, 5));
        assert_eq!(page_window(10, 0, 2), (0, 2));
        assert_eq!(page_window(10, 11, 20), (10, 10));
        assert_eq!(page_window(100, 1, usize::MAX), (0, MAX_IMAGES));
        assert_eq!(page_window(0, 1, 3), (0, 0));
    }

    #[test]
    fn clamp_scale_limits() {
        assert_eq!(clamp_scale(1.5, 600.0, 800.0), 1.5);
        assert_eq!(clamp_scale(9.0, 100.0, 100.0), 3.0);
        assert_eq!(clamp_scale(f32::NAN, 100.0, 100.0), 1.5);
        let s = clamp_scale(3.0, 3000.0, 1000.0);
        assert!(3000.0 * s <= MAX_EDGE_PX as f32 + 0.01);
    }

    // ---------- 三类文档出图测试 ----------
    use crate::handle::WriterOptions;
    use roe_model::calc::{CellValue, Sheet, SparseGrid, Workbook};
    use roe_model::common::{Color as ModelColor, Transform as ModelTransform};
    use roe_model::impress::*;
    use roe_parser::odf::fodt::FodtParser;
    use std::collections::{HashMap, VecDeque};

    fn open(doc: RoeDocument) -> u64 {
        handle::insert(DocumentEntry {
            doc,
            snapshot: None,
            is_shadow: false,
            events: VecDeque::new(),
            animation: HashMap::new(),
            writer_layout_cache: None,
            writer_font_registry: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            writer_options: WriterOptions::default(),
            impress_font_registry: None,
            writer_parley_font_context: None,
            calc_engine: None,
            pending_comment_hits: Vec::new(),
        })
    }

    fn is_png(b: &[u8]) -> bool {
        b.len() > 24 && b[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
    }

    fn calc_doc() -> RoeDocument {
        let mut grid = SparseGrid::default();
        for r in 0..10u32 {
            for c in 0..5u32 {
                grid.set(r, c, CellValue::Text(format!("R{}C{}", r + 1, c + 1)));
            }
        }
        RoeDocument::Calc(Workbook {
            sheets: vec![Sheet { name: "Sheet1".into(), cells: grid, ..Default::default() }],
            named_ranges: HashMap::new(),
            named_formulas: HashMap::new(),
            styles: roe_model::style::StyleRegistry::default(),
            formula_dialect_source: Default::default(),
        })
    }

    const FODT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
    office:mimetype="application/vnd.oasis.opendocument.text">
  <office:styles>
    <style:style style:name="Default" style:family="paragraph">
      <style:text-properties fo:font-size="12pt"/>
    </style:style>
  </office:styles>
  <office:body>
    <office:text>
      <text:p text:style-name="Default">export_image 测试段落。</text:p>
    </office:text>
  </office:body>
</office:document>"#;

    fn impress_doc(n: usize) -> RoeDocument {
        let slide = Slide {
            layout_idx: None,
            shapes: vec![Shape {
                id: 1,
                geometry: ShapeGeometry::Rect,
                position: ModelTransform::from_rect(50.0, 50.0, 200.0, 100.0),
                style: ShapeStyle {
                    fill: Some(FillStyle::Solid(ModelColor { r: 70, g: 130, b: 200, a: 255 })),
                    stroke: None,
                    shadow: None,
                    head_arrow: None,
                    tail_arrow: None,
                },
                content: ShapeContent::Empty,
                text_body: Default::default(),
                placeholder: None,
                end_cxn: None,
                st_cxn: None,
            }],
            master_idx: None,
            animation_tree: AnimationNode::TimingRoot { children: vec![] },
            transition: None,
            notes: None,
            background: None,
            layout_shapes: vec![],
        };
        RoeDocument::Impress(Presentation {
            styles: roe_model::style::StyleRegistry::default(),
            slide_size: (960.0, 540.0),
            slides: vec![slide; n],
            master_slides: vec![],
            theme_colors: Default::default(),
            default_text_style: Default::default(),
        })
    }

    #[test]
    fn calc_default_and_clamped() {
        let h = open(calc_doc());
        let imgs = export_images(h, &ExportOptions::default()).unwrap();
        assert_eq!(imgs.len(), 1);
        assert!(is_png(&imgs[0].png));
        assert!(imgs[0].width <= MAX_EDGE_PX && imgs[0].height <= MAX_EDGE_PX);
        assert!(imgs[0].label.starts_with("sheet1_"));
        let big = ExportOptions { rows: 500, cols: 500, scale: 3.0, ..Default::default() };
        let imgs = export_images(h, &big).unwrap();
        assert!(is_png(&imgs[0].png));
        assert!(imgs[0].width <= MAX_EDGE_PX && imgs[0].height <= MAX_EDGE_PX,
            "实际尺寸 {}x{}", imgs[0].width, imgs[0].height);
        let bad = ExportOptions { sheet: 5, ..Default::default() };
        assert!(export_images(h, &bad).is_err());
        handle::remove(h);
    }

    #[test]
    fn writer_pages_and_range() {
        let h = open(RoeDocument::Writer(FodtParser::parse(FODT).expect("fodt")));
        let imgs = export_images(h, &ExportOptions::default()).unwrap();
        assert_eq!(imgs.len(), 1);
        assert!(is_png(&imgs[0].png));
        assert_eq!(imgs[0].label, "page1");
        let o = ExportOptions { from: 2, to: 3, ..Default::default() };
        assert!(export_images(h, &o).unwrap().is_empty());
        handle::remove(h);
    }

    #[test]
    fn gradient_backgrounds_render_and_dump_png() {
        use roe_model::common::{FillStyle as F, GradientKind as K, GradientStop as G};
        let white = ModelColor { r: 255, g: 255, b: 255, a: 255 };
        let blue = ModelColor { r: 30, g: 80, b: 160, a: 255 };
        let blue_half = ModelColor { r: 30, g: 80, b: 160, a: 128 };
        let mut doc = impress_doc(3);
        if let RoeDocument::Impress(pres) = &mut doc {
            let mk = |kind: K, end: ModelColor| F::Gradient {
                stops: vec![G { pos: 0.0, color: white }, G { pos: 1.0, color: end }],
                kind,
                rotate_with_shape: true,
            };
            pres.slides[0].background = Some(mk(K::Rect { cx: 0.5, cy: 0.5 }, blue));
            pres.slides[1].background = Some(mk(K::Rect { cx: 0.2, cy: 0.3 }, blue));
            pres.slides[2].background = Some(mk(K::Radial { cx: 0.7, cy: 0.6 }, blue_half));
        }
        let h = open(doc);
        let o = ExportOptions { scale: 1.0, ..Default::default() };
        let imgs = export_images(h, &o).unwrap();
        assert_eq!(imgs.len(), 3);
        std::fs::create_dir_all("/tmp/grad_test").unwrap();
        let mut pix: Vec<tiny_skia::Pixmap> = Vec::new();
        for (i, im) in imgs.iter().enumerate() {
            std::fs::write(format!("/tmp/grad_test/grad{}.png", i + 1), &im.png).unwrap();
            pix.push(tiny_skia::Pixmap::decode_png(&im.png).unwrap());
        }
        let at = |pm: &tiny_skia::Pixmap, x: u32, y: u32| {
            let d = pm.data();
            let i = ((y * pm.width() + x) * 4) as usize;
            (d[i], d[i + 1], d[i + 2])
        };
        // 页 1：居中矩形渐变，中心近白，左下/右下角近深蓝
        let c = at(&pix[0], 480, 270);
        assert!(c.0 > 240 && c.1 > 240, "页1中心应近白: {:?}", c);
        let k = at(&pix[0], 5, 535);
        assert!(k.0 < 70 && k.2 > 140, "页1角落应近深蓝: {:?}", k);
        // 页 2：偏心矩形渐变，最亮点在 (192,162)
        let c = at(&pix[1], 192, 162);
        assert!(c.0 > 240 && c.1 > 240, "页2偏心点应近白: {:?}", c);
        let k = at(&pix[1], 955, 535);
        assert!(k.0 < 80, "页2远角应近深蓝: {:?}", k);
        // 页 3：偏心径向，中心 (672,324) 近白；角落因半透明比纯蓝亮
        let c = at(&pix[2], 672, 324);
        assert!(c.0 > 240, "页3中心应近白: {:?}", c);
        let k = at(&pix[2], 5, 535);
        assert!(k.0 > 100, "页3角落半透明应比纯蓝亮: {:?}", k);
        handle::remove(h);
    }

    #[test]
    fn pattern_overview_render_and_dump_png() {
        let mut doc = impress_doc(1);
        if let RoeDocument::Impress(pres) = &mut doc {
            let base = pres.slides[0].shapes[0].clone();
            let fg = ModelColor { r: 20, g: 60, b: 140, a: 255 };
            let bg = ModelColor { r: 255, g: 255, b: 255, a: 255 };
            let mut shapes = Vec::new();
            for (i, (name, _)) in roe_impress::pattern::TILES.iter().enumerate() {
                let (r, c) = (i / 6, i % 6);
                let mut s = base.clone();
                s.id = (i + 1) as _;
                s.position = ModelTransform::from_rect(5.0 + 160.0 * c as f32, 5.0 + 60.0 * r as f32, 150.0, 50.0);
                s.style.fill = Some(FillStyle::Pattern { preset: name.to_string(), fg, bg });
                shapes.push(s);
            }
            pres.slides[0].shapes = shapes;
            pres.slides[0].background = Some(FillStyle::Solid(ModelColor { r: 230, g: 230, b: 230, a: 255 }));
        }
        let h = open(doc);
        // 倍率 4/3：一格 0.75pt 正好 1 个像素
        let o = ExportOptions { scale: 4.0 / 3.0, ..Default::default() };
        let imgs = export_images(h, &o).unwrap();
        assert_eq!(imgs.len(), 1);
        std::fs::create_dir_all("/tmp/pattern_test").unwrap();
        std::fs::write("/tmp/pattern_test/patterns.png", &imgs[0].png).unwrap();
        let pm = tiny_skia::Pixmap::decode_png(&imgs[0].png).unwrap();
        // pct50（第 7 个：第2行第1列）区域内，8 个相邻像素应出现至少 2 种颜色
        let d = pm.data();
        let reds: std::collections::HashSet<u8> = (0..8u32)
            .map(|dx| d[((100 * pm.width() + 40 + dx) * 4) as usize])
            .collect();
        assert!(reds.len() >= 2, "pct50 区域应有前景/背景交替: {:?}", reds);
        handle::remove(h);
    }

    #[test]
    fn impress_slides_and_range() {
        let h = open(impress_doc(2));
        let imgs = export_images(h, &ExportOptions::default()).unwrap();
        assert_eq!(imgs.len(), 2);
        assert!(imgs.iter().all(|i| is_png(&i.png)));
        let o = ExportOptions { from: 2, to: 2, ..Default::default() };
        let one = export_images(h, &o).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].label, "slide2");
        handle::remove(h);
    }

    #[test]
    fn invalid_handle_errors() {
        assert!(export_images(u64::MAX, &ExportOptions::default()).is_err());
    }
}

#[cfg(test)]
mod expand_objects_tests {
    use super::*;

    #[test]
    fn empty_sheet_needs_one_cell() {
        let sheet = roe_model::calc::Sheet::default();
        let cfg = Default::default();
        let (r, c, t) = expand_for_objects(&sheet, &cfg);
        assert_eq!((r, c, t), (1, 1, false));
    }
}

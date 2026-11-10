use roe_core::tile::{CalcTileRenderer, CalcRenderConfig};
use roe_core::tile_geometry::{col_x_px, row_y_px};
use roe_model::calc::{Workbook, Sheet, SparseGrid, CellValue, SparseColumn, CellBlock};
use roe_polars_bridge::{PivotConfig, PivotOpts, pivot_sheet_into_workbook_v2_opt};
use std::collections::HashMap;

fn make_col(grid: &mut SparseGrid, col_idx: u32, values: Vec<CellValue>) {
    use smallvec::smallvec;
    grid.cols.insert(col_idx, SparseColumn {
        blocks: smallvec![CellBlock { start_row: 0, values: values.into() }],
    });
}

fn render(wb: &Workbook, pivot_idx: usize, path: &str) {
    let mut renderer = CalcTileRenderer::new(wb);
    renderer.sheet_idx = pivot_idx;
    renderer.config = CalcRenderConfig {
        default_col_width: 100.0,
        default_row_height: 28.0,
        ..CalcRenderConfig::default()
    };
    let max_cols = 8u32;
    let max_rows = 22u32;
    let sheet = &wb.sheets[pivot_idx];
    let cfg = &renderer.config;
    let vw = col_x_px(sheet, max_cols, cfg) as u32 + 1;
    let vh = row_y_px(sheet, max_rows, cfg) as u32 + 1;
    let mut buf = vec![0u8; (vw as usize) * (vh as usize) * 4];
    let ok = renderer.render_viewport_into(0.0, 0.0, vw, vh, 100, 0.0, 0.0, &mut buf);
    assert!(ok, "render_viewport_into failed");
    let pixmap = tiny_skia::Pixmap::from_vec(buf, tiny_skia::IntSize::from_wh(vw, vh).unwrap()).unwrap();
    pixmap.save_png(path).unwrap();
    println!("-> {}", path);
}

fn main() {
    // Region Product Size Ch Sales
    let data: Vec<(&str, &str, &str, &str, f64)> = vec![
        ("East", "Apple",  "S", "X", 1.0),
        ("East", "Apple",  "L", "Y", 2.0),
        ("East", "Banana", "S", "X", 4.0),
        ("East", "Banana", "L", "Y", 8.0),
        ("West", "Apple",  "S", "X", 16.0),
        ("West", "Apple",  "L", "Y", 32.0),
        ("West", "Banana", "S", "X", 64.0),
        ("West", "Banana", "L", "Y", 128.0),
    ];
    let mut grid = SparseGrid::default();
    let mut c0 = vec![CellValue::Text("Region".into())];
    let mut c1 = vec![CellValue::Text("Product".into())];
    let mut c2 = vec![CellValue::Text("Size".into())];
    let mut c3 = vec![CellValue::Text("Ch".into())];
    let mut c4 = vec![CellValue::Text("Sales".into())];
    for (r, p, s, ch, v) in &data {
        c0.push(CellValue::Text(r.to_string()));
        c1.push(CellValue::Text(p.to_string()));
        c2.push(CellValue::Text(s.to_string()));
        c3.push(CellValue::Text(ch.to_string()));
        c4.push(CellValue::Number(*v));
    }
    make_col(&mut grid, 0, c0);
    make_col(&mut grid, 1, c1);
    make_col(&mut grid, 2, c2);
    make_col(&mut grid, 3, c3);
    make_col(&mut grid, 4, c4);

    let source = Sheet {
        col_style: vec![],
        name: "Source".into(),
        cells: grid,
        col_widths: vec![100.0; 5],
        row_heights: vec![],
        frozen_rows: 0,
        frozen_cols: 0,
        cell_formats: HashMap::new(),
        cond_formats: vec![],
        data_validations: vec![],
        auto_filter: None,
        merged_cells: vec![],
        pivot: None,
        ..Default::default()
    };
    let mut wb = Workbook {
        sheets: vec![source],
        named_ranges: HashMap::new(),
        named_formulas: HashMap::new(),
        styles: roe_model::style::StyleRegistry::default(),
        formula_dialect_source: roe_model::calc::FormulaDialectSource::Excel,
    };
    let cfg = PivotConfig {
        rows: vec!["Region".into(), "Product".into(), "Size".into()],
        cols: vec!["Ch".into()],
        values: vec!["Sales".into()],
        agg: "sum".into(),
        filters: HashMap::new(),
    };

    let on = pivot_sheet_into_workbook_v2_opt(
        &mut wb, 0, &cfg, "PivotOn".into(), &[],
        PivotOpts { row_subtotals: true, col_subtotals: true },
    ).expect("pivot on");
    render(&wb, on, "/tmp/roe_pivot_sub3_on.png");

    let off = pivot_sheet_into_workbook_v2_opt(
        &mut wb, 0, &cfg, "PivotOff".into(), &[],
        PivotOpts { row_subtotals: false, col_subtotals: false },
    ).expect("pivot off");
    render(&wb, off, "/tmp/roe_pivot_sub3_off.png");
}

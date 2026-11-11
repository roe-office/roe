//! 图表数据源刷新:按 ChartModel.source 从单元格区域重读数据,保留样式字段。
//! 渲染前由 roe-api 在可变借用阶段调用;非数字格静默按 0 处理。

use roe_model::calc::{CellValue, Workbook};
use roe_model::impress::{ChartKind, ChartSeries};

/// 与前台 chart_registry::parse_num 同规则:千分位逗号、百分号除 100、拒绝 inf/nan。
pub fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', "");
    if t.is_empty() { return None; }
    let (body, pct) = match t.strip_suffix('%') { Some(b) => (b.trim(), true), None => (t.as_str(), false) };
    let v: f64 = body.parse().ok()?;
    if !v.is_finite() { return None; }
    Some(if pct { v / 100.0 } else { v })
}

fn cell_num(v: &CellValue) -> f64 {
    match v {
        CellValue::Number(n) => *n,
        CellValue::Formula { cached, .. } => cell_num(cached),
        CellValue::Bool(b) => if *b { 1.0 } else { 0.0 },
        other => parse_num(&other.to_display_string()).unwrap_or(0.0),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesData {
    pub name: String,
    pub data: Vec<f64>,
    pub x_values: Option<Vec<f64>>,
    pub sizes: Option<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    pub x_labels: Vec<String>,
    pub series: Vec<SeriesData>,
}

/// 区域矩阵 -> 图表数据。bubble=true 时区域按 X、Y、大小三列合成一个系列。
pub fn data_from_matrix(
    kind: &ChartKind, m: &[Vec<CellValue>], header_row: bool, cat_col: bool, bubble: bool,
) -> Option<ChartData> {
    let ncols = m.iter().map(|r| r.len()).max()?;
    let start = if header_row { 1 } else { 0 };
    if m.len() <= start || ncols == 0 { return None; }
    let at = |r: usize, c: usize| m.get(r).and_then(|row| row.get(c)).cloned().unwrap_or(CellValue::Empty);
    let rows = start..m.len();

    if bubble {
        if ncols < 3 { return None; }
        let name = if header_row { at(0, 1).to_display_string() } else { String::new() };
        let name = if name.trim().is_empty() { "系列 1".to_string() } else { name };
        let xs: Vec<f64> = rows.clone().map(|r| cell_num(&at(r, 0))).collect();
        let ys: Vec<f64> = rows.clone().map(|r| cell_num(&at(r, 1))).collect();
        let zs: Vec<f64> = rows.clone().map(|r| cell_num(&at(r, 2))).collect();
        let labels = (1..=xs.len()).map(|i| i.to_string()).collect();
        return Some(ChartData { x_labels: labels, series: vec![SeriesData { name, data: ys, x_values: Some(xs), sizes: Some(zs) }] });
    }

    let first_val = if cat_col { 1 } else { 0 };
    if ncols <= first_val { return None; }
    let x_labels: Vec<String> = if cat_col {
        rows.clone().map(|r| at(r, 0).to_display_string()).collect()
    } else {
        (1..=rows.len()).map(|i| i.to_string()).collect()
    };
    let mut last_col = ncols;
    if matches!(kind, ChartKind::Stock) { last_col = last_col.min(first_val + 4); }
    if matches!(kind, ChartKind::Pie | ChartKind::Doughnut) { last_col = last_col.min(first_val + 1); }
    let mut series = Vec::new();
    for c in first_val..last_col {
        let name = if header_row { at(0, c).to_display_string() } else { String::new() };
        let name = if name.trim().is_empty() { format!("系列 {}", c - first_val + 1) } else { name };
        let data: Vec<f64> = rows.clone().map(|r| cell_num(&at(r, c))).collect();
        series.push(SeriesData { name, data, x_values: None, sizes: None });
    }
    Some(ChartData { x_labels, series })
}

/// 把新数据写进图表,保留既有系列的 color/as_line/trendline。数据没变返回 false。
fn apply_data(chart: &mut roe_model::impress::ChartModel, d: ChartData) -> bool {
    let mut changed = chart.x_labels != d.x_labels || chart.series.len() != d.series.len();
    for (i, nd) in d.series.iter().enumerate() {
        match chart.series.get(i) {
            Some(o) => {
                if o.name != nd.name || o.data != nd.data || o.x_values != nd.x_values || o.sizes != nd.sizes { changed = true; }
            }
            None => changed = true,
        }
    }
    if !changed { return false; }
    let old = std::mem::take(&mut chart.series);
    chart.series = d.series.into_iter().enumerate().map(|(i, nd)| {
        let (color, as_line, trendline) = match old.get(i) {
            Some(o) => (o.color.clone(), o.as_line, o.trendline),
            None => (None, false, false),
        };
        ChartSeries { name: nd.name, data: nd.data, color, as_line, trendline, x_values: nd.x_values, sizes: nd.sizes }
    }).collect();
    chart.x_labels = d.x_labels;
    true
}

/// 刷新 sheet_idx 上所有带 source 的浮动图表。返回是否有图表被更新。
pub fn refresh_chart_sources(wb: &mut Workbook, sheet_idx: usize) -> bool {
    let Some(sheet) = wb.sheets.get(sheet_idx) else { return false };
    let mut updates: Vec<(usize, ChartData)> = Vec::new();
    for (i, ch) in sheet.floating_charts.iter().enumerate() {
        let Some(src) = &ch.source else { continue };
        let Some(ssheet) = wb.sheets.get(src.sheet as usize) else { continue };
        let (r1, r2) = (src.r1.min(src.r2), src.r1.max(src.r2));
        let (c1, c2) = (src.c1.min(src.c2), src.c1.max(src.c2));
        let m: Vec<Vec<CellValue>> = (r1..=r2)
            .map(|r| (c1..=c2).map(|c| ssheet.cells.get(r, c)).collect())
            .collect();
        let bubble = matches!(ch.kind, ChartKind::Scatter)
            && ch.series.first().map_or(false, |s| s.sizes.is_some());
        if let Some(d) = data_from_matrix(&ch.kind, &m, src.header_row, src.cat_col, bubble) {
            updates.push((i, d));
        }
    }
    let mut any = false;
    if let Some(sheet) = wb.sheets.get_mut(sheet_idx) {
        for (i, d) in updates {
            if let Some(ch) = sheet.floating_charts.get_mut(i) { any |= apply_data(ch, d); }
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;
    fn n(v: f64) -> CellValue { CellValue::Number(v) }
    fn t(s: &str) -> CellValue { CellValue::Text(s.to_string()) }

    #[test]
    fn parse_num_rules() {
        assert_eq!(parse_num(" 1,234 "), Some(1234.0));
        assert_eq!(parse_num("12%"), Some(0.12));
        assert_eq!(parse_num("abc"), None);
        assert_eq!(parse_num("inf"), None);
        assert_eq!(parse_num(""), None);
    }

    #[test]
    fn generic_header_and_cat() {
        let m = vec![vec![t(""), t("销售"), t("利润")], vec![t("一月"), n(1.0), n(2.0)], vec![t("二月"), t("x"), n(4.0)]];
        let d = data_from_matrix(&ChartKind::Bar, &m, true, true, false).unwrap();
        assert_eq!(d.x_labels, vec!["一月", "二月"]);
        assert_eq!(d.series.len(), 2);
        assert_eq!(d.series[0].name, "销售");
        assert_eq!(d.series[0].data, vec![1.0, 0.0]); // 非数字 -> 0
    }

    #[test]
    fn pie_first_series_only() {
        let m = vec![vec![t("a"), n(1.0), n(2.0)], vec![t("b"), n(3.0), n(4.0)]];
        let d = data_from_matrix(&ChartKind::Pie, &m, false, true, false).unwrap();
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].name, "系列 1");
    }

    #[test]
    fn stock_takes_four_columns() {
        let m = vec![vec![t("日期"), t("开"), t("高"), t("低"), t("收"), t("量")], vec![t("d1"), n(1.0), n(2.0), n(0.5), n(1.5), n(9.0)]];
        let d = data_from_matrix(&ChartKind::Stock, &m, true, true, false).unwrap();
        assert_eq!(d.series.len(), 4);
        assert_eq!(d.series[3].name, "收");
    }

    #[test]
    fn bubble_three_columns() {
        let m = vec![vec![t("x"), t("y"), t("s")], vec![n(1.0), n(2.0), n(10.0)], vec![n(3.0), n(4.0), n(20.0)]];
        let d = data_from_matrix(&ChartKind::Scatter, &m, true, false, true).unwrap();
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].data, vec![2.0, 4.0]);
        assert_eq!(d.series[0].x_values, Some(vec![1.0, 3.0]));
        assert_eq!(d.series[0].sizes, Some(vec![10.0, 20.0]));
    }

    #[test]
    fn header_only_is_none() {
        let m = vec![vec![t(""), t("v")]];
        assert!(data_from_matrix(&ChartKind::Bar, &m, true, true, false).is_none());
    }
}

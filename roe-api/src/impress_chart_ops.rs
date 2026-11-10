// v221 新增：Impress 图表插入/读取/整体替换（普通 ChartModel，17 种）。
// 统计类图表（直方/箱线/瀑布/漏斗/旭日/树状）放第二阶段。

use roe_parser::RoeDocument;

fn read_cstr_v221(p: *const std::os::raw::c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(p) }.to_str().ok().map(|s| s.to_string())
}

fn parse_chart_model_v221(json: &str) -> Result<roe_model::impress::ChartModel, String> {
    serde_json::from_str::<roe_model::impress::ChartModel>(json)
        .map_err(|e| format!("图表 JSON 解析失败: {e}"))
}

/// 在指定幻灯片插入图表形状。chart_json 为 ChartModel 的 JSON：
/// 必填 kind / x_labels / series，其余字段缺省时用模型默认值。
/// kind 取值：Bar HorizontalBar Line Pie Scatter Radar Area Doughnut；
/// grouping 取值：Standard Stacked PercentStacked；系列 as_line=true 即柱线组合。
/// x/y/w/h 单位 pt。返回新形状 id，失败 -1（详情见 last_error）。
#[no_mangle]
pub extern "C" fn roe_impress_insert_chart(
    handle: u64,
    slide_idx: u32,
    chart_json: *const std::os::raw::c_char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> i32 {
    let json = match read_cstr_v221(chart_json) {
        Some(s) => s,
        None => {
            crate::error::set_last_error("roe_impress_insert_chart: chart_json 为空或不是 UTF-8".to_string());
            return -1;
        }
    };
    let model = match parse_chart_model_v221(&json) {
        Ok(m) => m,
        Err(e) => {
            crate::error::set_last_error(format!("roe_impress_insert_chart: {e}"));
            return -1;
        }
    };
    if !(w > 0.0 && h > 0.0) {
        crate::error::set_last_error("roe_impress_insert_chart: 宽高必须大于 0".to_string());
        return -1;
    }

    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get_mut(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let next_id = slide.shapes.iter().map(|sh| sh.id).max().unwrap_or(0) + 1;
        let mut m = model.clone();
        m.id = next_id as u64;
        m.x_pt = x;
        m.y_pt = y;
        m.width_pt = w;
        m.height_pt = h;
        slide.shapes.push(roe_model::impress::Shape {
            id: next_id,
            geometry: roe_model::impress::ShapeGeometry::Rect,
            position: roe_model::common::Transform::from_rect(x, y, w, h),
            style: roe_model::common::ShapeStyle {
                fill: None,
                stroke: None,
                ..Default::default()
            },
            content: roe_model::impress::ShapeContent::Chart(m),
            text_body: Default::default(),
            placeholder: None,
            st_cxn: None,
            end_cxn: None,
        });
        Ok(next_id)
    });

    match result {
        None => {
            crate::error::set_last_error("roe_impress_insert_chart: 无效句柄".to_string());
            -1
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_insert_chart: {msg}"));
            -1
        }
        Some(Ok(id)) => id as i32,
    }
}

/// 读取图表形状的完整 ChartModel（JSON 字符串，调用方用现有的字符串释放函数释放）。
/// 编辑数据窗口用它取当前数据。非图表形状或失败返回 null。
#[no_mangle]
pub extern "C" fn roe_impress_get_chart_json(
    handle: u64,
    slide_idx: u32,
    shape_id: u32,
) -> *mut std::os::raw::c_char {
    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let shape = slide
            .shapes
            .iter()
            .find(|sh| sh.id == shape_id)
            .ok_or_else(|| format!("shape_id={shape_id} 不存在"))?;
        match &shape.content {
            roe_model::impress::ShapeContent::Chart(m) => {
                serde_json::to_string(m).map_err(|e| format!("序列化失败: {e}"))
            }
            _ => Err("该形状不是图表".to_string()),
        }
    });

    match result {
        None => {
            crate::error::set_last_error("roe_impress_get_chart_json: 无效句柄".to_string());
            std::ptr::null_mut()
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_get_chart_json: {msg}"));
            std::ptr::null_mut()
        }
        Some(Ok(s)) => match std::ffi::CString::new(s) {
            Ok(c) => c.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
    }
}

/// 用新的 ChartModel JSON 整体替换已有图表形状的内容（改类型、改数据、改样式都走这里）。
/// 保留原来的 id 与位置/尺寸。成功 true。
#[no_mangle]
pub extern "C" fn roe_impress_set_chart_json(
    handle: u64,
    slide_idx: u32,
    shape_id: u32,
    chart_json: *const std::os::raw::c_char,
) -> bool {
    let json = match read_cstr_v221(chart_json) {
        Some(s) => s,
        None => {
            crate::error::set_last_error("roe_impress_set_chart_json: chart_json 为空或不是 UTF-8".to_string());
            return false;
        }
    };
    let new_model = match parse_chart_model_v221(&json) {
        Ok(m) => m,
        Err(e) => {
            crate::error::set_last_error(format!("roe_impress_set_chart_json: {e}"));
            return false;
        }
    };

    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get_mut(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let shape = slide
            .shapes
            .iter_mut()
            .find(|sh| sh.id == shape_id)
            .ok_or_else(|| format!("shape_id={shape_id} 不存在"))?;
        match &mut shape.content {
            roe_model::impress::ShapeContent::Chart(old) => {
                let mut nm = new_model.clone();
                nm.id = old.id;
                nm.x_pt = old.x_pt;
                nm.y_pt = old.y_pt;
                nm.width_pt = old.width_pt;
                nm.height_pt = old.height_pt;
                *old = nm;
                Ok(())
            }
            _ => Err("该形状不是图表".to_string()),
        }
    });

    match result {
        None => {
            crate::error::set_last_error("roe_impress_set_chart_json: 无效句柄".to_string());
            false
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_set_chart_json: {msg}"));
            false
        }
        Some(Ok(())) => true,
    }
}

// v222: 插入统计类图表（直方/箱线/瀑布/漏斗/旭日/树状）。
// stat_json 为完整的 StatChartModel JSON（该模型无 serde 默认值，字段需写全）。
// x/y/w/h 单位 pt。返回新形状 id，失败 -1。
#[no_mangle]
pub extern "C" fn roe_impress_insert_stat_chart(
    handle: u64,
    slide_idx: u32,
    stat_json: *const std::os::raw::c_char,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> i32 {
    let json = match read_cstr_v221(stat_json) {
        Some(s) => s,
        None => {
            crate::error::set_last_error("roe_impress_insert_stat_chart: json 为空或不是 UTF-8".to_string());
            return -1;
        }
    };
    let model = match serde_json::from_str::<roe_model::stat_chart::StatChartModel>(&json) {
        Ok(m) => m,
        Err(e) => {
            crate::error::set_last_error(format!("roe_impress_insert_stat_chart: JSON 解析失败: {e}"));
            return -1;
        }
    };
    if !(w > 0.0 && h > 0.0) {
        crate::error::set_last_error("roe_impress_insert_stat_chart: 宽高必须大于 0".to_string());
        return -1;
    }
    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get_mut(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let next_id = slide.shapes.iter().map(|sh| sh.id).max().unwrap_or(0) + 1;
        slide.shapes.push(roe_model::impress::Shape {
            id: next_id,
            geometry: roe_model::impress::ShapeGeometry::Rect,
            position: roe_model::common::Transform::from_rect(x, y, w, h),
            style: roe_model::common::ShapeStyle {
                fill: None,
                stroke: None,
                ..Default::default()
            },
            content: roe_model::impress::ShapeContent::StatChart(model.clone()),
            text_body: Default::default(),
            placeholder: None,
            st_cxn: None,
            end_cxn: None,
        });
        Ok(next_id)
    });
    match result {
        None => {
            crate::error::set_last_error("roe_impress_insert_stat_chart: 无效句柄".to_string());
            -1
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_insert_stat_chart: {msg}"));
            -1
        }
        Some(Ok(id)) => id as i32,
    }
}

// ───────────── 统计图编辑数据(Impress) ─────────────

/// 读取统计图形状的完整 StatChartModel JSON。非统计图/失败返回 null。
#[no_mangle]
pub extern "C" fn roe_impress_get_stat_chart_json(
    handle: u64,
    slide_idx: u32,
    shape_id: u32,
) -> *mut std::os::raw::c_char {
    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let shape = slide
            .shapes
            .iter()
            .find(|sh| sh.id == shape_id)
            .ok_or_else(|| format!("shape_id={shape_id} 不存在"))?;
        match &shape.content {
            roe_model::impress::ShapeContent::StatChart(m) => {
                serde_json::to_string(m).map_err(|e| format!("序列化失败: {e}"))
            }
            _ => Err("该形状不是统计图".to_string()),
        }
    });
    match result {
        None => {
            crate::error::set_last_error("roe_impress_get_stat_chart_json: 无效句柄".to_string());
            std::ptr::null_mut()
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_get_stat_chart_json: {msg}"));
            std::ptr::null_mut()
        }
        Some(Ok(s)) => match std::ffi::CString::new(s) {
            Ok(c) => c.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
    }
}

/// Impress:图表/统计图配置 JSON,字段同 roe_get_chart_config。失败返回 null。
#[no_mangle]
pub extern "C" fn roe_impress_get_chart_config(
    handle: u64,
    slide_idx: u32,
    shape_id: u32,
) -> *mut std::os::raw::c_char {
    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let shape = slide
            .shapes
            .iter()
            .find(|sh| sh.id == shape_id)
            .ok_or_else(|| format!("shape_id={shape_id} 不存在"))?;
        match &shape.content {
            roe_model::impress::ShapeContent::Chart(m) => Ok(crate::chart_query::chart_config_json(m)),
            roe_model::impress::ShapeContent::StatChart(m) => {
                Ok(crate::chart_query::stat_chart_config_json(shape_id as u64, m))
            }
            _ => Err("该形状不是图表".to_string()),
        }
    });
    match result {
        None => {
            crate::error::set_last_error("roe_impress_get_chart_config: 无效句柄".to_string());
            std::ptr::null_mut()
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_get_chart_config: {msg}"));
            std::ptr::null_mut()
        }
        Some(Ok(s)) => match std::ffi::CString::new(s) {
            Ok(c) => c.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
    }
}

/// Impress:局部修改图表样式(键同 roe_set_chart_style,校验在 roe_core::chart_style)。
/// 值非法整体不生效并返回 false。
#[no_mangle]
pub extern "C" fn roe_impress_set_chart_style(
    handle: u64,
    slide_idx: u32,
    shape_id: u32,
    style_json: *const std::os::raw::c_char,
) -> bool {
    let json = match read_cstr_v221(style_json) {
        Some(s) => s,
        None => {
            crate::error::set_last_error("roe_impress_set_chart_style: style_json 为空或不是 UTF-8".to_string());
            return false;
        }
    };
    let val: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => {
            crate::error::set_last_error(format!("roe_impress_set_chart_style: JSON 解析失败: {e}"));
            return false;
        }
    };
    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get_mut(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let shape = slide
            .shapes
            .iter_mut()
            .find(|sh| sh.id == shape_id)
            .ok_or_else(|| format!("shape_id={shape_id} 不存在"))?;
        match &mut shape.content {
            roe_model::impress::ShapeContent::Chart(m) => {
                roe_core::chart_style::apply_chart_style(m, &val, false)?;
                Ok(())
            }
            roe_model::impress::ShapeContent::StatChart(m) => {
                roe_core::chart_style::apply_stat_chart_style(m, &val, false)?;
                Ok(())
            }
            _ => Err("该形状不是图表".to_string()),
        }
    });
    match result {
        None => {
            crate::error::set_last_error("roe_impress_set_chart_style: 无效句柄".to_string());
            false
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_set_chart_style: {msg}"));
            false
        }
        Some(Ok(())) => true,
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct StatDataEdit {
    pub(crate) categories: Vec<Vec<String>>,
    pub(crate) values: Vec<Option<f64>>,
    /// 瀑布图汇总柱下标。None=不改(越界丢弃);Some=整体替换(排序去重,越界丢弃并计数)。
    #[serde(default)]
    pub(crate) subtotals: Option<Vec<usize>>,
}

/// 用编辑结果替换 data[0] 的 categories/values,保留 id/value_kind 与其余数据组。
/// 越界的 subtotals 下标(>= 新行数)被丢弃,返回丢弃个数。
pub(crate) fn apply_stat_data(
    model: &mut roe_model::stat_chart::StatChartModel,
    edit: StatDataEdit,
) -> Result<usize, String> {
    let rows = edit.values.len();
    if rows == 0 {
        return Err("数据行数不能为 0".to_string());
    }
    let first = model.data.first_mut().ok_or_else(|| "统计图没有数据组".to_string())?;
    for (i, lvl) in edit.categories.iter().enumerate() {
        if lvl.len() != rows {
            return Err(format!("第 {} 层类目行数 {} 与数值行数 {} 不一致", i + 1, lvl.len(), rows));
        }
    }
    let data_id = first.id;
    first.categories = edit.categories;
    first.values = edit.values;
    let mut dropped = 0usize;
    for s in model.series.iter_mut().filter(|s| s.data_id == data_id) {
        if let Some(v) = &edit.subtotals {
            let mut nv: Vec<usize> = v.iter().copied().filter(|&i| i < rows).collect();
            nv.sort_unstable();
            nv.dedup();
            dropped += v.iter().filter(|&&i| i >= rows).count();
            s.subtotals = nv;
            continue;
        }
        let before = s.subtotals.len();
        s.subtotals.retain(|&i| i < rows);
        dropped += before - s.subtotals.len();
    }
    Ok(dropped)
}

/// 统计图「编辑数据」回写。edit_json: {"categories":[[..],..],"values":[..]}。
/// 返回:-1 失败(见 last_error);>=0 成功,数值为被丢弃的越界 subtotals 个数。
#[no_mangle]
pub extern "C" fn roe_impress_set_stat_chart_data(
    handle: u64,
    slide_idx: u32,
    shape_id: u32,
    edit_json: *const std::os::raw::c_char,
) -> i32 {
    let json = match read_cstr_v221(edit_json) {
        Some(s) => s,
        None => {
            crate::error::set_last_error("roe_impress_set_stat_chart_data: json 为空或不是 UTF-8".to_string());
            return -1;
        }
    };
    let edit: StatDataEdit = match serde_json::from_str(&json) {
        Ok(e) => e,
        Err(e) => {
            crate::error::set_last_error(format!("roe_impress_set_stat_chart_data: JSON 解析失败: {e}"));
            return -1;
        }
    };
    let result = crate::handle::with_entry(handle, |entry| {
        let RoeDocument::Impress(pres) = &mut entry.doc else {
            return Err("非 Impress 文档".to_string());
        };
        let slide = pres
            .slides
            .get_mut(slide_idx as usize)
            .ok_or_else(|| format!("slide_idx={slide_idx} 越界"))?;
        let shape = slide
            .shapes
            .iter_mut()
            .find(|sh| sh.id == shape_id)
            .ok_or_else(|| format!("shape_id={shape_id} 不存在"))?;
        match &mut shape.content {
            roe_model::impress::ShapeContent::StatChart(m) => apply_stat_data(m, edit),
            _ => Err("该形状不是统计图".to_string()),
        }
    });
    match result {
        None => {
            crate::error::set_last_error("roe_impress_set_stat_chart_data: 无效句柄".to_string());
            -1
        }
        Some(Err(msg)) => {
            crate::error::set_last_error(format!("roe_impress_set_stat_chart_data: {msg}"));
            -1
        }
        Some(Ok(n)) => n as i32,
    }
}

#[cfg(test)]
mod stat_edit_tests {
    use super::*;
    use roe_model::stat_chart::*;

    fn model() -> StatChartModel {
        StatChartModel {
            kind: StatChartKind::Waterfall,
            title_present: false,
            title: None,
            show_legend: false,
            legend_pos: None,
            show_grid: false,
            gap_width: None,
            data: vec![
                StatData { id: 0, categories: vec![vec!["a".into(), "b".into(), "c".into(), "d".into()]],
                    values: vec![Some(1.0), Some(2.0), Some(3.0), Some(6.0)], value_kind: "val".into() },
                StatData { id: 1, categories: vec![], values: vec![Some(9.0)], value_kind: "val".into() },
            ],
            series: vec![StatSeries {
                name: "s".into(), layout_id: "waterfall".into(), format_idx: 0, data_id: 0,
                bin_width: None, bin_count: None, interval_closed_right: false,
                quartile_exclusive: false, mean_line: false, mean_marker: false,
                show_outliers: false, show_non_outliers: false,
                subtotals: vec![0, 3], data_labels: None,
            }],
        }
    }

    #[test]
    fn replaces_first_data_keeps_others_and_kind() {
        let mut m = model();
        let n = apply_stat_data(&mut m, StatDataEdit {
            categories: vec![vec!["x".into(), "y".into(), "z".into(), "w".into()]],
            values: vec![Some(5.0), None, Some(7.0), Some(8.0)],
            subtotals: None,
        }).unwrap();
        assert_eq!(n, 0);
        assert_eq!(m.data[0].values.len(), 4);
        assert_eq!(m.data[0].values[1], None);
        assert_eq!(m.data[0].categories[0][0], "x");
        assert_eq!(m.data[0].value_kind, "val");
        assert_eq!(m.data[1].values, vec![Some(9.0)]);
    }

    #[test]
    fn drops_out_of_range_subtotals() {
        let mut m = model();
        let n = apply_stat_data(&mut m, StatDataEdit {
            categories: vec![vec!["x".into(), "y".into()]],
            values: vec![Some(1.0), Some(2.0)],
            subtotals: None,
        }).unwrap();
        assert_eq!(n, 1);
        assert_eq!(m.series[0].subtotals, vec![0]);
    }

    #[test]
    fn subtotals_some_replaces_sorted_dedup_and_counts_out_of_range() {
        let mut m = model();
        let n = apply_stat_data(&mut m, StatDataEdit {
            categories: vec![vec!["a".into(), "b".into(), "c".into()]],
            values: vec![Some(1.0), Some(2.0), Some(3.0)],
            subtotals: Some(vec![2, 1, 1, 9]),
        }).unwrap();
        assert_eq!(n, 1);
        assert_eq!(m.series[0].subtotals, vec![1, 2]);
    }

    #[test]
    fn subtotals_some_empty_clears() {
        let mut m = model();
        let n = apply_stat_data(&mut m, StatDataEdit {
            categories: vec![vec!["a".into(), "b".into(), "c".into(), "d".into()]],
            values: vec![Some(1.0), Some(2.0), Some(3.0), Some(6.0)],
            subtotals: Some(vec![]),
        }).unwrap();
        assert_eq!(n, 0);
        assert!(m.series[0].subtotals.is_empty());
    }

    #[test]
    fn rejects_mismatched_rows_and_empty() {
        let mut m = model();
        assert!(apply_stat_data(&mut m, StatDataEdit {
            categories: vec![vec!["x".into()]], values: vec![Some(1.0), Some(2.0)], subtotals: None }).is_err());
        assert!(apply_stat_data(&mut m, StatDataEdit { categories: vec![], values: vec![], subtotals: None }).is_err());
    }
}

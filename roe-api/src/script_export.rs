//! 脚本执行 + 导出图片:脚本成功后,按 export_image 排队的请求出图并写入 workspace/exports。
//! 脚本失败(已回滚)时丢弃队列,不出图。出图失败不让脚本失败(文档已改),错误放进 errors。

use crate::export_image::{export_images, CalcRange, ExportOptions};
use crate::script::{execute_script_impl, DirtyRegion};
use roe_script::export_queue::{self, ExportRequest};
use roe_script::media_context::MediaAssetSource;
use serde_json::Value;

/// 单次脚本最多落盘的图片数。
pub const MAX_FILES_PER_SCRIPT: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct ExportedFile {
    /// workspace 虚拟路径,如 /exports/封面_page1_1108_v1.png
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub calc_range: Option<CalcRange>,
}

#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    pub files: Vec<ExportedFile>,
    /// 本次脚本新建的导出文件夹(虚拟路径),每个 export_image 请求一个。
    pub folders: Vec<String>,
    pub errors: Vec<String>,
}

fn parse_range(v: &Value) -> Result<(usize, usize), String> {
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return Err("pages/slides 应为 \"3\" 或 \"1-3\"(1 起)".to_string()),
    };
    let bad = || format!("范围 {s:?} 不合法,应为 \"3\" 或 \"1-3\"(1 起,前不大于后)");
    let (a, b) = match s.split_once('-') {
        Some((a, b)) => (
            a.trim().parse::<usize>().map_err(|_| bad())?,
            b.trim().parse::<usize>().map_err(|_| bad())?,
        ),
        None => {
            let n = s.parse::<usize>().map_err(|_| bad())?;
            (n, n)
        }
    };
    if a == 0 || b < a {
        return Err(bad());
    }
    Ok((a, b))
}

/// 选项 JSON -> ExportOptions。空串=全默认。
pub fn parse_options(json: &str) -> Result<ExportOptions, String> {
    let mut o = ExportOptions::default();
    if json.trim().is_empty() {
        return Ok(o);
    }
    let v: Value = serde_json::from_str(json).map_err(|e| format!("options 不是合法 JSON: {e}"))?;
    let m = v.as_object().ok_or_else(|| "options 必须是 JSON 对象".to_string())?;
    for (k, val) in m {
        match k.as_str() {
            "pages" | "slides" => {
                let (a, b) = parse_range(val)?;
                o.from = a;
                o.to = b;
            }
            "scale" => {
                o.scale = val.as_f64().ok_or_else(|| "scale 应为数字".to_string())? as f32;
            }
            "sheet" => {
                o.sheet = val.as_u64().ok_or_else(|| "sheet 应为非负整数(0 起)".to_string())? as usize;
            }
            "rows" => {
                o.rows = val.as_u64().ok_or_else(|| "rows 应为正整数".to_string())? as u32;
            }
            "cols" => {
                o.cols = val.as_u64().ok_or_else(|| "cols 应为正整数".to_string())? as u32;
            }
            "headers" => {
                o.headers = val.as_bool().ok_or_else(|| "headers 应为 true/false".to_string())?;
            }
            "exact" => {
                o.exact = val.as_bool().ok_or_else(|| "exact 应为 true/false".to_string())?;
            }
            other => {
                return Err(format!(
                    "未知选项 {other};可用: pages/slides, scale, sheet, rows, cols, headers, exact"
                ))
            }
        }
    }
    Ok(o)
}

fn sanitize(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

fn run_exports(handle: u64, reqs: &[ExportRequest], media: Option<&dyn MediaAssetSource>) -> ExportReport {
    let mut rep = ExportReport::default();
    for req in reqs {
        let opts = match parse_options(&req.options_json) {
            Ok(o) => o,
            Err(e) => {
                rep.errors.push(format!("export_image(\"{}\"): {e}", req.name));
                continue;
            }
        };
        let imgs = match export_images(handle, &opts) {
            Ok(v) => v,
            Err(e) => {
                rep.errors.push(format!("export_image(\"{}\"): {e}", req.name));
                continue;
            }
        };
        if imgs.is_empty() {
            continue;
        }
        let Some(m) = media else {
            rep.errors.push("当前会话未提供 workspace,无法保存导出图片".to_string());
            return rep;
        };
        let dir = match m.begin_export_dir(&req.name) {
            Ok(d) => d,
            Err(e) => {
                rep.errors.push(format!("创建导出文件夹失败: {e}"));
                continue;
            }
        };
        rep.folders.push(dir.clone());
        for img in imgs {
            if rep.files.len() >= MAX_FILES_PER_SCRIPT {
                rep.errors.push(format!("单次脚本最多导出 {MAX_FILES_PER_SCRIPT} 张图片,其余已跳过"));
                return rep;
            }
            let file_name = format!("{}.png", sanitize(&img.label));
            match m.write_export_file(&dir, &file_name, &img.png) {
                Ok(path) => rep.files.push(ExportedFile { path, width: img.width, height: img.height, calc_range: img.calc_range.clone() }),
                Err(e) => rep.errors.push(format!("保存 {file_name} 失败: {e}")),
            }
        }
    }
    rep
}

/// 与 execute_script_impl 相同,另外处理 export_image 请求。
/// 脚本失败或句柄无效时不出图。
pub fn execute_script_with_exports(
    handle: u64,
    script: &str,
    media_source: Option<&dyn MediaAssetSource>,
) -> Option<Result<(Vec<usize>, Vec<DirtyRegion>, Value, ExportReport), String>> {
    export_queue::clear();
    let r = execute_script_impl(handle, script, media_source);
    let reqs = export_queue::take();
    match r {
        None => None,
        Some(Err(e)) => Some(Err(e)),
        Some(Ok((dirty, regions, ret))) => {
            let rep = if reqs.is_empty() {
                ExportReport::default()
            } else {
                run_exports(handle, &reqs, media_source)
            };
            Some(Ok((dirty, regions, ret, rep)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_defaults_and_fields() {
        let d = parse_options("").unwrap();
        assert_eq!((d.from, d.to, d.sheet), (1, usize::MAX, 0));
        let o = parse_options(r#"{"pages":"2-4","scale":2,"sheet":1,"rows":10,"cols":5,"headers":false}"#).unwrap();
        assert_eq!((o.from, o.to, o.sheet, o.rows, o.cols), (2, 4, 1, 10, 5));
        assert!(!o.headers);
        assert!((o.scale - 2.0).abs() < 1e-6);
        let s = parse_options(r#"{"slides":3}"#).unwrap();
        assert_eq!((s.from, s.to), (3, 3));
    }

    #[test]
    fn options_rejects_bad() {
        assert!(parse_options(r#"{"pages":"0"}"#).is_err());
        assert!(parse_options(r#"{"pages":"4-2"}"#).is_err());
        assert!(parse_options(r#"{"pages":"a"}"#).is_err());
        assert!(parse_options(r#"{"foo":1}"#).is_err());
        assert!(parse_options(r#"{"headers":"yes"}"#).is_err());
        assert!(parse_options("[1]").is_err());
    }

    #[test]
    fn sanitize_label() {
        assert_eq!(sanitize("sheet1_a/b:c"), "sheet1_a_b_c");
        assert_eq!(sanitize("page1"), "page1");
    }

    #[test]
    fn invalid_handle_is_none_and_queue_cleared() {
        export_queue::clear();
        export_queue::push("x", "").unwrap();
        assert!(execute_script_with_exports(u64::MAX, "1", None).is_none());
        assert_eq!(export_queue::pending_len(), 0);
    }
}

#[cfg(test)]
mod calc_range_tests {
    use crate::export_image::CalcRange;

    #[test]
    fn calc_range_serializes() {
        let v = serde_json::to_value(CalcRange { rows: 40, cols: 12, truncated: true }).unwrap();
        assert_eq!(v["rows"], 40);
        assert_eq!(v["cols"], 12);
        assert_eq!(v["truncated"], true);
    }
}

#[cfg(test)]
mod exact_option_tests {
    use super::*;

    #[test]
    fn exact_option_parses() {
        assert!(!parse_options("").unwrap().exact);
        let o = parse_options(r#"{"exact":true,"rows":40,"cols":20}"#).unwrap();
        assert!(o.exact);
        assert!(parse_options(r#"{"exact":"yes"}"#).is_err());
    }
}

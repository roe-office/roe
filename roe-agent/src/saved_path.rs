//! 从工具结果里取出 save_document 产物的虚拟路径(供 UI 在保存后刷新/提示)。

use rig_core::tool::ToolResult;

/// 仅当工具名是 `save_document` 且执行成功、输出是含字符串字段
/// `virtual_path` 的 JSON 对象时返回该路径;其余情况一律 `None`。
pub fn extract_saved_path(tool_name: &str, result: &ToolResult) -> Option<String> {
    if tool_name != "save_document" || !result.is_success() {
        return None;
    }
    result
        .output()
        .as_json()?
        .get("virtual_path")?
        .as_str()
        .map(str::to_string)
}

/// 执行脚本工具的名字(与 tool.rs 里 ExecuteScriptTool::NAME 一致,有测试守着)。
pub const EXECUTE_TOOL_NAME: &str = "execute_script_preview";

/// 仅当工具是执行脚本工具且成功时,取出结果里 `exports[*].path`(export_image 导出的图片);
/// 其余情况返回空列表。
pub fn extract_export_paths(tool_name: &str, result: &ToolResult) -> Vec<String> {
    if tool_name != EXECUTE_TOOL_NAME || !result.is_success() {
        return Vec::new();
    }
    let Some(json) = result.output().as_json() else { return Vec::new() };
    let Some(arr) = json.get("exports").and_then(|v| v.as_array()) else { return Vec::new() };
    arr.iter()
        .filter_map(|e| e.get("path").and_then(|p| p.as_str()).map(str::to_string))
        .collect()
}

/// 把导出的图片路径归并成文件夹路径(每个文件夹只出现一次);
/// 直接放在 /exports 根下的单个文件仍返回文件路径本身。
pub fn extract_export_folders(tool_name: &str, result: &ToolResult) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in extract_export_paths(tool_name, result) {
        let item = match p.rsplit_once('/') {
            Some((dir, _)) if dir.len() > "/exports".len() => dir.to_string(),
            _ => p.clone(),
        };
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_doc::SaveDocumentOutput;
    use crate::worker::DocKind;
    use rig_core::tool::{ToolExecutionError, ToolOutput};

    fn ok_json(v: serde_json::Value) -> ToolResult {
        ToolResult::success(ToolOutput::json(v))
    }

    #[test]
    fn returns_path_for_successful_save_document() {
        let out = SaveDocumentOutput {
            virtual_path: "/exports/报表.xlsx".to_string(),
            kind: DocKind::Calc,
            size_bytes: 12,
        };
        let r = ok_json(serde_json::to_value(&out).unwrap());
        assert_eq!(extract_saved_path("save_document", &r), Some("/exports/报表.xlsx".to_string()));
    }

    #[test]
    fn other_tool_name_gives_none() {
        let r = ok_json(serde_json::json!({"virtual_path": "/exports/a.xlsx"}));
        assert_eq!(extract_saved_path("list_workspace", &r), None);
    }

    #[test]
    fn failed_result_gives_none() {
        let r = ToolResult::failed(ToolExecutionError::other("boom"));
        assert_eq!(extract_saved_path("save_document", &r), None);
    }

    #[test]
    fn missing_or_non_string_virtual_path_gives_none() {
        let r1 = ok_json(serde_json::json!({"kind": "calc"}));
        let r2 = ok_json(serde_json::json!({"virtual_path": 5}));
        assert_eq!(extract_saved_path("save_document", &r1), None);
        assert_eq!(extract_saved_path("save_document", &r2), None);
    }
}

#[cfg(test)]
mod export_path_tests {
    use super::*;
    use rig_core::tool::{ToolExecutionError, ToolOutput};

    fn ok_json(v: serde_json::Value) -> ToolResult {
        ToolResult::success(ToolOutput::json(v))
    }

    #[test]
    fn tool_name_matches_real_tool() {
        use rig_agent::prelude::Tool;
        assert_eq!(crate::tool::ExecuteScriptTool::NAME, EXECUTE_TOOL_NAME);
    }

    #[test]
    fn collects_all_export_paths() {
        let r = ok_json(serde_json::json!({
            "dirty_pages": [], "dirty_regions": [], "script_result": null,
            "exports": [
                {"path": "/exports/a_page1_1108_v1.png", "width": 10, "height": 10},
                {"path": "/exports/a_page2_1108_v1.png", "width": 10, "height": 10}
            ]
        }));
        assert_eq!(extract_export_paths(EXECUTE_TOOL_NAME, &r).len(), 2);
    }

    #[test]
    fn no_exports_other_tool_or_failure_gives_empty() {
        let r = ok_json(serde_json::json!({"script_result": 1}));
        assert!(extract_export_paths(EXECUTE_TOOL_NAME, &r).is_empty());
        let r2 = ok_json(serde_json::json!({"exports": [{"path": "/exports/a.png"}]}));
        assert!(extract_export_paths("query_script_preview", &r2).is_empty());
        let r3 = ToolResult::failed(ToolExecutionError::other("boom"));
        assert!(extract_export_paths(EXECUTE_TOOL_NAME, &r3).is_empty());
    }
}

#[cfg(test)]
mod export_folder_tests {
    use super::*;
    use rig_core::tool::ToolOutput;

    #[test]
    fn folders_are_deduped() {
        let r = ToolResult::success(ToolOutput::json(serde_json::json!({
            "exports": [
                {"path": "/exports/a_1108_v1/page1.png"},
                {"path": "/exports/a_1108_v1/page2.png"},
                {"path": "/exports/b_1108_v1/slide1.png"},
                {"path": "/exports/old.png"}
            ]
        })));
        assert_eq!(
            extract_export_folders(EXECUTE_TOOL_NAME, &r),
            vec!["/exports/a_1108_v1", "/exports/b_1108_v1", "/exports/old.png"]
        );
    }
}

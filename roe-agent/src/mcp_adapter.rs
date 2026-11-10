//! MCP 适配层的纯逻辑(不依赖 rmcp):对外工具名别名、文本改写、
//! instructions 生成、状态锚定。对外叫 execute_script / query_script,
//! 内部仍是 *_preview(提示词与 GUI 不动)。

use serde_json::{json, Value as JsonValue};

use crate::worker::DocSummary;

/// (对外名, 内部名)。
pub const TOOL_ALIASES: [(&str, &str); 2] = [
    ("execute_script", "execute_script_preview"),
    ("query_script", "query_script_preview"),
];

/// 对外暴露的 7 个工具名(顺序即 list_tools 顺序)。
pub const PUBLIC_TOOLS: [&str; 7] = [
    "execute_script",
    "query_script",
    "new_document",
    "open_document",
    "save_document",
    "close_document",
    "list_workspace",
];

/// MCP 注解提示(read_only_hint / destructive_hint)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolHints {
    pub read_only: bool,
    pub destructive: bool,
}

/// 对外名 -> 内部名;不在 PUBLIC_TOOLS 里的(含旧的 *_preview 名)返回 None。
pub fn to_internal_name(public: &str) -> Option<&'static str> {
    let name = PUBLIC_TOOLS.iter().find(|n| **n == public).copied()?;
    for (p, i) in TOOL_ALIASES {
        if p == name {
            return Some(i);
        }
    }
    Some(name)
}

/// 只替换 execute_script_preview / query_script_preview 两个名字(df_preview 等不动)。
pub fn rewrite_text(s: &str) -> String {
    let mut out = s.to_string();
    for (public, internal) in TOOL_ALIASES {
        out = out.replace(internal, public);
    }
    out
}

/// 对外工具的注解;未知名返回 None。
pub fn hints_for(public: &str) -> Option<ToolHints> {
    let (read_only, destructive) = match public {
        "query_script" | "list_workspace" => (true, false),
        "execute_script" | "new_document" | "open_document" | "save_document" => (false, false),
        "close_document" => (false, true),
        _ => return None,
    };
    Some(ToolHints { read_only, destructive })
}

/// MCP instructions:headless_preamble 经别名改写,三处 GUI 措辞换成带工作区根目录的说法,
/// 末尾追加 MCP 专属段(栈语义、active_document、外部文件放 <root>/inputs/)。
/// 替换用整句匹配;prompt.rs 措辞变动导致匹配不到时,测试会报警。
pub fn build_instructions(workspace_root: &str) -> String {
    let root = workspace_root.trim_end_matches('/');
    let mut t = rewrite_text(&crate::prompt::headless_preamble());
    let swaps: [(&str, String); 3] = [
        (
            "用户的文件不在工作区里,你读不到,请他先上传。",
            format!("用户的文件不在工作区里,你读不到,请他把文件放进 {root}/inputs/ 后再告诉你。"),
        ),
        (
            "保存成功后只说文件名,不要说路径,界面会自动显示文件卡片,最后一句提示用户点击卡片上的“预览”或“在文件夹中显示”查看刚创建的文件。",
            format!("保存成功后告诉用户文件在 {root}/exports/ 下。"),
        ),
        (
            "6. 不要告诉用户宿主机的真实路径,也不要在回复里写文件路径,只说文件名和做了什么。",
            format!("调用工具时一律使用虚拟路径(/inputs/、/exports/);向用户说明文件位置时,告知工作区根目录是 {root}。"),
        ),
    ];
    for (old, new) in swaps {
        t = t.replace(old, &new);
    }
    t.push_str(&format!(
        "\n\n本服务通过 MCP 提供,工具为 execute_script、query_script、new_document、open_document、save_document、close_document、list_workspace。\n\
每个工具的返回都带状态锚:active_document(当前文档的名字和类型,没有则为 null)与 open_documents(已打开文档的完整栈,current 为 true 的是当前文档)。\
execute_script 与 query_script 只作用在 active_document 上;同时打开多份文档时,每次操作前先核对这两个字段确认目标。\n\
工作区根目录是 {root}。用户要处理的外部文件请他放进 {root}/inputs/,你再用 /inputs/文件名 打开;保存的产物在 {root}/exports/。"
    ));
    t.push_str(DOC_ID_NOTE_ZH);
    t
}

const DOC_ID_NOTE_ZH: &str = "\n每份文档都有稳定的 id(如 \"d1\"),列在状态锚的 open_documents 里。\
execute_script、query_script、save_document、close_document 可选传 doc_id 指定目标文档:不传则作用在当前文档(active_document);\
传了就只作用在那一份,不改变当前文档。同时处理多份文档时建议显式传 doc_id,避免操作到别的文档。id 不会复用,文档关闭后它的 id 即失效。";

fn docs_json(docs: &[DocSummary]) -> (JsonValue, JsonValue) {
    let active = docs
        .iter()
        .find(|d| d.current)
        .map(|d| json!({ "id": d.id, "name": d.name, "kind": serde_json::to_value(d.kind).unwrap_or(JsonValue::Null) }))
        .unwrap_or(JsonValue::Null);
    let all: Vec<JsonValue> = docs
        .iter()
        .map(|d| serde_json::to_value(d).unwrap_or(JsonValue::Null))
        .collect();
    (active, JsonValue::Array(all))
}

/// 成功结果加状态锚:{"result", "active_document", "open_documents"}。
pub fn anchor_ok(result: JsonValue, docs: &[DocSummary]) -> JsonValue {
    let (active, all) = docs_json(docs);
    json!({ "result": result, "active_document": active, "open_documents": all })
}

/// 错误结果加状态锚:{"error", "active_document", "open_documents"}。
pub fn anchor_err(message: &str, docs: &[DocSummary]) -> JsonValue {
    let (active, all) = docs_json(docs);
    json!({ "error": message, "active_document": active, "open_documents": all })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::DocKind;

    fn doc(name: &str, kind: DocKind, current: bool) -> DocSummary {
        DocSummary { id: format!("d-{name}"), kind, name: name.to_string(), current }
    }

    #[test]
    fn public_names_map_to_internal() {
        assert_eq!(to_internal_name("execute_script"), Some("execute_script_preview"));
        assert_eq!(to_internal_name("query_script"), Some("query_script_preview"));
        assert_eq!(to_internal_name("save_document"), Some("save_document"));
        assert_eq!(to_internal_name("execute_script_preview"), None);
        assert_eq!(to_internal_name("nope"), None);
    }

    #[test]
    fn every_public_tool_has_hints() {
        for n in PUBLIC_TOOLS {
            assert!(hints_for(n).is_some(), "{n}");
        }
        assert!(hints_for("execute_script_preview").is_none());
    }

    #[test]
    fn hints_values() {
        assert!(hints_for("query_script").unwrap().read_only);
        assert!(hints_for("list_workspace").unwrap().read_only);
        for n in ["execute_script", "new_document", "open_document", "save_document"] {
            let h = hints_for(n).unwrap();
            assert!(!h.read_only && !h.destructive, "{n}");
        }
        let c = hints_for("close_document").unwrap();
        assert!(!c.read_only && c.destructive);
    }

    #[test]
    fn rewrite_replaces_only_the_two_tool_names() {
        let s = "用 execute_script_preview 改,用 query_script_preview 读,df_preview(5) 不动";
        assert_eq!(rewrite_text(s), "用 execute_script 改,用 query_script 读,df_preview(5) 不动");
    }

    #[test]
    fn instructions_have_no_old_names_and_no_gui_wording() {
        let t = build_instructions("/home/u/ws");
        assert!(!t.contains("execute_script_preview") && !t.contains("query_script_preview"));
        assert!(t.contains("execute_script") && t.contains("query_script"));
        assert!(t.contains("list_functions"));
        assert!(!t.contains("项目目录"), "GUI 措辞未替换");
        assert!(!t.contains("前端"), "GUI 措辞未替换");
        assert!(!t.contains("请他先上传"), "GUI 措辞未替换");
        assert!(!t.contains("不要告诉用户宿主机的真实路径"), "GUI 措辞未替换");
    }

    #[test]
    fn instructions_explain_stack_anchor_and_root() {
        let t = build_instructions("/w");
        assert!(t.contains("active_document") && t.contains("open_documents"));
        assert!(t.contains("/w/inputs") && t.contains("/w/exports"));
    }

    #[test]
    fn anchor_ok_wraps_result_and_names_active() {
        let docs = vec![doc("A", DocKind::Calc, false), doc("B", DocKind::Writer, true)];
        let v = anchor_ok(json!({"x": 1}), &docs);
        assert_eq!(v["result"]["x"], 1);
        assert_eq!(v["active_document"]["name"], "B");
        assert_eq!(v["active_document"]["kind"], "writer");
        assert_eq!(v["open_documents"].as_array().unwrap().len(), 2);
        assert_eq!(v["open_documents"][1]["current"], true);
    }

    #[test]
    fn anchor_with_no_documents_is_null_and_empty() {
        let v = anchor_ok(json!("ok"), &[]);
        assert!(v["active_document"].is_null());
        assert_eq!(v["open_documents"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn anchor_err_carries_message_and_state() {
        let docs = vec![doc("A", DocKind::Impress, true)];
        let v = anchor_err("脚本失败", &docs);
        assert_eq!(v["error"], "脚本失败");
        assert_eq!(v["active_document"]["kind"], "impress");
        assert!(v.get("result").is_none());
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;
    use crate::worker::DocKind;

    #[test]
    fn anchor_active_document_and_open_documents_carry_ids() {
        let docs = vec![
            DocSummary { id: "d1".into(), kind: DocKind::Calc, name: "A".into(), current: false },
            DocSummary { id: "d2".into(), kind: DocKind::Writer, name: "B".into(), current: true },
        ];
        let v = anchor_ok(json!(null), &docs);
        assert_eq!(v["active_document"]["id"], "d2");
        assert_eq!(v["open_documents"][0]["id"], "d1");
        assert_eq!(v["open_documents"][1]["id"], "d2");
    }
}

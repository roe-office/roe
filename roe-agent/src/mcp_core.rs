//! MCP 服务的核心(不依赖 rmcp):装配 7 个工具、生成对外清单、分发调用并套状态锚。
//! rmcp 外壳(roe-mcp bin)只负责协议收发,调用都转给这里。

use std::sync::Arc;

use rig_agent::tool::Tool;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::mcp_adapter::{anchor_err, anchor_ok, hints_for, rewrite_text, to_internal_name, ToolHints, PUBLIC_TOOLS};
use crate::tool::{ExecuteScriptArgs, ExecuteScriptTool, QueryScriptArgs, QueryScriptTool};
use crate::tool_doc::{
    CloseDocumentArgs, CloseDocumentTool, ListWorkspaceArgs, ListWorkspaceTool, NewDocumentArgs,
    NewDocumentTool, OpenDocumentArgs, OpenDocumentTool, SaveDocumentArgs, SaveDocumentTool,
};
use crate::mcp_en::{tool_text_en, Lang};
use crate::worker::DocWorker;
use crate::workspace::WorkspaceContext;

/// 对外工具清单的一项。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    pub schema: JsonValue,
    pub hints: ToolHints,
}

/// 一次调用的结果:is_error 对应 MCP 的 isError,body 是带状态锚的 JSON。
#[derive(Debug, Clone)]
pub struct CallOutcome {
    pub is_error: bool,
    pub body: JsonValue,
}

pub struct McpCore {
    worker: Arc<DocWorker>,
    workspace: Arc<WorkspaceContext>,
    /// 串行闸门:rmcp 会并发处理请求,而文档栈是单一共享状态。
    /// call() 全程持有它,保证"分发 + 查状态锚"不与其他调用交叉。
    gate: tokio::sync::Mutex<()>,
    /// 对外语言(描述、schema、本层错误文案)。默认 Zh。
    lang: Lang,
}

impl McpCore {
    pub fn new(worker: Arc<DocWorker>, workspace: Arc<WorkspaceContext>) -> Self {
        Self { worker, workspace, gate: tokio::sync::Mutex::new(()), lang: Lang::Zh }
    }

    /// 设置对外语言(默认 Zh,与 v321 行为一致)。
    pub fn with_lang(mut self, lang: Lang) -> Self {
        self.lang = lang;
        self
    }

    /// 7 个对外工具,顺序同 PUBLIC_TOOLS;描述与 schema 经 rewrite_text 去掉旧工具名。
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = vec![
            spec("execute_script", &ExecuteScriptTool::new(self.worker.clone())),
            spec("query_script", &QueryScriptTool::new(self.worker.clone())),
            spec("new_document", &NewDocumentTool::new(self.worker.clone())),
            spec("open_document", &OpenDocumentTool::new(self.worker.clone(), self.workspace.clone())),
            spec("save_document", &SaveDocumentTool::new(self.worker.clone(), self.workspace.clone())),
            spec("close_document", &CloseDocumentTool::new(self.worker.clone())),
            spec("list_workspace", &ListWorkspaceTool::new(self.worker.clone(), self.workspace.clone())),
        ];
        if self.lang == Lang::En {
            for s in &mut specs {
                if let Some((d, sc)) = tool_text_en(s.name) {
                    s.description = d;
                    s.schema = sc;
                }
            }
        }
        for s in &mut specs {
            if DOC_ID_TOOLS.contains(&s.name) {
                add_doc_id_property(&mut s.schema, self.lang);
            }
        }
        specs
    }

    /// 按对外名分发。args 应为 JSON 对象(Null 按 {})。失败不抛协议错误,返回 is_error=true。
    /// 无论成败,返回体都带状态锚(调用后重新查询文档栈)。
    pub async fn call(&self, public_name: &str, args: JsonValue) -> CallOutcome {
        let _gate = self.gate.lock().await; // 持到函数返回,覆盖状态锚查询
        let args = if args.is_null() { serde_json::json!({}) } else { args };
        let result = self.dispatch(public_name, args).await;
        let docs = self.worker.list_documents().await.unwrap_or_default();
        match result {
            Ok(v) => CallOutcome { is_error: false, body: anchor_ok(v, &docs) },
            Err(e) => CallOutcome { is_error: true, body: anchor_err(&e, &docs) },
        }
    }

    /// execute_script / query_script / save_document / close_document 可带可选的 doc_id:
    /// 取出并校验(必须是字符串,且对应一份已打开的文档);其余工具原样放行。
    async fn take_doc_id(
        &self,
        public: &str,
        mut args: JsonValue,
    ) -> Result<(JsonValue, Option<String>), String> {
        if !DOC_ID_TOOLS.contains(&public) {
            return Ok((args, None));
        }
        let removed = args.as_object_mut().and_then(|m| m.remove("doc_id"));
        let raw = match removed {
            None | Some(JsonValue::Null) => return Ok((args, None)),
            Some(v) => v,
        };
        let id = raw
            .as_str()
            .map(|s| s.trim().to_string())
            .ok_or_else(|| match self.lang {
                Lang::Zh => "doc_id 必须是字符串(如 \"d1\")".to_string(),
                Lang::En => "doc_id must be a string (for example \"d1\")".to_string(),
            })?;
        let docs = self.worker.list_documents().await.unwrap_or_default();
        if docs.iter().any(|d| d.id == id) {
            return Ok((args, Some(id)));
        }
        let ids: Vec<&str> = docs.iter().map(|d| d.id.as_str()).collect();
        Err(match self.lang {
            Lang::Zh => format!("找不到 id 为 {id:?} 的文档;当前打开的文档 id: [{}]", ids.join(", ")),
            Lang::En => format!("No open document has id {id:?}; open document ids: [{}]", ids.join(", ")),
        })
    }

    async fn dispatch(&self, public: &str, args: JsonValue) -> Result<JsonValue, String> {
        if to_internal_name(public).is_none() {
            return Err(match self.lang {
                Lang::Zh => format!("未知工具 {public:?}:可用工具是 {}", PUBLIC_TOOLS.join("、")),
                Lang::En => format!("Unknown tool {public:?}: available tools are {}", PUBLIC_TOOLS.join(", ")),
            });
        }
        let (args, doc) = self.take_doc_id(public, args).await?;
        match public {
            "execute_script" => {
                let a: ExecuteScriptArgs = parse(args, self.lang)?;
                let out = self.worker.execute_on(doc, a.script).await.map_err(|e| e.to_string())?;
                to_json(&out)
            }
            "query_script" => {
                let a: QueryScriptArgs = parse(args, self.lang)?;
                self.worker.query_on(doc, a.script).await.map_err(|e| e.to_string())
            }
            "new_document" => {
                let a: NewDocumentArgs = parse(args, self.lang)?;
                let out = NewDocumentTool::new(self.worker.clone()).run(a).await.map_err(|e| e.to_string())?;
                to_json(&out)
            }
            "open_document" => {
                let a: OpenDocumentArgs = parse(args, self.lang)?;
                let out = OpenDocumentTool::new(self.worker.clone(), self.workspace.clone())
                    .run(a)
                    .await
                    .map_err(|e| e.to_string())?;
                to_json(&out)
            }
            "save_document" => {
                let a: SaveDocumentArgs = parse(args, self.lang)?;
                let out = SaveDocumentTool::new(self.worker.clone(), self.workspace.clone())
                    .run_for(a, doc)
                    .await
                    .map_err(|e| e.to_string())?;
                to_json(&out)
            }
            "close_document" => {
                let a: CloseDocumentArgs = parse(args, self.lang)?;
                let out = CloseDocumentTool::new(self.worker.clone()).run_for(a, doc).await.map_err(|e| e.to_string())?;
                to_json(&out)
            }
            "list_workspace" => {
                let a: ListWorkspaceArgs = parse(args, self.lang)?;
                let out = ListWorkspaceTool::new(self.worker.clone(), self.workspace.clone())
                    .run(a)
                    .await
                    .map_err(|e| e.to_string())?;
                to_json(&out)
            }
            other => Err(match self.lang {
                Lang::Zh => format!("工具 {other:?} 未实现"),
                Lang::En => format!("Tool {other:?} is not implemented"),
            }),
        }
    }
}

/// 支持可选 doc_id 的四个工具。
const DOC_ID_TOOLS: [&str; 4] = ["execute_script", "query_script", "save_document", "close_document"];

fn add_doc_id_property(schema: &mut JsonValue, lang: Lang) {
    let desc = match lang {
        Lang::Zh => "目标文档 id(如 \"d1\",取自状态锚的 open_documents);省略则作用在当前文档 active_document",
        Lang::En => "Target document id (for example \"d1\", taken from open_documents in the state anchor); omit to act on the active document",
    };
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.insert("doc_id".to_string(), serde_json::json!({ "type": "string", "description": desc }));
    }
}

fn parse<T: DeserializeOwned>(args: JsonValue, lang: Lang) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| match lang {
        Lang::Zh => format!("参数不正确:{e}"),
        Lang::En => format!("Invalid arguments: {e}"),
    })
}

fn to_json<T: Serialize>(v: &T) -> Result<JsonValue, String> {
    serde_json::to_value(v).map_err(|e| format!("result serialization failed: {e}"))
}

fn spec<T: Tool>(public: &'static str, tool: &T) -> ToolSpec {
    let schema_text = rewrite_text(&tool.parameters().to_string());
    let schema = serde_json::from_str(&schema_text).unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
    ToolSpec {
        name: public,
        description: rewrite_text(&tool.description()),
        schema,
        hints: hints_for(public).unwrap_or(ToolHints { read_only: false, destructive: true }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_adapter::PUBLIC_TOOLS;
    use serde_json::json;

    fn core() -> McpCore {
        let ws = Arc::new(
            WorkspaceContext::open_or_create(&tempfile::tempdir().expect("temp dir").keep()).expect("ws"),
        );
        let worker = Arc::new(DocWorker::spawn(ws.clone()).expect("spawn"));
        McpCore::new(worker, ws)
    }

    #[test]
    fn specs_match_public_tools_and_hide_old_names() {
        let specs = core().specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name).collect();
        assert_eq!(names, PUBLIC_TOOLS.to_vec());
        for s in &specs {
            let all = format!("{} {} {}", s.name, s.description, s.schema);
            assert!(!all.contains("execute_script_preview") && !all.contains("query_script_preview"), "{}", s.name);
            assert_eq!(s.schema["type"], "object", "{}", s.name);
            assert!(!s.description.is_empty(), "{}", s.name);
        }
        let exec = specs.iter().find(|s| s.name == "execute_script").unwrap();
        assert_eq!(exec.schema["required"][0], "script");
        assert!(specs.iter().find(|s| s.name == "query_script").unwrap().hints.read_only);
        assert!(specs.iter().find(|s| s.name == "close_document").unwrap().hints.destructive);
    }

    #[tokio::test]
    async fn new_document_returns_anchor() {
        let c = core();
        let o = c.call("new_document", json!({"kind": "calc", "name": "t"})).await;
        assert!(!o.is_error, "{}", o.body);
        assert_eq!(o.body["active_document"]["name"], "t");
        assert_eq!(o.body["active_document"]["kind"], "calc");
        assert_eq!(o.body["open_documents"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn execute_then_query_then_save_then_close() {
        let c = core();
        c.call("new_document", json!({"kind": "calc", "name": "t"})).await;
        let e = c.call("execute_script", json!({"script": "set_cell(0, 0, 0, \"42\")"})).await;
        assert!(!e.is_error, "{}", e.body);
        let q = c.call("query_script", json!({"script": "get_cell(0, 0, 0)"})).await;
        assert!(!q.is_error && q.body["result"].to_string().contains("42"), "{}", q.body);
        let s = c.call("save_document", json!({})).await;
        assert!(!s.is_error, "{}", s.body);
        let saved = s.body["result"]["virtual_path"].as_str().unwrap().to_string();
        assert!(saved.starts_with("/exports/t_") && saved.ends_with("_v1.xlsx"), "{saved}");
        let l = c.call("list_workspace", json!({"dir": "exports"})).await;
        assert!(!l.is_error && l.body["result"].to_string().contains(&saved), "{}", l.body);
        let x = c.call("close_document", json!({})).await;
        assert!(!x.is_error, "{}", x.body);
        assert!(x.body["active_document"].is_null());
    }

    #[tokio::test]
    async fn bad_script_is_tool_error_with_anchor() {
        let c = core();
        c.call("new_document", json!({"kind": "calc"})).await;
        let o = c.call("execute_script", json!({"script": "this is not rhai ("})).await;
        assert!(o.is_error);
        assert!(!o.body["error"].as_str().unwrap_or("").is_empty());
        assert_eq!(o.body["active_document"]["kind"], "calc");
    }

    #[tokio::test]
    async fn no_document_query_is_error_with_null_anchor() {
        let o = core().call("query_script", json!({"script": "1"})).await;
        assert!(o.is_error);
        assert!(o.body["active_document"].is_null());
    }

    #[tokio::test]
    async fn unknown_and_old_names_and_bad_args_are_errors() {
        let c = core();
        assert!(c.call("execute_script_preview", json!({"script": "1"})).await.is_error);
        assert!(c.call("nope", json!({})).await.is_error);
        assert!(c.call("new_document", json!({})).await.is_error); // 缺 kind
        assert!(c.call("new_document", JsonValue::Null).await.is_error); // 缺 kind,Null 按 {} 处理
        assert!(c.call("execute_script", json!({"script": 5})).await.is_error);
    }

    #[tokio::test]
    async fn open_rejects_path_outside_inputs_exports() {
        let c = core();
        let o = c.call("open_document", json!({"path": "/etc/passwd"})).await;
        assert!(o.is_error);
    }

    /// 并发调用必须串行:每个返回体的状态锚只反映自己这一步之后的文档栈。
    /// 8 个并发 new_document,锚里的 open_documents 长度应恰好是 1..=8 各一次。
    #[tokio::test]
    async fn concurrent_calls_are_serialized_each_anchor_sees_only_its_own_effect() {
        const N: usize = 8;
        let c = core();
        let futs = (0..N).map(|i| {
            let c = &c;
            async move {
                let o = c
                    .call("new_document", json!({"kind": "calc", "name": format!("t{i}")}))
                    .await;
                assert!(!o.is_error, "{}", o.body);
                o.body["open_documents"].as_array().expect("open_documents").len()
            }
        });
        let mut lens: Vec<usize> = futures::future::join_all(futs).await;
        lens.sort_unstable();
        assert_eq!(lens, (1..=N).collect::<Vec<_>>());
    }

    fn core_lang(lang: Lang) -> McpCore {
        core().with_lang(lang)
    }

    fn has_cjk(s: &str) -> bool {
        s.chars().any(|c| {
            matches!(c, '\u{3000}'..='\u{303f}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}' | '\u{201c}' | '\u{201d}')
        })
    }

    #[test]
    fn default_lang_is_zh_and_english_specs_differ() {
        let zh = core().specs();
        let en = core_lang(Lang::En).specs();
        assert_eq!(zh.len(), en.len());
        assert!(has_cjk(&zh[0].description), "默认应是中文");
        for (z, e) in zh.iter().zip(en.iter()) {
            assert_eq!(z.name, e.name);
            assert_ne!(z.description, e.description, "{}", z.name);
        }
    }

    /// 英文版整份清单无中文;schema 结构必须与中文版一致(防一边改参数另一边漏改)。
    #[test]
    fn english_specs_have_no_cjk_and_keep_schema_shape() {
        let zh = core().specs();
        let en = core_lang(Lang::En).specs();
        let keys = |v: &JsonValue| -> Vec<String> {
            let mut k: Vec<String> =
                v["properties"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
            k.sort();
            k
        };
        for (z, e) in zh.iter().zip(en.iter()) {
            let all = format!("{} {} {}", e.name, e.description, e.schema);
            assert!(!has_cjk(&all), "{} 含中文字符", e.name);
            assert_eq!(z.schema["required"], e.schema["required"], "{} required", z.name);
            assert_eq!(keys(&z.schema), keys(&e.schema), "{} properties", z.name);
            for k in keys(&z.schema) {
                assert_eq!(z.schema["properties"][&k]["type"], e.schema["properties"][&k]["type"], "{}.{} type", z.name, k);
                assert_eq!(z.schema["properties"][&k]["enum"], e.schema["properties"][&k]["enum"], "{}.{} enum", z.name, k);
            }
            assert_eq!(z.hints, e.hints, "{} hints", z.name);
        }
    }

    #[tokio::test]
    async fn english_mode_errors_have_no_cjk() {
        let c = core_lang(Lang::En);
        for (name, args) in [
            ("nope", json!({})),
            ("execute_script_preview", json!({"script": "1"})),
            ("new_document", json!({})),
            ("execute_script", json!({"script": 5})),
        ] {
            let o = c.call(name, args).await;
            assert!(o.is_error, "{name}");
            assert!(!has_cjk(&o.body.to_string()), "{name}: {}", o.body);
        }
    }

    #[tokio::test]
    async fn chinese_mode_errors_stay_chinese() {
        let o = core().call("nope", json!({})).await;
        assert!(o.is_error);
        assert!(o.body["error"].as_str().unwrap().contains("未知工具"), "{}", o.body);
    }
}

#[cfg(test)]
mod doc_id_tests {
    use super::*;
    use serde_json::json;

    fn core(lang: Lang) -> McpCore {
        let ws = Arc::new(
            WorkspaceContext::open_or_create(&tempfile::tempdir().expect("temp dir").keep()).expect("ws"),
        );
        let worker = Arc::new(DocWorker::spawn(ws.clone()).expect("spawn"));
        McpCore::new(worker, ws).with_lang(lang)
    }

    fn has_cjk(s: &str) -> bool {
        s.chars().any(|c| {
            matches!(c, '\u{3000}'..='\u{303f}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}' | '\u{201c}' | '\u{201d}')
        })
    }

    #[tokio::test]
    async fn anchor_carries_ids() {
        let c = core(Lang::Zh);
        let o = c.call("new_document", json!({"kind": "calc", "name": "a"})).await;
        assert!(!o.is_error, "{}", o.body);
        assert_eq!(o.body["active_document"]["id"], "d1");
        assert_eq!(o.body["open_documents"][0]["id"], "d1");
        let o = c.call("new_document", json!({"kind": "writer", "name": "b"})).await;
        assert_eq!(o.body["active_document"]["id"], "d2");
        assert_eq!(o.body["open_documents"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn doc_id_routes_execute_query_save_and_close() {
        let c = core(Lang::En);
        c.call("new_document", json!({"kind": "calc", "name": "first"})).await;
        c.call("new_document", json!({"kind": "calc", "name": "second"})).await;
        let e = c.call("execute_script", json!({"script": "set_cell(0, 0, 0, \"alpha\")", "doc_id": "d1"})).await;
        assert!(!e.is_error, "{}", e.body);
        assert_eq!(e.body["active_document"]["id"], "d2", "doc_id 不改变当前文档");
        let e = c.call("execute_script", json!({"script": "set_cell(0, 0, 0, \"beta\")"})).await;
        assert!(!e.is_error, "{}", e.body);
        let q1 = c.call("query_script", json!({"script": "get_cell(0, 0, 0)", "doc_id": "d1"})).await;
        let q1 = q1.body["result"].to_string();
        assert!(q1.contains("alpha") && !q1.contains("beta"), "{q1}");
        let q2 = c.call("query_script", json!({"script": "get_cell(0, 0, 0)"})).await;
        let q2 = q2.body["result"].to_string();
        assert!(q2.contains("beta") && !q2.contains("alpha"), "{q2}");
        let s = c.call("save_document", json!({"doc_id": "d1"})).await;
        assert!(!s.is_error, "{}", s.body);
        let saved = s.body["result"]["virtual_path"].as_str().unwrap();
        assert!(saved.starts_with("/exports/first_") && saved.ends_with("_v1.xlsx"), "{saved}");
        let x = c.call("close_document", json!({"doc_id": "d1"})).await;
        assert!(!x.is_error, "{}", x.body);
        assert_eq!(x.body["open_documents"].as_array().unwrap().len(), 1);
        assert_eq!(x.body["active_document"]["id"], "d2");
    }

    #[tokio::test]
    async fn unknown_doc_id_is_a_localized_error_and_changes_nothing() {
        for (lang, needle) in [(Lang::Zh, "找不到"), (Lang::En, "No open document")] {
            let c = core(lang);
            c.call("new_document", json!({"kind": "calc", "name": "a"})).await;
            for tool in ["execute_script", "query_script", "save_document", "close_document"] {
                let o = c.call(tool, json!({"script": "1", "doc_id": "d9"})).await;
                assert!(o.is_error, "{tool}");
                let msg = o.body["error"].as_str().unwrap_or("");
                assert!(msg.contains(needle) && msg.contains("d1"), "{tool}: {msg}");
                if lang == Lang::En {
                    assert!(!has_cjk(&o.body.to_string()), "{tool}: {}", o.body);
                }
                assert_eq!(o.body["open_documents"].as_array().unwrap().len(), 1, "{tool}");
            }
        }
    }

    #[tokio::test]
    async fn non_string_doc_id_is_an_error_and_null_means_active() {
        let c = core(Lang::En);
        c.call("new_document", json!({"kind": "calc", "name": "a"})).await;
        let bad = c.call("query_script", json!({"script": "get_sheet_count()", "doc_id": 5})).await;
        assert!(bad.is_error && !has_cjk(&bad.body.to_string()), "{}", bad.body);
        let ok = c.call("query_script", json!({"script": "get_sheet_count()", "doc_id": null})).await;
        assert!(!ok.is_error, "{}", ok.body);
    }

    #[test]
    fn four_tools_advertise_optional_doc_id_and_others_do_not() {
        let four = ["execute_script", "query_script", "save_document", "close_document"];
        for lang in [Lang::Zh, Lang::En] {
            for s in core(lang).specs() {
                let has = s.schema["properties"].get("doc_id").is_some();
                assert_eq!(has, four.contains(&s.name), "{} {:?}", s.name, lang);
                if has {
                    assert_eq!(s.schema["properties"]["doc_id"]["type"], "string");
                    let req = s.schema["required"].as_array().cloned().unwrap_or_default();
                    assert!(!req.iter().any(|r| r == "doc_id"), "{} 不应把 doc_id 设为必填", s.name);
                    if lang == Lang::En {
                        assert!(!has_cjk(&s.schema.to_string()), "{}", s.name);
                    }
                }
            }
        }
    }

    #[test]
    fn instructions_explain_doc_id_in_both_languages() {
        assert!(crate::mcp_en::instructions_for(Lang::En, "/w").contains("doc_id"));
        assert!(crate::mcp_en::instructions_for(Lang::Zh, "/w").contains("doc_id"));
    }
}

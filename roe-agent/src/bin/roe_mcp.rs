//! roe-mcp:ROE 的 MCP Server(stdio)。外壳只做协议转换,逻辑在 roe_agent::mcp_core。
//! stdout 只能出协议帧,所有日志一律走 stderr。
//!
//! 参数:--workspace <目录>(默认 $HOME/.roe-mcp/workspace,不与 GUI 工作区共用)。
//!       --lang <en|zh>(默认 en):工具描述、instructions 与服务层错误文案的语言。

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};

use roe_agent::mcp_en::{instructions_for, Lang};
use roe_agent::mcp_core::McpCore;
use roe_agent::worker::DocWorker;
use roe_agent::workspace::WorkspaceContext;

struct RoeMcp {
    core: McpCore,
    instructions: String,
}

impl ServerHandler for RoeMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("roe-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(self.instructions.clone())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools: Vec<Tool> = self
            .core
            .specs()
            .into_iter()
            .map(|s| {
                let schema = match s.schema {
                    serde_json::Value::Object(m) => m,
                    _ => Default::default(),
                };
                let ann = ToolAnnotations::new().read_only(s.hints.read_only).destructive(s.hints.destructive);
                Tool::new(s.name, s.description, schema).with_annotations(ann)
            })
            .collect();
        let mut r = ListToolsResult::default();
        r.tools = tools;
        Ok(r)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = request.arguments.map(serde_json::Value::Object).unwrap_or(serde_json::Value::Null);
        let out = self.core.call(request.name.as_ref(), args).await;
        let content = vec![ContentBlock::text(out.body.to_string())];
        let result = if out.is_error { CallToolResult::error(content) } else { CallToolResult::success(content) };
        Ok(result.into())
    }
}

fn parse_workspace_root() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    if let Some(p) = args.iter().position(|a| a == "--workspace").and_then(|i| args.get(i + 1)) {
        return PathBuf::from(p);
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".roe-mcp")
        .join("workspace")
}

/// --lang <en|zh>;缺省为 en。写错或缺值直接报错退出,不静默回落。
fn parse_lang() -> Result<Lang, String> {
    let args: Vec<String> = std::env::args().collect();
    let Some(i) = args.iter().position(|a| a == "--lang") else { return Ok(Lang::En) };
    let v = args.get(i + 1).ok_or("--lang needs a value (en or zh)")?;
    Lang::parse(v).ok_or_else(|| format!("--lang must be en or zh, got {v:?}"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = parse_workspace_root();
    let lang = parse_lang()?;
    let ws = WorkspaceContext::open_or_create(&root)
        .map_err(|e| format!("无法打开工作区 {}: {e}", root.display()))?;
    let abs = std::fs::canonicalize(&root).unwrap_or(root.clone());
    eprintln!("[roe-mcp] workspace = {}", abs.display());
    let ws = Arc::new(ws);
    let worker = Arc::new(DocWorker::spawn(ws.clone())?);
    let handler = RoeMcp {
        core: McpCore::new(worker, ws).with_lang(lang),
        instructions: instructions_for(lang, &abs.display().to_string()),
    };
    let running = handler
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| format!("MCP 初始化失败: {e:?}"))?;
    running.waiting().await.map_err(|e| format!("MCP 服务异常退出: {e:?}"))?;
    Ok(())
}

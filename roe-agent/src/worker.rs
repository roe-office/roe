//! DocWorker:无头模式的"文档工作线程"。
//!
//! roe_api 的文档句柄表是 thread_local,文档只能在创建它的线程上访问。
//! 无头 Agent 没有 UI 线程可借,所以起一个专属线程持有所有文档;
//! 其他线程(tokio、MCP)只通过 mpsc 发命令、用 oneshot 收结果。
//!
//! 文档按栈管理:new/open 之后新文档成为"当前文档",execute/query/save
//! 都作用于当前文档;close_current 关闭当前文档并回到上一个。
//! 每条命令包了 catch_unwind:脚本 panic 时线程不退出,调用方收到错误。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{mpsc, Arc};

use futures::channel::oneshot;
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::Value as JsonValue;

use roe_api::document::{
    close_document_impl, new_document_impl, open_document_bytes_impl, save_document_ooxml_impl,
};
use roe_api::script::readonly_query_script_impl;
use roe_api::script_export::execute_script_with_exports;

use crate::executor::{ExecuteOutcome, ScriptExecError, ScriptExecutor};
use crate::workspace::WorkspaceContext;

/// 无头模式支持的文档类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Writer,
    Calc,
    Impress,
}

impl DocKind {
    /// new_document_impl 的 doc_type:1=Writer 2=Calc 3=Impress。
    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            1 => Some(Self::Writer),
            2 => Some(Self::Calc),
            3 => Some(Self::Impress),
            _ => None,
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Writer => 1,
            Self::Calc => 2,
            Self::Impress => 3,
        }
    }

    /// 保存用的 OOXML format_hint:docx=4 xlsx=5 pptx=6。
    pub fn ooxml_hint(self) -> i32 {
        match self {
            Self::Writer => 4,
            Self::Calc => 5,
            Self::Impress => 6,
        }
    }

    /// 保存产物的扩展名(不含点)。
    pub fn ext(self) -> &'static str {
        match self {
            Self::Writer => "docx",
            Self::Calc => "xlsx",
            Self::Impress => "pptx",
        }
    }

    /// 按扩展名(不含点,不分大小写)判断能打开的格式,返回 (文档类型, open 用的 format_hint)。
    pub fn from_extension(ext: &str) -> Option<(Self, i32)> {
        match ext.to_ascii_lowercase().as_str() {
            "docx" => Some((Self::Writer, 4)),
            "xlsx" => Some((Self::Calc, 5)),
            "csv" => Some((Self::Calc, 10)),
            "pptx" => Some((Self::Impress, 6)),
            _ => None,
        }
    }
}

/// 一个无头文档的摘要(不暴露 handle)。
#[derive(Debug, Clone, Serialize)]
pub struct DocSummary {
    /// 稳定 id("d1"、"d2"…):进程内单调递增,关闭后不复用。
    pub id: String,
    pub kind: DocKind,
    pub name: String,
    pub current: bool,
}

/// 保存得到的字节。落盘由调用方(Tool 层)经工作区完成。
#[derive(Debug)]
pub struct SavedBytes {
    pub bytes: Vec<u8>,
    pub kind: DocKind,
    /// 文档的显示名(新建时给的名字,或打开时的原文件名)。
    pub name: String,
}

type Reply<T, E = String> = oneshot::Sender<Result<T, E>>;

enum Cmd {
    New { kind: DocKind, name: String, reply: Reply<DocSummary> },
    Open { bytes: Vec<u8>, kind: DocKind, hint: i32, name: String, reply: Reply<DocSummary> },
    Save { doc: Option<String>, reply: Reply<SavedBytes> },
    Close { doc: Option<String>, reply: Reply<DocSummary> },
    List { reply: Reply<Vec<DocSummary>> },
    Execute { doc: Option<String>, script: String, reply: Reply<ExecuteOutcome, ScriptExecError> },
    Query { doc: Option<String>, script: String, reply: Reply<JsonValue, ScriptExecError> },
}

struct Doc {
    /// 稳定 id(进程内单调递增,关闭后不复用),对外写成 "d<数字>"。
    id: u64,
    handle: u64,
    kind: DocKind,
    name: String,
}

struct State {
    docs: Vec<Doc>,
    workspace: Arc<WorkspaceContext>,
    next_id: u64,
}

fn doc_id_str(n: u64) -> String {
    format!("d{n}")
}

impl State {
    fn summary_of(&self, idx: usize) -> DocSummary {
        let d = &self.docs[idx];
        DocSummary { id: doc_id_str(d.id), kind: d.kind, name: d.name.clone(), current: idx + 1 == self.docs.len() }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// 定位目标文档:None = 当前文档(栈顶);Some(id) = 指定 id,不改变栈。
/// Err(None) 表示一份文档都没有(沿用旧语义);Err(Some(msg)) 表示 id 不存在。
fn locate(st: &State, doc: &Option<String>) -> Result<usize, Option<String>> {
    match doc {
        None => {
            if st.docs.is_empty() {
                Err(None)
            } else {
                Ok(st.docs.len() - 1)
            }
        }
        Some(id) => {
            let id = id.trim();
            st.docs.iter().position(|d| doc_id_str(d.id) == id).ok_or_else(|| {
                let ids: Vec<String> = st.docs.iter().map(|d| doc_id_str(d.id)).collect();
                Some(format!("找不到 id 为 {id:?} 的文档;当前打开的文档 id: [{}]", ids.join(", ")))
            })
        }
    }
}

fn run(st: &mut State, cmd: Cmd) {
    match cmd {
        Cmd::New { kind, name, reply } => {
            let handle = new_document_impl(kind.code());
            let r = if handle == 0 {
                Err("创建文档失败".to_string())
            } else {
                let id = st.alloc_id();
                st.docs.push(Doc { id, handle, kind, name });
                Ok(st.summary_of(st.docs.len() - 1))
            };
            let _ = reply.send(r);
        }
        Cmd::Open { bytes, kind, hint, name, reply } => {
            let handle = open_document_bytes_impl(&bytes, hint);
            let r = if handle == 0 {
                Err(format!("无法打开 {name}:文件损坏或格式不受支持"))
            } else {
                let id = st.alloc_id();
                st.docs.push(Doc { id, handle, kind, name });
                Ok(st.summary_of(st.docs.len() - 1))
            };
            let _ = reply.send(r);
        }
        Cmd::Save { doc, reply } => {
            let r = match locate(st, &doc) {
                Err(None) => Err("没有打开的文档".to_string()),
                Err(Some(msg)) => Err(msg),
                Ok(i) => {
                    let d = &st.docs[i];
                    save_document_ooxml_impl(d.handle, d.kind.ooxml_hint())
                        .map(|bytes| SavedBytes { bytes, kind: d.kind, name: d.name.clone() })
                }
            };
            let _ = reply.send(r);
        }
        Cmd::Close { doc, reply } => {
            let r = match locate(st, &doc) {
                Err(None) => Err("没有打开的文档".to_string()),
                Err(Some(msg)) => Err(msg),
                Ok(i) => {
                    let d = st.docs.remove(i);
                    close_document_impl(d.handle);
                    Ok(DocSummary { id: doc_id_str(d.id), kind: d.kind, name: d.name, current: false })
                }
            };
            let _ = reply.send(r);
        }
        Cmd::List { reply } => {
            let v = (0..st.docs.len()).map(|i| st.summary_of(i)).collect();
            let _ = reply.send(Ok(v));
        }
        Cmd::Execute { doc, script, reply } => {
            let r = match locate(st, &doc) {
                Err(None) => Err(ScriptExecError::NoActiveDocument),
                Err(Some(msg)) => Err(ScriptExecError::Script(msg)),
                Ok(i) => {
                    let d = &st.docs[i];
                    let media: &dyn roe_script::media_context::MediaAssetSource = &*st.workspace;
                    match execute_script_with_exports(d.handle, &script, Some(media)) {
                        None => Err(ScriptExecError::InvalidHandle(d.handle)),
                        Some(Err(e)) => Err(ScriptExecError::Script(e)),
                        Some(Ok((dirty_pages, dirty_regions, script_result, rep))) => {
                            let (exports, export_errors) = crate::executor::convert_report(rep);
                            Ok(ExecuteOutcome { dirty_pages, dirty_regions, script_result, exports, export_errors })
                        }
                    }
                }
            };
            let _ = reply.send(r);
        }
        Cmd::Query { doc, script, reply } => {
            let r = match locate(st, &doc) {
                Err(None) => Err(ScriptExecError::NoActiveDocument),
                Err(Some(msg)) => Err(ScriptExecError::Script(msg)),
                Ok(i) => {
                    let d = &st.docs[i];
                    match readonly_query_script_impl(d.handle, &script) {
                        None => Err(ScriptExecError::InvalidHandle(d.handle)),
                        Some(Err(e)) => Err(ScriptExecError::Script(e)),
                        Some(Ok(v)) => Ok(v),
                    }
                }
            };
            let _ = reply.send(r);
        }
    }
}

/// 文档工作线程的句柄。可 clone 进 Arc 在各线程共享;全部句柄 drop 后线程退出并关闭所有文档。
pub struct DocWorker {
    tx: mpsc::Sender<Cmd>,
}

const GONE: &str = "文档工作线程已退出,或脚本导致其内部错误";

impl DocWorker {
    pub fn spawn(workspace: Arc<WorkspaceContext>) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("roe-doc-worker".into())
            .spawn(move || {
                let mut st = State { docs: Vec::new(), workspace, next_id: 1 };
                while let Ok(cmd) = rx.recv() {
                    // panic 时 cmd 里的 reply 被丢弃,调用方收到 GONE,线程继续服务。
                    let _ = catch_unwind(AssertUnwindSafe(|| run(&mut st, cmd)));
                }
                for d in st.docs.drain(..) {
                    close_document_impl(d.handle);
                }
            })?;
        Ok(Self { tx })
    }

    fn ask<T, E>(
        &self,
        make: impl FnOnce(Reply<T, E>) -> Cmd,
        gone: fn() -> E,
    ) -> BoxFuture<'static, Result<T, E>>
    where
        T: Send + 'static,
        E: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let sent = self.tx.send(make(tx)).is_ok();
        Box::pin(async move {
            if !sent {
                return Err(gone());
            }
            match rx.await {
                Ok(r) => r,
                Err(_) => Err(gone()),
            }
        })
    }

    /// 新建空白文档,成为当前文档。
    pub async fn new_document(&self, kind: DocKind, name: impl Into<String>) -> Result<DocSummary, String> {
        let name = name.into();
        self.ask(move |reply| Cmd::New { kind, name, reply }, || GONE.to_string()).await
    }

    /// 从字节打开文档,成为当前文档。hint 见 DocKind::from_extension。
    pub async fn open_document(
        &self,
        bytes: Vec<u8>,
        kind: DocKind,
        hint: i32,
        name: impl Into<String>,
    ) -> Result<DocSummary, String> {
        let name = name.into();
        self.ask(move |reply| Cmd::Open { bytes, kind, hint, name, reply }, || GONE.to_string()).await
    }

    /// 把当前文档序列化为 OOXML 字节(不落盘)。
    pub async fn save_document(&self) -> Result<SavedBytes, String> {
        self.save_document_of(None).await
    }

    /// 关闭当前文档,回到上一个(若有)。
    pub async fn close_current(&self) -> Result<DocSummary, String> {
        self.close_document_of(None).await
    }

    /// 保存指定 id 的文档(None = 当前文档)。不改变当前文档。
    pub async fn save_document_of(&self, doc: Option<String>) -> Result<SavedBytes, String> {
        self.ask(move |reply| Cmd::Save { doc, reply }, || GONE.to_string()).await
    }

    /// 关闭指定 id 的文档(None = 当前文档)。关闭非栈顶文档时,当前文档不变。
    pub async fn close_document_of(&self, doc: Option<String>) -> Result<DocSummary, String> {
        self.ask(move |reply| Cmd::Close { doc, reply }, || GONE.to_string()).await
    }

    /// 在指定 id 的文档上执行写脚本(None = 当前文档)。不改变当前文档。
    pub fn execute_on(
        &self,
        doc: Option<String>,
        script: String,
    ) -> BoxFuture<'static, Result<ExecuteOutcome, ScriptExecError>> {
        self.ask(move |reply| Cmd::Execute { doc, script, reply }, || ScriptExecError::Script(GONE.to_string()))
    }

    /// 在指定 id 的文档上执行只读查询(None = 当前文档)。
    pub fn query_on(
        &self,
        doc: Option<String>,
        script: String,
    ) -> BoxFuture<'static, Result<JsonValue, ScriptExecError>> {
        self.ask(move |reply| Cmd::Query { doc, script, reply }, || ScriptExecError::Script(GONE.to_string()))
    }

    pub async fn list_documents(&self) -> Result<Vec<DocSummary>, String> {
        self.ask(|reply| Cmd::List { reply }, || GONE.to_string()).await
    }
}

impl ScriptExecutor for DocWorker {
    fn execute(&self, script: String) -> BoxFuture<'static, Result<ExecuteOutcome, ScriptExecError>> {
        self.execute_on(None, script)
    }

    fn query(&self, script: String) -> BoxFuture<'static, Result<JsonValue, ScriptExecError>> {
        self.query_on(None, script)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker() -> DocWorker {
        let ws = WorkspaceContext::open_or_create(&tempfile::tempdir().expect("temp dir").keep())
            .expect("open workspace");
        DocWorker::spawn(Arc::new(ws)).expect("spawn worker")
    }

    #[test]
    fn kind_mapping() {
        assert_eq!(DocKind::from_code(2), Some(DocKind::Calc));
        assert_eq!(DocKind::from_code(0), None);
        assert_eq!(DocKind::Impress.ooxml_hint(), 6);
        assert_eq!(DocKind::Writer.ext(), "docx");
        assert_eq!(DocKind::from_extension("XLSX"), Some((DocKind::Calc, 5)));
        assert_eq!(DocKind::from_extension("csv"), Some((DocKind::Calc, 10)));
        assert_eq!(DocKind::from_extension("odt"), None);
    }

    #[tokio::test]
    async fn no_document_errors() {
        let w = worker();
        assert!(matches!(w.execute("1".into()).await, Err(ScriptExecError::NoActiveDocument)));
        assert!(matches!(w.query("1".into()).await, Err(ScriptExecError::NoActiveDocument)));
        assert!(w.save_document().await.is_err());
        assert!(w.close_current().await.is_err());
        assert!(w.list_documents().await.unwrap().is_empty());
    }

    /// 新建 -> 写 42 -> 保存 -> 原文档改成 99 -> 打开保存的字节;
    /// 当前文档应是重新打开的那份(42,不是 99)。
    #[tokio::test]
    async fn calc_save_and_reopen_roundtrip() {
        let w = worker();
        w.new_document(DocKind::Calc, "t").await.unwrap();
        w.execute(r#"set_cell(0, 0, 0, "42")"#.into()).await.unwrap();
        let saved = w.save_document().await.unwrap();
        assert_eq!(&saved.bytes[..2], b"PK");
        assert_eq!(saved.kind, DocKind::Calc);
        w.execute(r#"set_cell(0, 0, 0, "99")"#.into()).await.unwrap();

        let (kind, hint) = DocKind::from_extension("xlsx").unwrap();
        w.open_document(saved.bytes, kind, hint, "back.xlsx").await.unwrap();
        let docs = w.list_documents().await.unwrap();
        assert_eq!(docs.len(), 2);
        assert!(docs[1].current && !docs[0].current);
        let v = w.query("get_cell(0, 0, 0)".into()).await.unwrap().to_string();
        assert!(v.contains("42") && !v.contains("99"), "got {v}");
    }

    #[tokio::test]
    async fn writer_and_impress_can_be_created_and_saved() {
        let w = worker();
        for (kind, ext) in [(DocKind::Writer, "docx"), (DocKind::Impress, "pptx")] {
            w.new_document(kind, "x").await.unwrap();
            let s = w.save_document().await.unwrap();
            assert_eq!(&s.bytes[..2], b"PK", "{ext}");
            assert_eq!(s.kind.ext(), ext);
            w.close_current().await.unwrap();
        }
    }

    #[tokio::test]
    async fn documents_form_a_stack() {
        let w = worker();
        w.new_document(DocKind::Calc, "A").await.unwrap();
        w.execute(r#"set_cell(0, 0, 0, "A1")"#.into()).await.unwrap();
        w.new_document(DocKind::Calc, "B").await.unwrap();
        w.execute(r#"set_cell(0, 0, 0, "B1")"#.into()).await.unwrap();
        assert!(w.query("get_cell(0, 0, 0)".into()).await.unwrap().to_string().contains("B1"));

        let closed = w.close_current().await.unwrap();
        assert_eq!(closed.name, "B");
        assert!(w.query("get_cell(0, 0, 0)".into()).await.unwrap().to_string().contains("A1"));

        w.close_current().await.unwrap();
        assert!(matches!(w.query("1".into()).await, Err(ScriptExecError::NoActiveDocument)));
    }

    #[tokio::test]
    async fn open_garbage_is_error_and_keeps_current() {
        let w = worker();
        w.new_document(DocKind::Calc, "A").await.unwrap();
        assert!(w.open_document(b"not a file".to_vec(), DocKind::Calc, 5, "bad.xlsx").await.is_err());
        assert_eq!(w.list_documents().await.unwrap().len(), 1);
    }

    /// 关键性质:从别的线程发命令也能操作文档(文档始终只在工作线程上被访问)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn works_from_other_threads() {
        let w = Arc::new(worker());
        w.new_document(DocKind::Calc, "A").await.unwrap();
        let w2 = w.clone();
        tokio::spawn(async move { w2.execute(r#"set_cell(0, 0, 0, "7")"#.into()).await })
            .await
            .unwrap()
            .unwrap();
        assert!(w.query("get_cell(0, 0, 0)".into()).await.unwrap().to_string().contains("7"));
    }
}

#[cfg(test)]
mod doc_id_tests {
    use super::*;

    fn worker() -> DocWorker {
        let ws = Arc::new(
            WorkspaceContext::open_or_create(&tempfile::tempdir().expect("temp dir").keep()).expect("ws"),
        );
        DocWorker::spawn(ws).expect("spawn")
    }

    fn id(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[tokio::test]
    async fn ids_are_stable_unique_and_never_reused() {
        let w = worker();
        let a = w.new_document(DocKind::Calc, "a").await.unwrap();
        let b = w.new_document(DocKind::Writer, "b").await.unwrap();
        assert_eq!((a.id.as_str(), b.id.as_str()), ("d1", "d2"));
        w.close_document_of(id("d2")).await.unwrap();
        let c = w.new_document(DocKind::Calc, "c").await.unwrap();
        assert_eq!(c.id, "d3", "关闭后 id 不复用");
        let ids: Vec<String> = w.list_documents().await.unwrap().into_iter().map(|d| d.id).collect();
        assert_eq!(ids, vec!["d1", "d3"]);
    }

    #[tokio::test]
    async fn execute_and_query_target_the_named_document_only() {
        let w = worker();
        w.new_document(DocKind::Calc, "first").await.unwrap();
        w.new_document(DocKind::Calc, "second").await.unwrap();
        w.execute_on(id("d1"), "set_cell(0, 0, 0, \"alpha\")".into()).await.unwrap();
        w.execute_on(None, "set_cell(0, 0, 0, \"beta\")".into()).await.unwrap();
        let q1 = w.query_on(id("d1"), "get_cell(0, 0, 0)".into()).await.unwrap().to_string();
        let q2 = w.query_on(None, "get_cell(0, 0, 0)".into()).await.unwrap().to_string();
        assert!(q1.contains("alpha") && !q1.contains("beta"), "{q1}");
        assert!(q2.contains("beta") && !q2.contains("alpha"), "{q2}");
        let docs = w.list_documents().await.unwrap();
        assert_eq!(docs.iter().find(|d| d.current).unwrap().id, "d2", "doc_id 不改变当前文档");
    }

    #[tokio::test]
    async fn unknown_id_is_an_error_and_empty_stack_keeps_old_semantics() {
        let w = worker();
        assert!(matches!(w.execute_on(None, "1".into()).await, Err(ScriptExecError::NoActiveDocument)));
        assert!(matches!(w.query_on(None, "1".into()).await, Err(ScriptExecError::NoActiveDocument)));
        w.new_document(DocKind::Calc, "a").await.unwrap();
        assert!(matches!(w.execute_on(id("d9"), "1".into()).await, Err(ScriptExecError::Script(_))));
        assert!(matches!(w.query_on(id("d9"), "1".into()).await, Err(ScriptExecError::Script(_))));
        assert!(w.save_document_of(id("d9")).await.is_err());
        assert!(w.close_document_of(id("d9")).await.is_err());
        assert_eq!(w.list_documents().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn save_by_id_saves_the_named_document() {
        let w = worker();
        w.new_document(DocKind::Calc, "first").await.unwrap();
        w.new_document(DocKind::Writer, "second").await.unwrap();
        let a = w.save_document_of(id("d1")).await.unwrap();
        assert_eq!((a.name.as_str(), a.kind), ("first", DocKind::Calc));
        let b = w.save_document().await.unwrap();
        assert_eq!((b.name.as_str(), b.kind), ("second", DocKind::Writer));
    }

    #[tokio::test]
    async fn close_by_id_removes_only_that_document_and_keeps_active() {
        let w = worker();
        w.new_document(DocKind::Calc, "first").await.unwrap();
        w.new_document(DocKind::Calc, "second").await.unwrap();
        let closed = w.close_document_of(id("d1")).await.unwrap();
        assert_eq!(closed.name, "first");
        let docs = w.list_documents().await.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!((docs[0].id.as_str(), docs[0].current), ("d2", true));
        let top = w.close_document_of(None).await.unwrap();
        assert_eq!(top.name, "second");
    }
}

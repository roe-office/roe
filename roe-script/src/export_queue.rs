//! 导出图片请求队列(线程内)。
//!
//! 脚本里调用 export_image(name) / export_image(name, options_json) 时,
//! 这里只把请求排进队列,不做任何渲染(roe-script 不能依赖 roe-api)。
//! 脚本成功后,由 roe-api 的 execute_script_impl 取出队列,对改完后的文档出图。
//! 脚本失败回滚时,调用方应 clear() 丢弃队列。

use rhai::Engine;
use std::cell::RefCell;

#[derive(Debug, Clone, PartialEq)]
pub struct ExportRequest {
    /// 文件名前缀(不含扩展名),已校验过不含路径分隔符等非法字符。
    pub name: String,
    /// 选项 JSON(对象),空串表示全部默认。
    pub options_json: String,
}

/// 单次脚本最多排队的导出请求数。
pub const MAX_PENDING: usize = 20;
const MAX_NAME_CHARS: usize = 64;

thread_local! {
    static QUEUE: RefCell<Vec<ExportRequest>> = const { RefCell::new(Vec::new()) };
}

/// 清空队列(脚本开始前、失败回滚后调用)。
pub fn clear() {
    QUEUE.with(|q| q.borrow_mut().clear());
}

/// 取出并清空队列。
pub fn take() -> Vec<ExportRequest> {
    QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

pub fn pending_len() -> usize {
    QUEUE.with(|q| q.borrow().len())
}

/// 校验并排队。成功返回队列当前长度;失败返回带 err: 前缀的说明。
pub fn push(name: &str, options_json: &str) -> Result<usize, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("err:export_image 的 name 不能为空".to_string());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("err:export_image 的 name 过长(最多 {} 个字符)", MAX_NAME_CHARS));
    }
    if name.starts_with('.')
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
    {
        return Err("err:export_image 的 name 不能以 . 开头,也不能含 / \\ : * ? \" < > | 等字符".to_string());
    }
    let opt = options_json.trim();
    if !opt.is_empty() {
        match serde_json::from_str::<serde_json::Value>(opt) {
            Ok(v) if v.is_object() => {}
            Ok(_) => return Err("err:export_image 的 options 必须是 JSON 对象".to_string()),
            Err(e) => return Err(format!("err:export_image 的 options 不是合法 JSON: {e}")),
        }
    }
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if q.len() >= MAX_PENDING {
            return Err(format!("err:一次脚本最多排队 {} 个导出请求", MAX_PENDING));
        }
        q.push(ExportRequest { name: name.to_string(), options_json: opt.to_string() });
        Ok(q.len())
    })
}

fn to_ret(r: Result<usize, String>) -> String {
    match r {
        Ok(_) => "ok".to_string(),
        Err(m) => m,
    }
}

/// 注册 export_image 两个重载。只放进读写引擎(ScriptEngine::new),
/// 只读引擎不注册:只读环境里排队了也没人取,会造成"成功却没图"的假象。
pub fn register(engine: &mut Engine) {
    engine.register_fn("export_image", |name: String| -> String { to_ret(push(&name, "")) });
    engine.register_fn("export_image", |name: String, options: String| -> String {
        to_ret(push(&name, &options))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_take_clear() {
        clear();
        assert_eq!(push("cover", "").unwrap(), 1);
        assert_eq!(push("p1", r#"{"pages":"1-3"}"#).unwrap(), 2);
        assert_eq!(pending_len(), 2);
        let got = take();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "cover");
        assert_eq!(got[1].options_json, r#"{"pages":"1-3"}"#);
        assert_eq!(pending_len(), 0);
        push("x", "").unwrap();
        clear();
        assert_eq!(pending_len(), 0);
    }

    #[test]
    fn rejects_bad_input() {
        clear();
        assert!(push("", "").is_err());
        assert!(push("   ", "").is_err());
        assert!(push("a/b", "").is_err());
        assert!(push("a\\b", "").is_err());
        assert!(push(".hidden", "").is_err());
        assert!(push(&"长".repeat(65), "").is_err());
        assert!(push("ok", "not json").is_err());
        assert!(push("ok", "[1,2]").is_err());
        assert_eq!(pending_len(), 0);
        assert!(push("中文名", "").is_ok());
        clear();
    }

    #[test]
    fn limit_enforced() {
        clear();
        for i in 0..MAX_PENDING {
            push(&format!("n{i}"), "").unwrap();
        }
        assert!(push("overflow", "").is_err());
        assert_eq!(pending_len(), MAX_PENDING);
        clear();
    }

    #[test]
    fn rhai_overloads_work() {
        clear();
        let mut e = Engine::new();
        register(&mut e);
        assert_eq!(e.eval::<String>(r#"export_image("a")"#).unwrap(), "ok");
        assert_eq!(e.eval::<String>(r#"export_image("b", "{\"scale\":2}")"#).unwrap(), "ok");
        assert!(e.eval::<String>(r#"export_image("c/d")"#).unwrap().starts_with("err:"));
        let got = take();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].name, "b");
    }
}

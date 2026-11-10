//! 修订的接受 / 拒绝（第一版：只处理 run 的最外层标记）。
use std::os::raw::c_int;
use roe_model::writer::{BodyElement, Revision, RevisionKind, Run, WriterDocument};
use roe_parser::RoeDocument;
use crate::error::set_last_error;
use crate::handle;

/// 对一个 run 做接受/拒绝。返回 None 表示整个 run 删除，Some 为保留后的 run。
fn resolve_run(mut r: Run, accept: bool) -> Option<Run> {
    let Some(rev) = r.revision.take() else { return Some(r) };
    match (rev.kind, accept) {
        (RevisionKind::Insert, true) => Some(r),
        (RevisionKind::Insert, false) => None,
        (RevisionKind::Delete, true) => None,
        (RevisionKind::Delete, false) => {
            r.revision = rev.under.map(|b| *b);
            Some(r)
        }
    }
}

fn resolve_runs(runs: Vec<Run>, accept: bool, only: Option<usize>, base: &mut usize) -> (Vec<Run>, bool) {
    let mut out = Vec::with_capacity(runs.len());
    let mut changed = false;
    for r in runs {
        let start = *base;
        *base += r.text.len();
        let hit = r.revision.is_some()
            && match only {
                None => true,
                Some(off) => start <= off && off < (*base).max(start + 1),
            };
        if hit {
            changed = true;
            let had_ref = r.comment_ref.is_some() || r.footnote_ref.is_some() || r.endnote_ref.is_some()
                || !r.comment_range_start.is_empty() || !r.comment_range_end.is_empty();
            match resolve_run(r.clone(), accept) {
                Some(x) => out.push(x),
                None if had_ref => {
                    // 保留批注/脚注锚点，只清空文字
                    let mut keep = r;
                    keep.text.clear();
                    keep.revision = None;
                    out.push(keep);
                }
                None => {}
            }
        } else {
            out.push(r);
        }
    }
    (out, changed)
}

/// 处理光标所在位置的一条修订。返回所在段落号。
pub fn resolve_revision_at_core(doc: &mut WriterDocument, para: usize, offset: usize, accept: bool) -> Result<usize, String> {
    let Some(BodyElement::Paragraph(p)) = doc.body.get_mut(para) else {
        return Err(format!("段落 {para} 不存在或不是普通段落"));
    };
    let mut base = 0usize;
    let runs = std::mem::take(&mut p.runs);
    let (new_runs, changed) = resolve_runs(runs, accept, Some(offset), &mut base);
    p.runs = new_runs;
    if !changed { return Err("该位置没有修订".to_string()); }
    Ok(para)
}

/// 全部接受 / 全部拒绝。返回第一个有改动的段落号。
pub fn resolve_all_revisions_core(doc: &mut WriterDocument, accept: bool) -> Result<usize, String> {
    let mut first: Option<usize> = None;
    // 拒绝删除会退回到下层的插入标记，需要再来一轮；循环到没有修订为止
    // （嵌套最多两层，保险起见限制 8 轮）。
    for _ in 0..8 {
        let mut any = false;
        for (i, el) in doc.body.iter_mut().enumerate() {
            if let BodyElement::Paragraph(p) = el {
                let mut base = 0usize;
                let runs = std::mem::take(&mut p.runs);
                let (nr, ch) = resolve_runs(runs, accept, None, &mut base);
                p.runs = nr;
                if ch {
                    any = true;
                    if first.is_none() { first = Some(i); }
                }
            }
        }
        if !any { break; }
    }
    first.ok_or_else(|| "文档中没有修订".to_string())
}

fn run_op(handle_val: u64, name: &str, op: impl FnOnce(&mut WriterDocument) -> Result<usize, String>) -> c_int {
    let result = handle::with_entry(handle_val, |entry| -> Result<usize, String> {
        if !matches!(&entry.doc, RoeDocument::Writer(_)) { return Err("非 Writer 文档".to_string()); }
        let prev = entry.doc.clone();
        let r = {
            let RoeDocument::Writer(doc) = &mut entry.doc else { unreachable!() };
            op(doc)
        };
        match r {
            Ok(first) => {
                entry.push_undo_snapshot(prev, handle::EditCursorContext::at(first, 0));
                Ok(first)
            }
            Err(e) => { entry.doc = prev; Err(e) }
        }
    });
    let ok = matches!(&result, Some(Ok(_)));
    if ok { handle::invalidate_writer_layout_cache(handle_val); }
    match result {
        Some(Ok(_)) => 0,
        Some(Err(m)) => { set_last_error(format!("{name}: {m}")); -1 }
        None => { set_last_error(format!("{name}: 无效句柄")); -1 }
    }
}

/// 接受(accept=1)或拒绝(accept=0)光标处的修订。成功 0，失败 -1。带撤销快照。
#[no_mangle]
pub extern "C" fn roe_resolve_revision(handle_val: u64, para: u32, offset: u32, accept: u32) -> c_int {
    run_op(handle_val, "roe_resolve_revision", |d| resolve_revision_at_core(d, para as usize, offset as usize, accept != 0))
}

/// 全部接受 / 全部拒绝。成功 0，失败 -1。带撤销快照。
#[no_mangle]
pub extern "C" fn roe_resolve_all_revisions(handle_val: u64, accept: u32) -> c_int {
    run_op(handle_val, "roe_resolve_all_revisions", |d| resolve_all_revisions_core(d, accept != 0))
}

/// 合并后的一条修订（同段落内相邻、类型和作者都相同的 run 合成一条）。
#[derive(Debug, Clone, PartialEq)]
pub struct RevisionInfo {
    pub para: usize,
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
    pub author: String,
    pub date: String,
    pub text: String,
}

/// 列出正文顶层段落里的全部修订（字节偏移口径与 resolve_runs 一致）。
pub fn list_revisions_core(doc: &WriterDocument) -> Vec<RevisionInfo> {
    let mut out: Vec<RevisionInfo> = Vec::new();
    for (i, el) in doc.body.iter().enumerate() {
        if let BodyElement::Paragraph(p) = el {
            let mut base = 0usize;
            for r in &p.runs {
                let start = base;
                base += r.text.len();
                let Some(rev) = &r.revision else { continue };
                let kind = match rev.kind {
                    RevisionKind::Insert => "insert",
                    RevisionKind::Delete => "delete",
                };
                let merged = match out.last_mut() {
                    Some(l) if l.para == i && l.end == start && l.kind == kind && l.author == rev.author => {
                        l.end = base;
                        l.text.push_str(&r.text);
                        true
                    }
                    _ => false,
                };
                if !merged {
                    out.push(RevisionInfo {
                        para: i,
                        start,
                        end: base,
                        kind,
                        author: rev.author.clone(),
                        date: rev.date.clone(),
                        text: r.text.clone(),
                    });
                }
            }
        }
    }
    out
}

fn rev_json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

pub fn revisions_to_json(list: &[RevisionInfo]) -> String {
    let items: Vec<String> = list.iter().map(|r| format!(
        "{{\"para\":{},\"start\":{},\"end\":{},\"kind\":\"{}\",\"author\":\"{}\",\"date\":\"{}\",\"text\":\"{}\"}}",
        r.para, r.start, r.end, r.kind, rev_json_escape(&r.author), rev_json_escape(&r.date), rev_json_escape(&r.text)
    )).collect();
    format!("{{\"revisions\":[{}]}}", items.join(","))
}

/// 列出全部修订，返回 JSON 字符串，用 roe_free_string 释放。
#[no_mangle]
pub extern "C" fn roe_list_revisions(handle_val: u64) -> *mut std::os::raw::c_char {
    let result = handle::with_entry(handle_val, |entry| match &entry.doc {
        RoeDocument::Writer(doc) => revisions_to_json(&list_revisions_core(doc)),
        _ => "{\"revisions\":[]}".to_string(),
    });
    let json = result.unwrap_or_else(|| {
        set_last_error("roe_list_revisions: 无效句柄");
        "{\"revisions\":[]}".to_string()
    });
    match std::ffi::CString::new(json) {
        Ok(cs) => cs.into_raw(),
        Err(e) => {
            set_last_error(format!("roe_list_revisions: {e}"));
            std::ptr::null_mut()
        }
    }
}

/// 处理段落内字节区间 [start, end) 里所有带修订的 run（合并后的一条修订可能含多个 run）。
pub fn resolve_revision_range_core(doc: &mut WriterDocument, para: usize, start: usize, end: usize, accept: bool) -> Result<usize, String> {
    let Some(BodyElement::Paragraph(p)) = doc.body.get_mut(para) else {
        return Err(format!("段落 {para} 不存在或不是普通段落"));
    };
    let runs = std::mem::take(&mut p.runs);
    let mut out = Vec::with_capacity(runs.len());
    let mut base = 0usize;
    let mut changed = false;
    for r in runs {
        let s = base;
        base += r.text.len();
        let hit = r.revision.is_some() && s < end && base.max(s + 1) > start;
        if !hit { out.push(r); continue; }
        changed = true;
        let had_ref = r.comment_ref.is_some() || r.footnote_ref.is_some() || r.endnote_ref.is_some()
            || !r.comment_range_start.is_empty() || !r.comment_range_end.is_empty();
        match resolve_run(r.clone(), accept) {
            Some(x) => out.push(x),
            None if had_ref => {
                let mut keep = r;
                keep.text.clear();
                keep.revision = None;
                out.push(keep);
            }
            None => {}
        }
    }
    p.runs = out;
    if !changed { return Err("该区间没有修订".to_string()); }
    Ok(para)
}

/// 接受/拒绝段落内一个区间的修订。成功 0，失败 -1。带撤销快照。
#[no_mangle]
pub extern "C" fn roe_resolve_revision_range(handle_val: u64, para: u32, start: u32, end: u32, accept: u32) -> c_int {
    run_op(handle_val, "roe_resolve_revision_range", |d| resolve_revision_range_core(d, para as usize, start as usize, end as usize, accept != 0))
}

#[allow(dead_code)]
fn _keep(_: &Revision) {}

// ───────── v308: 修订模式（创建修订的基础件）─────────

/// Unix 秒 → "YYYY-MM-DDTHH:MM:SSZ"（UTC，只用标准库）。
pub fn iso_from_unix(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y0 = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y0 + 1 } else { y0 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// 当前 UTC 时间的 ISO 字符串。
pub fn now_iso_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso_from_unix(secs)
}

/// 文档正文里现有最大修订 id 加 1（含嵌套的 under 层）；没有修订返回 1。
pub fn next_revision_id(doc: &WriterDocument) -> u32 {
    fn max_in(rev: &Revision) -> u32 {
        let mut m = rev.id;
        if let Some(u) = &rev.under { m = m.max(max_in(u)); }
        m
    }
    let mut max_id = 0u32;
    for el in &doc.body {
        if let BodyElement::Paragraph(p) = el {
            for r in &p.runs {
                if let Some(rev) = &r.revision { max_id = max_id.max(max_in(rev)); }
            }
        }
    }
    max_id + 1
}

/// 设置修订模式。on: 0 关 / 非 0 开；author 可为空指针（保持原作者名不变）。
/// 返回 0 成功，非 0 失败。
#[no_mangle]
pub extern "C" fn roe_set_track_changes(handle: u64, on: i32, author: *const std::os::raw::c_char) -> i32 {
    let author_str: Option<String> = if author.is_null() {
        None
    } else {
        match unsafe { std::ffi::CStr::from_ptr(author) }.to_str() {
            Ok(s) => Some(s.to_owned()),
            Err(_) => {
                set_last_error("roe_set_track_changes: 作者名不是合法 UTF-8");
                return crate::error::RoeErrorCode::ParseError as i32;
            }
        }
    };
    let result = handle::with_entry(handle, |entry| {
        entry.writer_options.track_changes = on != 0;
        if let Some(a) = author_str {
            if !a.is_empty() { entry.writer_options.track_author = a; }
        }
    });
    match result {
        Some(()) => crate::error::RoeErrorCode::Ok as i32,
        None => {
            set_last_error("roe_set_track_changes: 无效句柄");
            crate::error::RoeErrorCode::InvalidHandle as i32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roe_model::writer::Paragraph;

    fn rev(kind: RevisionKind, a: &str, under: Option<Revision>) -> Revision {
        Revision { kind, id: 0, author: a.into(), date: String::new(), under: under.map(Box::new) }
    }
    fn run(t: &str, r: Option<Revision>) -> Run { Run { text: t.into(), revision: r, ..Default::default() } }
    fn doc_of(runs: Vec<Run>) -> WriterDocument {
        WriterDocument { body: vec![BodyElement::Paragraph(Paragraph { runs, ..Default::default() })], ..Default::default() }
    }
    fn texts(d: &WriterDocument) -> Vec<String> {
        let BodyElement::Paragraph(p) = &d.body[0] else { panic!() };
        p.runs.iter().map(|r| r.text.clone()).collect()
    }
    fn sample() -> WriterDocument {
        let alice = rev(RevisionKind::Insert, "Alice", None);
        let bob = rev(RevisionKind::Delete, "Bob", Some(alice.clone()));
        doc_of(vec![run("AAA", Some(alice.clone())), run("BBB", Some(bob)), run("CCC", Some(alice)), run("DDD", None)])
    }


    #[test]
    fn range_accept_handles_all_runs_in_span() {
        let a = rev(RevisionKind::Insert, "Alice", None);
        let mut d = doc_of(vec![run("ab", Some(a.clone())), run("cd", Some(a)), run("ef", None)]);
        resolve_revision_range_core(&mut d, 0, 0, 4, true).unwrap();
        let BodyElement::Paragraph(p) = &d.body[0] else { panic!() };
        assert!(p.runs.iter().all(|r| r.revision.is_none()));
        assert_eq!(texts(&d), vec!["ab", "cd", "ef"]);
    }
    #[test]
    fn range_reject_removes_only_span() {
        let mut d = sample();
        resolve_revision_range_core(&mut d, 0, 0, 3, false).unwrap();
        assert_eq!(texts(&d), vec!["BBB", "CCC", "DDD"]);
    }
    #[test]
    fn range_without_revision_is_error() {
        let mut d = sample();
        assert!(resolve_revision_range_core(&mut d, 0, 9, 12, true).is_err());
    }
    #[test]
    fn list_revisions_basic() {
        let d = sample();
        let l = list_revisions_core(&d);
        assert_eq!(l.len(), 3);
        assert_eq!((l[0].start, l[0].end, l[0].kind), (0, 3, "insert"));
        assert_eq!((l[1].start, l[1].end, l[1].kind), (3, 6, "delete"));
        assert_eq!((l[2].start, l[2].end, l[2].kind), (6, 9, "insert"));
        assert_eq!(l[1].author, "Bob");
    }
    #[test]
    fn list_revisions_merges_adjacent_same_author() {
        let a = rev(RevisionKind::Insert, "Alice", None);
        let d = doc_of(vec![run("ab", Some(a.clone())), run("cd", Some(a)), run("ef", None)]);
        let l = list_revisions_core(&d);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].start, l[0].end, l[0].text.as_str()), (0, 4, "abcd"));
    }
    #[test]
    fn list_revisions_json_escapes() {
        let a = rev(RevisionKind::Insert, "A\"l", None);
        let d = doc_of(vec![run("x\ny", Some(a))]);
        let j = revisions_to_json(&list_revisions_core(&d));
        assert!(j.contains("A\\\"l"));
        assert!(j.contains("x\\ny"));
        assert!(j.contains("\"date\":\"\""));
    }

    #[test]
    fn accept_insert_keeps_text_without_mark() {
        let mut d = sample();
        resolve_revision_at_core(&mut d, 0, 1, true).unwrap();
        let BodyElement::Paragraph(p) = &d.body[0] else { panic!() };
        assert_eq!(p.runs[0].text, "AAA");
        assert!(p.runs[0].revision.is_none());
    }
    #[test]
    fn reject_insert_removes_text() {
        let mut d = sample();
        resolve_revision_at_core(&mut d, 0, 1, false).unwrap();
        assert_eq!(texts(&d), vec!["BBB", "CCC", "DDD"]);
    }
    #[test]
    fn accept_delete_removes_text() {
        let mut d = sample();
        resolve_revision_at_core(&mut d, 0, 4, true).unwrap();
        assert_eq!(texts(&d), vec!["AAA", "CCC", "DDD"]);
    }
    #[test]
    fn reject_delete_falls_back_to_under_insert() {
        let mut d = sample();
        resolve_revision_at_core(&mut d, 0, 4, false).unwrap();
        let BodyElement::Paragraph(p) = &d.body[0] else { panic!() };
        assert_eq!(p.runs[1].text, "BBB");
        let r = p.runs[1].revision.as_ref().unwrap();
        assert_eq!((r.kind, r.author.as_str()), (RevisionKind::Insert, "Alice"));
    }
    #[test]
    fn reject_plain_delete_clears_mark() {
        let mut d = doc_of(vec![run("x", Some(rev(RevisionKind::Delete, "Bob", None)))]);
        resolve_revision_at_core(&mut d, 0, 0, false).unwrap();
        let BodyElement::Paragraph(p) = &d.body[0] else { panic!() };
        assert!(p.runs[0].revision.is_none());
    }
    #[test]
    fn no_revision_at_offset_is_error() {
        let mut d = sample();
        assert!(resolve_revision_at_core(&mut d, 0, 10, true).is_err());
    }
    #[test]
    fn accept_all_leaves_no_revision() {
        let mut d = sample();
        resolve_all_revisions_core(&mut d, true).unwrap();
        let BodyElement::Paragraph(p) = &d.body[0] else { panic!() };
        assert!(p.runs.iter().all(|r| r.revision.is_none()));
        assert_eq!(texts(&d), vec!["AAA", "CCC", "DDD"]);
    }
    #[test]
    fn reject_all_leaves_only_plain_and_restored() {
        let mut d = sample();
        resolve_all_revisions_core(&mut d, false).unwrap();
        assert_eq!(texts(&d), vec!["DDD"]);
        let BodyElement::Paragraph(p) = &d.body[0] else { panic!() };
        assert!(p.runs.iter().all(|r| r.revision.is_none()), "全部拒绝后不应残留修订");
    }
}

#[cfg(test)]
mod track_tests {
    use super::*;

    #[test]
    fn iso_epoch_and_known_dates() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(iso_from_unix(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn next_id_empty_and_nested() {
        let mut doc = WriterDocument::default();
        assert_eq!(next_revision_id(&doc), 1);
        let under = Revision { kind: RevisionKind::Insert, id: 7, author: "A".into(), date: String::new(), under: None };
        let top = Revision { kind: RevisionKind::Delete, id: 3, author: "B".into(), date: String::new(), under: Some(Box::new(under)) };
        let run = Run { text: "x".into(), revision: Some(top), ..Default::default() };
        let mut p = roe_model::writer::Paragraph::default();
        p.runs.push(run);
        doc.body.push(BodyElement::Paragraph(p));
        assert_eq!(next_revision_id(&doc), 8);
    }
}

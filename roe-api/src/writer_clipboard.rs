//! v247: Writer 字符级剪贴板（应用内，进程级单槽位，可跨文档）。
//! 偏移 = 段内全部 run 文本拼接后的 UTF-8 字节偏移，与格式接口一致。
//! 选区起止须是普通段落；中间的表格/图片/图表/形状整块复制删除粘贴；含分节符会拒绝。

use crate::error::{set_last_error, RoeErrorCode};
use crate::handle;
use roe_model::writer::{BodyElement, Paragraph, Revision, RevisionKind, Run};
use roe_parser::RoeDocument;
use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::c_char;

thread_local! {
    static WCLIP: RefCell<Option<Vec<BodyElement>>> = const { RefCell::new(None) };
}

fn order(pa: u32, oa: u32, pb: u32, ob: u32) -> (usize, usize, usize, usize) {
    if (pa, oa) <= (pb, ob) {
        (pa as usize, oa as usize, pb as usize, ob as usize)
    } else {
        (pb as usize, ob as usize, pa as usize, oa as usize)
    }
}

fn para_ref(body: &[BodyElement], i: usize) -> Result<&Paragraph, String> {
    match body.get(i) {
        Some(BodyElement::Paragraph(p)) => Ok(p),
        _ => Err(format!("段落 {i} 不存在或不是普通段落")),
    }
}

fn check_body(body: &[BodyElement], pa: usize, pb: usize) -> Result<(), String> {
    if pa > pb || pb >= body.len() {
        return Err("段落下标越界".to_string());
    }
    if !matches!(body[pa], BodyElement::Paragraph(_)) || !matches!(body[pb], BodyElement::Paragraph(_)) {
        return Err("选区起止必须落在普通段落内".to_string());
    }
    for i in pa..=pb {
        if matches!(body[i], BodyElement::Section(_)) {
            return Err("选区包含分节符，暂不支持".to_string());
        }
    }
    Ok(())
}

/// 在字节偏移 offset 处把 runs 切成左右两份（必要时切开一个 run）。
fn split_runs(runs: &[Run], offset: usize) -> Result<(Vec<Run>, Vec<Run>), String> {
    let mut left: Vec<Run> = Vec::new();
    let mut right: Vec<Run> = Vec::new();
    let mut acc = 0usize;
    for r in runs {
        let len = r.text.len();
        if acc + len <= offset {
            left.push(r.clone());
        } else if acc >= offset {
            right.push(r.clone());
        } else {
            let rel = offset - acc;
            if !r.text.is_char_boundary(rel) {
                return Err("偏移不在字符边界上".to_string());
            }
            let mut l = r.clone();
            let mut rr = r.clone();
            l.text = r.text[..rel].to_string();
            rr.text = r.text[rel..].to_string();
            l.comment_range_end.clear();
            rr.comment_range_start.clear();
            rr.footnote_ref = None;
            rr.endnote_ref = None;
            rr.comment_ref = None;
            left.push(l);
            right.push(rr);
        }
        acc += len;
    }
    Ok((left, right))
}

fn text_len(runs: &[Run]) -> usize {
    runs.iter().map(|r| r.text.len()).sum()
}

fn copy_in(body: &[BodyElement], pa: usize, oa: usize, pb: usize, ob: usize) -> Result<Vec<BodyElement>, String> {
    check_body(body, pa, pb)?;
    let mut out: Vec<BodyElement> = Vec::new();
    if pa == pb {
        let p = para_ref(body, pa)?;
        let right = split_runs(&p.runs, oa)?.1;
        let mid = split_runs(&right, ob - oa)?.0;
        let mut np = p.clone();
        np.runs = mid;
        out.push(BodyElement::Paragraph(np));
    } else {
        let p = para_ref(body, pa)?;
        let mut np = p.clone();
        np.runs = split_runs(&p.runs, oa)?.1;
        out.push(BodyElement::Paragraph(np));
        // 中间的段落、表格、图片、图表、形状整块复制
        for i in (pa + 1)..pb {
            out.push(body[i].clone());
        }
        let p = para_ref(body, pb)?;
        let mut np = p.clone();
        np.runs = split_runs(&p.runs, ob)?.0;
        out.push(BodyElement::Paragraph(np));
    }
    for e in out.iter_mut() {
        if let BodyElement::Paragraph(p) = e {
            p.bookmarks.clear();
        }
    }
    Ok(out)
}

fn delete_in(body: &mut Vec<BodyElement>, pa: usize, oa: usize, pb: usize, ob: usize) -> Result<(), String> {
    check_body(body, pa, pb)?;
    let np = {
        let first = para_ref(body, pa)?;
        let last = para_ref(body, pb)?;
        let mut left = split_runs(&first.runs, oa)?.0;
        let right = split_runs(&last.runs, ob)?.1;
        left.extend(right);
        let mut np = first.clone();
        np.runs = left;
        np
    };
    body[pa] = BodyElement::Paragraph(np);
    if pb > pa {
        body.drain((pa + 1)..=pb);
    }
    Ok(())
}

/// v308: 修订模式下的范围删除。区间内文字标成删除；跨段时把各段并成一段（合并本身不记修订）。
fn tracked_delete_range(doc: &mut roe_model::writer::WriterDocument, pa: usize, oa: usize, pb: usize, ob: usize, author: &str) -> Result<(), String> {
    check_body(&doc.body, pa, pb)?;
    for i in pa..=pb {
        if !matches!(doc.body[i], BodyElement::Paragraph(_)) {
            return Err("修订模式：选区内含表格或图片，暂不支持删除".to_string());
        }
    }
    let date = crate::revisions::now_iso_utc();
    for i in pa..=pb {
        let start = if i == pa { oa } else { 0 };
        let end = if i == pb { ob } else { text_len(&para_ref(&doc.body, i)?.runs) };
        let id = crate::revisions::next_revision_id(doc);
        let BodyElement::Paragraph(p) = &mut doc.body[i] else { unreachable!() };
        crate::text_edit::tracked_delete_span(p, start, end, author, id, &date)?;
    }
    if pb > pa {
        let mut tail: Vec<Run> = Vec::new();
        for i in (pa + 1)..=pb {
            tail.extend(para_ref(&doc.body, i)?.runs.clone());
        }
        let BodyElement::Paragraph(first) = &mut doc.body[pa] else { unreachable!() };
        first.runs.extend(tail);
        doc.body.drain((pa + 1)..=pb);
    }
    Ok(())
}

fn frag_para(frag: &[BodyElement], i: usize) -> Result<&Paragraph, String> {
    match frag.get(i) {
        Some(BodyElement::Paragraph(p)) => Ok(p),
        _ => Err("剪贴板内容首尾不是段落".to_string()),
    }
}

fn paste_in(body: &mut Vec<BodyElement>, frag: &[BodyElement], p: usize, o: usize) -> Result<(usize, usize), String> {
    check_body(body, p, p)?;
    let n = frag.len();
    if n == 0 {
        return Err("剪贴板为空".to_string());
    }
    let first_frag = frag_para(frag, 0)?;
    let last_frag = frag_para(frag, n - 1)?;
    let target = para_ref(body, p)?.clone();
    let (left, right) = split_runs(&target.runs, o)?;
    if n == 1 {
        let add = text_len(&first_frag.runs);
        let mut runs = left;
        runs.extend(first_frag.runs.iter().cloned());
        runs.extend(right);
        let mut np = target;
        np.runs = runs;
        body[p] = BodyElement::Paragraph(np);
        return Ok((p, o + add));
    }
    let mut first = target.clone();
    let mut fr = left;
    fr.extend(first_frag.runs.iter().cloned());
    first.runs = fr;
    let last_len = text_len(&last_frag.runs);
    let mut last = target;
    let mut lr: Vec<Run> = last_frag.runs.iter().cloned().collect();
    lr.extend(right);
    last.runs = lr;
    last.page_break_before = false;
    last.bookmarks.clear();
    let mut inserts: Vec<BodyElement> = Vec::new();
    for fe in &frag[1..n - 1] {
        inserts.push(fe.clone());
    }
    inserts.push(BodyElement::Paragraph(last));
    body[p] = BodyElement::Paragraph(first);
    for (k, e) in inserts.into_iter().enumerate() {
        body.insert(p + 1 + k, e);
    }
    Ok((p + n - 1, last_len))
}

/// v309: 修订模式下粘贴，片段内所有 run 标成同一条“插入”修订（拒绝时整段一起消失）。
/// 片段原有的修订标记被覆盖；表格、图片等非段落元素这一轮不标。
fn mark_frag_inserted(frag: &[BodyElement], id: u32, author: &str) -> Vec<BodyElement> {
    let date = crate::revisions::now_iso_utc();
    let mut out = frag.to_vec();
    for el in out.iter_mut() {
        if let BodyElement::Paragraph(p) = el {
            for r in p.runs.iter_mut() {
                r.revision = Some(Revision {
                    kind: RevisionKind::Insert,
                    id,
                    author: author.to_string(),
                    date: date.clone(),
                    under: None,
                });
            }
        }
    }
    out
}

/// 剪贴板是否有 Writer 文本内容。
#[no_mangle]
pub extern "C" fn roe_writer_clip_has() -> i32 {
    WCLIP.with(|c| if c.borrow().is_some() { 1 } else { 0 })
}

/// 复制 [(pa,oa),(pb,ob)) 区间到剪贴板（起止顺序不限）。
#[no_mangle]
pub extern "C" fn roe_writer_copy(handle: u64, pa: u32, oa: u32, pb: u32, ob: u32) -> i32 {
    let (pa, oa, pb, ob) = order(pa, oa, pb, ob);
    let result = handle::with_entry(handle, |entry| -> Result<Vec<BodyElement>, String> {
        let RoeDocument::Writer(doc) = &entry.doc else { return Err("非 Writer 文档".to_string()); };
        copy_in(&doc.body, pa, oa, pb, ob)
    });
    match result {
        Some(Ok(frag)) => {
            WCLIP.with(|c| *c.borrow_mut() = Some(frag));
            RoeErrorCode::Ok as i32
        }
        Some(Err(m)) => { set_last_error(format!("roe_writer_copy: {m}")); RoeErrorCode::ScriptError as i32 }
        None => { set_last_error("roe_writer_copy: 无效句柄"); RoeErrorCode::InvalidHandle as i32 }
    }
}

/// 删除 [(pa,oa),(pb,ob)) 区间（可跨段），带一次撤销快照。
#[no_mangle]
pub extern "C" fn roe_writer_delete_range(handle: u64, pa: u32, oa: u32, pb: u32, ob: u32) -> i32 {
    let (pa, oa, pb, ob) = order(pa, oa, pb, ob);
    if pa == pb && oa == ob {
        return RoeErrorCode::Ok as i32;
    }
    let result = handle::with_entry(handle, |entry| -> Result<(), String> {
        if !matches!(&entry.doc, RoeDocument::Writer(_)) { return Err("非 Writer 文档".to_string()); }
        let prev = entry.doc.clone();
        let (track, author) = (entry.writer_options.track_changes,
            if entry.writer_options.track_author.is_empty() { "用户".to_string() } else { entry.writer_options.track_author.clone() });
        let RoeDocument::Writer(doc) = &mut entry.doc else { return Err("非 Writer 文档".to_string()); };
        if track {
            tracked_delete_range(doc, pa, oa, pb, ob, &author)?;
        } else {
            delete_in(&mut doc.body, pa, oa, pb, ob)?;
        }
        entry.push_undo_snapshot(prev, handle::EditCursorContext::at(pa, 0));
        Ok(())
    });
    let ok = matches!(&result, Some(Ok(())));
    if ok {
        handle::invalidate_writer_layout_cache(handle);
    }
    match result {
        Some(Ok(())) => RoeErrorCode::Ok as i32,
        Some(Err(m)) => { set_last_error(format!("roe_writer_delete_range: {m}")); RoeErrorCode::ScriptError as i32 }
        None => { set_last_error("roe_writer_delete_range: 无效句柄"); RoeErrorCode::InvalidHandle as i32 }
    }
}

/// 在 (para, offset) 处粘贴，返回新光标 JSON {"local_para_index":N,"char_offset":N}。
/// 失败返回 null。调用方用 roe_free_string 释放。
#[no_mangle]
pub extern "C" fn roe_writer_paste(handle: u64, para: u32, offset: u32) -> *mut c_char {
    let frag = WCLIP.with(|c| c.borrow().clone());
    let Some(frag) = frag else {
        set_last_error("roe_writer_paste: 剪贴板为空");
        return std::ptr::null_mut();
    };
    let (p, o) = (para as usize, offset as usize);
    let result = handle::with_entry(handle, |entry| -> Result<(usize, usize), String> {
        if !matches!(&entry.doc, RoeDocument::Writer(_)) { return Err("非 Writer 文档".to_string()); }
        let prev = entry.doc.clone();
        let track = entry.writer_options.track_changes;
        let author = if entry.writer_options.track_author.is_empty() { "用户".to_string() } else { entry.writer_options.track_author.clone() };
        let RoeDocument::Writer(doc) = &mut entry.doc else { return Err("非 Writer 文档".to_string()); };
        let frag = if track {
            mark_frag_inserted(&frag, crate::revisions::next_revision_id(doc), &author)
        } else {
            frag.clone()
        };
        let pos = paste_in(&mut doc.body, &frag, p, o)?;
        entry.push_undo_snapshot(prev, handle::EditCursorContext::at(p, 0));
        Ok(pos)
    });
    let ok = matches!(&result, Some(Ok(_)));
    if ok {
        handle::invalidate_writer_layout_cache(handle);
    }
    match result {
        Some(Ok((np, no))) => {
            let json = format!("{{\"local_para_index\":{},\"char_offset\":{}}}", np, no);
            match CString::new(json) {
                Ok(cs) => cs.into_raw(),
                Err(e) => { set_last_error(format!("roe_writer_paste: {e}")); std::ptr::null_mut() }
            }
        }
        Some(Err(m)) => { set_last_error(format!("roe_writer_paste: {m}")); std::ptr::null_mut() }
        None => { set_last_error("roe_writer_paste: 无效句柄"); std::ptr::null_mut() }
    }
}


// ───────── v249: 格式刷（取一个 run 的字符格式，套到区间内的文字）─────────
thread_local! {
    static WPAINT: RefCell<Option<Run>> = const { RefCell::new(None) };
}

fn run_at(runs: &[Run], offset: usize) -> Option<&Run> {
    let mut acc = 0usize;
    for r in runs {
        let len = r.text.len();
        if offset >= acc && offset < acc + len {
            return Some(r);
        }
        acc += len;
    }
    runs.last()
}

/// 取 (para, offset) 所在 run 的字符格式存入格式刷槽位。
#[no_mangle]
pub extern "C" fn roe_writer_painter_pick(handle: u64, para: u32, offset: u32) -> i32 {
    let result = handle::with_entry(handle, |entry| -> Result<Run, String> {
        let RoeDocument::Writer(doc) = &entry.doc else { return Err("非 Writer 文档".to_string()); };
        let p = para_ref(&doc.body, para as usize)?;
        let r = run_at(&p.runs, offset as usize).ok_or_else(|| "该段没有文字".to_string())?;
        let mut t = r.clone();
        t.text = String::new();
        t.hyperlink = None;
        t.footnote_ref = None;
        t.endnote_ref = None;
        t.comment_range_start.clear();
        t.comment_range_end.clear();
        t.comment_ref = None;
        t.formula = None;
        t.field = None;
        Ok(t)
    });
    match result {
        Some(Ok(t)) => {
            WPAINT.with(|c| *c.borrow_mut() = Some(t));
            RoeErrorCode::Ok as i32
        }
        Some(Err(m)) => { set_last_error(format!("roe_writer_painter_pick: {m}")); RoeErrorCode::ScriptError as i32 }
        None => { set_last_error("roe_writer_painter_pick: 无效句柄"); RoeErrorCode::InvalidHandle as i32 }
    }
}

/// 把格式刷槽位里的格式套到 para 段内 [start, end) 区间，带一次撤销快照。
#[no_mangle]
pub extern "C" fn roe_writer_painter_apply(handle: u64, para: u32, start: u32, end: u32) -> i32 {
    let tpl = WPAINT.with(|c| c.borrow().clone());
    let Some(tpl) = tpl else {
        set_last_error("roe_writer_painter_apply: 没有已取的格式");
        return RoeErrorCode::ScriptError as i32;
    };
    if start >= end {
        return RoeErrorCode::Ok as i32;
    }
    let p = para as usize;
    let (s, e) = (start as usize, end as usize);
    let result = handle::with_entry(handle, |entry| -> Result<(), String> {
        if !matches!(&entry.doc, RoeDocument::Writer(_)) { return Err("非 Writer 文档".to_string()); }
        let prev = entry.doc.clone();
        let RoeDocument::Writer(doc) = &mut entry.doc else { return Err("非 Writer 文档".to_string()); };
        let target = para_ref(&doc.body, p)?.clone();
        let (left, right) = split_runs(&target.runs, s)?;
        let (mut mid, tail) = split_runs(&right, e - s)?;
        for r in mid.iter_mut() {
            if r.formula.is_some() || r.field.is_some() { continue; }
            r.bold = tpl.bold.clone();
            r.italic = tpl.italic.clone();
            r.underline = tpl.underline.clone();
            r.underline_style = tpl.underline_style.clone();
            r.font_size = tpl.font_size.clone();
            r.font_family = tpl.font_family.clone();
            r.font_family_ascii = tpl.font_family_ascii.clone();
            r.color = tpl.color.clone();
            r.background = tpl.background.clone();
            r.strikethrough = tpl.strikethrough.clone();
            r.script = tpl.script.clone();
            r.vertical_position = tpl.vertical_position.clone();
            r.char_style = tpl.char_style.clone();
        }
        let mut runs = left;
        runs.extend(mid);
        runs.extend(tail);
        let mut np = target;
        np.runs = runs;
        doc.body[p] = BodyElement::Paragraph(np);
        entry.push_undo_snapshot(prev, handle::EditCursorContext::at(p, 0));
        Ok(())
    });
    let ok = matches!(&result, Some(Ok(())));
    if ok {
        handle::invalidate_writer_layout_cache(handle);
    }
    match result {
        Some(Ok(())) => RoeErrorCode::Ok as i32,
        Some(Err(m)) => { set_last_error(format!("roe_writer_painter_apply: {m}")); RoeErrorCode::ScriptError as i32 }
        None => { set_last_error("roe_writer_painter_apply: 无效句柄"); RoeErrorCode::InvalidHandle as i32 }
    }
}


// ───────── v266: 更改大小写 ─────────
fn convert_case(text: &str, mode: u32) -> String {
    match mode {
        1 => text.to_lowercase(),
        2 => text.to_uppercase(),
        3 => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for ch in text.chars() {
                if ch.is_alphanumeric() || ch == '\'' {
                    if start { out.extend(ch.to_uppercase()); } else { out.extend(ch.to_lowercase()); }
                    start = false;
                } else {
                    out.push(ch);
                    start = true;
                }
            }
            out
        }
        4 => text.chars().map(|c| {
            if c.is_lowercase() { c.to_uppercase().collect::<String>() }
            else if c.is_uppercase() { c.to_lowercase().collect::<String>() }
            else { c.to_string() }
        }).collect(),
        _ => {
            // 句首大写：run 开头和 . ! ? 。！？ 之后的第一个字母大写，其余小写
            let mut out = String::with_capacity(text.len());
            let mut cap = true;
            for ch in text.chars() {
                if ch.is_alphabetic() {
                    if cap { out.extend(ch.to_uppercase()); } else { out.extend(ch.to_lowercase()); }
                    cap = false;
                } else {
                    out.push(ch);
                    if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？') { cap = true; }
                }
            }
            out
        }
    }
}

/// 更改 para 段内 [start, end) 的大小写。mode: 0句首 1小写 2大写 3每词首字母 4切换。带一次撤销快照。
#[no_mangle]
pub extern "C" fn roe_writer_change_case(handle: u64, para: u32, start: u32, end: u32, mode: u32) -> i32 {
    if mode > 4 {
        set_last_error("roe_writer_change_case: mode 非法（合法值 0-4）");
        return RoeErrorCode::ScriptError as i32;
    }
    if start >= end { return RoeErrorCode::Ok as i32; }
    let p = para as usize;
    let (s, e) = (start as usize, end as usize);
    let result = handle::with_entry(handle, |entry| -> Result<(), String> {
        if !matches!(&entry.doc, RoeDocument::Writer(_)) { return Err("非 Writer 文档".to_string()); }
        let prev = entry.doc.clone();
        let RoeDocument::Writer(doc) = &mut entry.doc else { return Err("非 Writer 文档".to_string()); };
        let target = para_ref(&doc.body, p)?.clone();
        let (left, right) = split_runs(&target.runs, s)?;
        let (mut mid, tail) = split_runs(&right, e - s)?;
        for r in mid.iter_mut() {
            if r.formula.is_some() || r.field.is_some() { continue; }
            r.text = convert_case(&r.text, mode);
        }
        let mut runs = left;
        runs.extend(mid);
        runs.extend(tail);
        let mut np = target;
        np.runs = runs;
        doc.body[p] = BodyElement::Paragraph(np);
        entry.push_undo_snapshot(prev, handle::EditCursorContext::at(p, 0));
        Ok(())
    });
    let ok = matches!(&result, Some(Ok(())));
    if ok { handle::invalidate_writer_layout_cache(handle); }
    match result {
        Some(Ok(())) => RoeErrorCode::Ok as i32,
        Some(Err(m)) => { set_last_error(format!("roe_writer_change_case: {m}")); RoeErrorCode::ScriptError as i32 }
        None => { set_last_error("roe_writer_change_case: 无效句柄"); RoeErrorCode::InvalidHandle as i32 }
    }
}


// ───────── v267: 插入分页符 ─────────
/// 在 para 段内 offset 处插入分页符：offset=0 只给本段置 page_break_before；
/// 否则拆段，后半段置标志。返回 0=未拆段 1=已拆段 -1=失败。带一次撤销快照。
#[no_mangle]
pub extern "C" fn roe_writer_insert_page_break(handle: u64, para: u32, offset: u32) -> i32 {
    let p = para as usize;
    let o = offset as usize;
    let result = handle::with_entry(handle, |entry| -> Result<i32, String> {
        if !matches!(&entry.doc, RoeDocument::Writer(_)) { return Err("非 Writer 文档".to_string()); }
        let prev = entry.doc.clone();
        let RoeDocument::Writer(doc) = &mut entry.doc else { return Err("非 Writer 文档".to_string()); };
        let target = para_ref(&doc.body, p)?.clone();
        let total = text_len(&target.runs);
        if o > total { return Err(format!("offset={o} 超出段落长度 {total}")); }
        let code;
        if o == 0 {
            // 与 Word 一致: 断点前留一个空段落, 原段落移到下一页
            let mut blank = target.clone();
            blank.runs = Vec::new();
            blank.bookmarks.clear();
            blank.page_break_before = false;
            let mut np = target;
            np.page_break_before = true;
            doc.body[p] = BodyElement::Paragraph(blank);
            doc.body.insert(p + 1, BodyElement::Paragraph(np));
            code = 1;
        } else {
            let (left, right) = split_runs(&target.runs, o)?;
            let mut first = target.clone();
            first.runs = left;
            let mut second = target;
            second.runs = right;
            second.bookmarks.clear();
            second.page_break_before = true;
            doc.body[p] = BodyElement::Paragraph(first);
            doc.body.insert(p + 1, BodyElement::Paragraph(second));
            code = 1;
        }
        entry.push_undo_snapshot(prev, handle::EditCursorContext::at(p, o));
        Ok(code)
    });
    match result {
        Some(Ok(c)) => { handle::invalidate_writer_layout_cache(handle); c }
        Some(Err(m)) => { set_last_error(format!("roe_writer_insert_page_break: {m}")); -1 }
        None => { set_last_error("roe_writer_insert_page_break: 无效句柄"); -1 }
    }
}

// ───────── 系统剪贴板配合：缓冲与纯文本互转 ─────────
fn frag_to_text(frag: &[BodyElement]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for e in frag {
        if let BodyElement::Paragraph(p) = e {
            lines.push(p.runs.iter().map(|r| r.text.as_str()).collect::<String>());
        }
    }
    lines.join("\n")
}

fn text_to_frag(text: &str) -> Result<Vec<BodyElement>, String> {
    let norm = text.replace("\r\n", "\n").replace('\r', "\n");
    if norm.is_empty() {
        return Err("文本为空".to_string());
    }
    Ok(norm
        .split('\n')
        .map(|line| {
            let runs = if line.is_empty() { vec![] } else { vec![Run { text: line.to_string(), ..Default::default() }] };
            BodyElement::Paragraph(Paragraph { runs, ..Default::default() })
        })
        .collect())
}

/// 导出内部缓冲的纯文本（段落用 \n 连接，整块元素跳过）。无缓冲返回 null。用 roe_free_string 释放。
#[no_mangle]
pub extern "C" fn roe_writer_clip_text() -> *mut c_char {
    let text = WCLIP.with(|c| c.borrow().as_ref().map(|f| frag_to_text(f)));
    match text {
        Some(t) => CString::new(t.replace('\0', "")).map(|cs| cs.into_raw()).unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// 把外部纯文本（UTF-8，\n 或 \r\n 分段）放进内部缓冲，之后可直接 roe_writer_paste。
#[no_mangle]
pub extern "C" fn roe_writer_set_clip_text(text: *const c_char) -> i32 {
    if text.is_null() {
        set_last_error("roe_writer_set_clip_text: 空指针");
        return RoeErrorCode::ScriptError as i32;
    }
    let cs = unsafe { std::ffi::CStr::from_ptr(text) };
    let Ok(t) = cs.to_str() else {
        set_last_error("roe_writer_set_clip_text: 不是有效 UTF-8");
        return RoeErrorCode::ScriptError as i32;
    };
    match text_to_frag(t) {
        Ok(f) => { WCLIP.with(|c| *c.borrow_mut() = Some(f)); RoeErrorCode::Ok as i32 }
        Err(m) => { set_last_error(format!("roe_writer_set_clip_text: {m}")); RoeErrorCode::ScriptError as i32 }
    }
}

#[cfg(test)]
mod clip_block_tests {
    use super::*;
    use roe_model::common::{Color, Table};
    use roe_model::writer::Section;

    fn mk_table() -> Table {
        let black = Some(Color { r: 0, g: 0, b: 0, a: 255 });
        Table {
            float_pos: None,
            col_widths: vec![],
            style_id: None,
            first_row: false, first_col: false, last_row: false, last_col: false,
            band_row: false, band_col: false,
            default_border_color: black,
            default_border_width: Some(0.5),
            inside_h_color: black,
            inside_h_width: Some(0.5),
            inside_v_color: black,
            inside_v_width: Some(0.5),
            rows: vec![],
        }
    }

    fn para(t: &str) -> BodyElement {
        BodyElement::Paragraph(Paragraph {
            runs: vec![Run { text: t.to_string(), ..Default::default() }],
            ..Default::default()
        })
    }
    fn text(e: &BodyElement) -> String {
        match e {
            BodyElement::Paragraph(p) => p.runs.iter().map(|r| r.text.as_str()).collect(),
            _ => "<obj>".to_string(),
        }
    }
    fn texts(b: &[BodyElement]) -> Vec<String> { b.iter().map(text).collect() }

    #[test]
    fn copy_across_table_keeps_block() {
        let body = vec![para("AAAA"), BodyElement::Table(mk_table()), para("BBBB")];
        let frag = copy_in(&body, 0, 2, 2, 2).unwrap();
        assert_eq!(texts(&frag), vec!["AA", "<obj>", "BB"]);
    }

    #[test]
    fn delete_across_table_removes_block() {
        let mut body = vec![para("AAAA"), BodyElement::Table(mk_table()), para("BBBB")];
        delete_in(&mut body, 0, 2, 2, 2).unwrap();
        assert_eq!(texts(&body), vec!["AABB"]);
    }

    #[test]
    fn paste_inserts_block_in_order() {
        let src = vec![para("AAAA"), BodyElement::Table(mk_table()), para("BBBB")];
        let frag = copy_in(&src, 0, 2, 2, 2).unwrap();
        let mut body = vec![para("XY")];
        let pos = paste_in(&mut body, &frag, 0, 1).unwrap();
        assert_eq!(texts(&body), vec!["XAA", "<obj>", "BBY"]);
        assert_eq!(pos, (2, 2));
    }

    #[test]
    fn plain_paragraphs_still_work() {
        let src = vec![para("hello"), para("world")];
        let frag = copy_in(&src, 0, 3, 1, 2).unwrap();
        assert_eq!(texts(&frag), vec!["lo", "wo"]);
        let mut body = vec![para("ab")];
        let pos = paste_in(&mut body, &frag, 0, 1).unwrap();
        assert_eq!(texts(&body), vec!["alo", "wob"]);
        assert_eq!(pos, (1, 2));
    }

    #[test]
    fn section_and_non_paragraph_ends_rejected() {
        let body = vec![para("A"), BodyElement::Section(Section::default()), para("B")];
        assert!(copy_in(&body, 0, 0, 2, 1).is_err());
        let body2 = vec![para("A"), BodyElement::Table(mk_table()), para("B")];
        assert!(copy_in(&body2, 1, 0, 2, 1).is_err());
        assert!(copy_in(&body2, 0, 0, 1, 0).is_err());
    }
}

#[cfg(test)]
mod clip_text_tests {
    use super::*;
    use roe_model::writer::Section;

    fn para(t: &str) -> BodyElement {
        BodyElement::Paragraph(Paragraph { runs: vec![Run { text: t.to_string(), ..Default::default() }], ..Default::default() })
    }

    #[test]
    fn frag_text_skips_blocks() {
        let f = vec![para("AA"), BodyElement::Section(Section::default()), para("BB")];
        assert_eq!(frag_to_text(&f), "AA\nBB");
    }

    #[test]
    fn text_splits_into_paragraphs() {
        let f = text_to_frag("a\r\n\r\nb").unwrap();
        assert_eq!(frag_to_text(&f), "a\n\nb");
        assert_eq!(f.len(), 3);
        assert!(text_to_frag("").is_err());
    }

    #[test]
    fn external_text_pastes_and_roundtrips() {
        let frag = text_to_frag("X\nY").unwrap();
        let mut body = vec![para("abcd")];
        let pos = paste_in(&mut body, &frag, 0, 2).unwrap();
        assert_eq!(frag_to_text(&body), "abX\nYcd");
        assert_eq!(pos, (1, 1));
    }
}

#[cfg(test)]
mod track_range_tests {
    use super::*;
    use roe_model::writer::{RevisionKind, WriterDocument};

    fn doc_of(paras: &[&str]) -> WriterDocument {
        let mut d = WriterDocument::default();
        for t in paras {
            let mut p = Paragraph::default();
            p.runs.push(Run { text: t.to_string(), ..Default::default() });
            d.body.push(BodyElement::Paragraph(p));
        }
        d
    }

    fn texts(d: &WriterDocument, i: usize) -> Vec<(String, Option<RevisionKind>)> {
        let BodyElement::Paragraph(p) = &d.body[i] else { panic!() };
        p.runs.iter().map(|r| (r.text.clone(), r.revision.as_ref().map(|v| v.kind))).collect()
    }

    #[test]
    fn same_paragraph_marks_delete() {
        let mut d = doc_of(&["ABCDEF"]);
        tracked_delete_range(&mut d, 0, 1, 0, 3, "me").unwrap();
        let t = texts(&d, 0);
        assert_eq!(t.len(), 3);
        assert_eq!(t[1], ("BC".to_string(), Some(RevisionKind::Delete)));
        assert_eq!(d.body.len(), 1);
    }

    #[test]
    fn cross_paragraph_marks_and_merges() {
        let mut d = doc_of(&["AAAA", "BBBB", "CCCC"]);
        tracked_delete_range(&mut d, 0, 2, 2, 1, "me").unwrap();
        assert_eq!(d.body.len(), 1);
        let t = texts(&d, 0);
        let plain: String = t.iter().filter(|x| x.1.is_none()).map(|x| x.0.as_str()).collect();
        let del: String = t.iter().filter(|x| x.1 == Some(RevisionKind::Delete)).map(|x| x.0.as_str()).collect();
        assert_eq!(plain, "AACCC");
        assert_eq!(del, "AABBBBC");
    }
}

#[cfg(test)]
mod track_paste_tests {
    use super::*;

    fn para(t: &str) -> BodyElement {
        BodyElement::Paragraph(Paragraph { runs: vec![Run { text: t.to_string(), ..Default::default() }], ..Default::default() })
    }

    /// 取某段所有非空 run：(文本, 修订类型, 修订id, 作者)
    fn runs_of(body: &[BodyElement], i: usize) -> Vec<(String, Option<RevisionKind>, u32, String)> {
        let BodyElement::Paragraph(p) = &body[i] else { panic!("not paragraph") };
        p.runs.iter().filter(|r| !r.text.is_empty()).map(|r| {
            let rv = r.revision.as_ref();
            (r.text.clone(), rv.map(|v| v.kind), rv.map(|v| v.id).unwrap_or(0), rv.map(|v| v.author.clone()).unwrap_or_default())
        }).collect()
    }

    #[test]
    fn untracked_paste_has_no_revision() {
        let frag = text_to_frag("X\nY").unwrap();
        let mut body = vec![para("abcd")];
        paste_in(&mut body, &frag, 0, 2).unwrap();
        for i in 0..2 {
            for (_, k, _, _) in runs_of(&body, i) { assert!(k.is_none()); }
        }
    }

    #[test]
    fn tracked_paste_marks_all_pasted_runs_as_one_insert() {
        let frag = text_to_frag("X\nY").unwrap();
        let marked = mark_frag_inserted(&frag, 7, "用户");
        let mut body = vec![para("abcd")];
        paste_in(&mut body, &marked, 0, 2).unwrap();
        let p0 = runs_of(&body, 0);
        let p1 = runs_of(&body, 1);
        assert_eq!(p0[0], ("ab".to_string(), None, 0, String::new()));
        assert_eq!(p0[1], ("X".to_string(), Some(RevisionKind::Insert), 7, "用户".to_string()));
        assert_eq!(p1[0], ("Y".to_string(), Some(RevisionKind::Insert), 7, "用户".to_string()));
        assert_eq!(p1[1], ("cd".to_string(), None, 0, String::new()));
    }

    #[test]
    fn marking_overrides_existing_revision() {
        let mut frag = vec![para("Z")];
        if let BodyElement::Paragraph(p) = &mut frag[0] {
            p.runs[0].revision = Some(Revision {
                kind: RevisionKind::Delete, id: 3, author: "别人".to_string(),
                date: String::new(), under: None,
            });
        }
        let marked = mark_frag_inserted(&frag, 9, "AI");
        let r = runs_of(&marked, 0);
        assert_eq!(r[0], ("Z".to_string(), Some(RevisionKind::Insert), 9, "AI".to_string()));
    }
}

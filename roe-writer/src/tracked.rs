//! 修订模式下的文档编辑（只依赖 WriterDocument，脚本入口使用）。
//! 注意：roe-api 里有另一份前台用的实现，两处逻辑一致，改一处请同步另一处。

use crate::find_replace::ReplaceTextResult;
use regex::Regex;
use roe_model::writer::{BodyElement, Paragraph, Revision, RevisionKind, Run, WriterDocument};

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

pub fn now_iso_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso_from_unix(secs)
}

/// 正文里现有最大修订 id 加 1（含 under 嵌套层）；没有修订返回 1。
pub fn next_revision_id(doc: &WriterDocument) -> u32 {
    fn max_in(rev: &Revision) -> u32 {
        let mut m = rev.id;
        if let Some(u) = &rev.under {
            m = m.max(max_in(u));
        }
        m
    }
    let mut max_id = 0u32;
    for el in &doc.body {
        if let BodyElement::Paragraph(p) = el {
            for r in &p.runs {
                if let Some(rev) = &r.revision {
                    max_id = max_id.max(max_in(rev));
                }
            }
        }
    }
    max_id + 1
}

fn new_rev(author: &str, id: u32, date: &str) -> Revision {
    Revision { kind: RevisionKind::Insert, id, author: author.to_string(), date: date.to_string(), under: None }
}

fn text_len(runs: &[Run]) -> usize {
    runs.iter().map(|r| r.text.len()).sum()
}

/// 保证 off 处有 run 边界（必要时把 run 一分为二）。
pub fn split_runs_at(runs: &mut Vec<Run>, off: usize) -> Result<(), String> {
    let mut consumed = 0usize;
    for i in 0..runs.len() {
        let len = runs[i].text.len();
        if off > consumed && off < consumed + len {
            let k = off - consumed;
            if !runs[i].text.is_char_boundary(k) {
                return Err(format!("偏移 {off} 不在字符边界上"));
            }
            if runs[i].formula.is_some() || runs[i].field.is_some() {
                return Err("修订模式：不能拆分公式或域".to_string());
            }
            let mut right = runs[i].clone();
            right.text = right.text[k..].to_string();
            right.comment_range_start.clear();
            right.comment_ref = None;
            right.footnote_ref = None;
            right.endnote_ref = None;
            runs[i].text.truncate(k);
            runs[i].comment_range_end.clear();
            runs.insert(i + 1, right);
            return Ok(());
        }
        consumed += len;
    }
    Ok(())
}

/// 修订模式下删除段内字节区间 [start,end)。返回被真删（作者自己的插入）的字节数。
pub fn tracked_delete_span(
    para: &mut Paragraph,
    start: usize,
    end: usize,
    author: &str,
    next_id: &mut u32,
    date: &str,
) -> Result<usize, String> {
    if start >= end {
        return Ok(0);
    }
    split_runs_at(&mut para.runs, start)?;
    split_runs_at(&mut para.runs, end)?;
    let old = std::mem::take(&mut para.runs);
    let mut out = Vec::with_capacity(old.len());
    let mut consumed = 0usize;
    let mut removed = 0usize;
    for mut r in old {
        let len = r.text.len();
        let rs = consumed;
        consumed += len;
        if len == 0 || rs < start || rs + len > end {
            out.push(r);
            continue;
        }
        match r.revision.take() {
            None => {
                r.revision = Some(Revision { kind: RevisionKind::Delete, id: *next_id, author: author.to_string(), date: date.to_string(), under: None });
                *next_id += 1;
                out.push(r);
            }
            Some(rev) if rev.kind == RevisionKind::Delete => {
                r.revision = Some(rev);
                out.push(r);
            }
            Some(rev) if rev.author == author && rev.under.is_none() => {
                removed += len;
                let had_ref = r.comment_ref.is_some() || r.footnote_ref.is_some() || r.endnote_ref.is_some()
                    || !r.comment_range_start.is_empty() || !r.comment_range_end.is_empty();
                if had_ref {
                    r.text.clear();
                    out.push(r);
                }
            }
            Some(rev) => {
                r.revision = Some(Revision { kind: RevisionKind::Delete, id: *next_id, author: author.to_string(), date: date.to_string(), under: Some(Box::new(rev)) });
                *next_id += 1;
                out.push(r);
            }
        }
    }
    para.runs = out;
    Ok(removed)
}

/// 在字节偏移 off 处插入一个 run（off 处必须能拆开）。
fn insert_run_at(para: &mut Paragraph, off: usize, run: Run) -> Result<(), String> {
    split_runs_at(&mut para.runs, off)?;
    let mut consumed = 0usize;
    for i in 0..para.runs.len() {
        if consumed >= off {
            para.runs.insert(i, run);
            return Ok(());
        }
        consumed += para.runs[i].text.len();
    }
    para.runs.push(run);
    Ok(())
}

/// 在段内字节偏移 offset 处纯插入文字。rev 为 Some 时记成该修订（同作者相邻插入并入同一 run），
/// 为 None 时是普通文字（不并入相邻的修订 run）。
pub fn insert_text_at(para: &mut Paragraph, offset: usize, text: &str, rev: Option<Revision>) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    if offset > text_len(&para.runs) {
        return Err(format!("offset {offset} 超出段落长度"));
    }
    split_runs_at(&mut para.runs, offset)?;
    let mut idx = para.runs.len();
    let mut consumed = 0usize;
    for (i, r) in para.runs.iter().enumerate() {
        if consumed >= offset {
            idx = i;
            break;
        }
        consumed += r.text.len();
    }
    if idx > 0 {
        let prev = &mut para.runs[idx - 1];
        let clean = prev.formula.is_none() && prev.field.is_none() && prev.comment_ref.is_none()
            && prev.footnote_ref.is_none() && prev.endnote_ref.is_none() && prev.comment_range_end.is_empty();
        let can_merge = clean && match (&rev, &prev.revision) {
            (None, None) => true,
            (Some(n), Some(p)) => p.kind == RevisionKind::Insert && p.author == n.author && p.under.is_none(),
            _ => false,
        };
        if can_merge {
            prev.text.push_str(text);
            return Ok(());
        }
    }
    let mut run = if idx > 0 { para.runs[idx - 1].clone() } else { para.runs.get(idx).cloned().unwrap_or_default() };
    run.text = text.to_string();
    run.revision = rev;
    run.comment_range_start.clear();
    run.comment_range_end.clear();
    run.comment_ref = None;
    run.footnote_ref = None;
    run.endnote_ref = None;
    run.formula = None;
    run.field = None;
    para.runs.insert(idx, run);
    Ok(())
}

/// 设置整段文字：旧文字标删除，新文字记为插入（接在末尾）。
pub fn tracked_set_paragraph_text(
    para: &mut Paragraph,
    text: &str,
    author: &str,
    next_id: &mut u32,
    date: &str,
) -> Result<(), String> {
    let total = text_len(&para.runs);
    // 差异修订：段内没有已删除文字、没有公式/域时，只对不同的中间部分记修订。
    let simple = para.runs.iter().all(|r| {
        r.formula.is_none() && r.field.is_none()
            && !matches!(&r.revision, Some(rv) if rv.kind == RevisionKind::Delete)
    });
    if simple && total > 0 && !text.is_empty() {
        let old: String = para.runs.iter().map(|r| r.text.as_str()).collect();
        let oc: Vec<(usize, char)> = old.char_indices().collect();
        let nc: Vec<(usize, char)> = text.char_indices().collect();
        let mut pre = 0usize;
        while pre < oc.len() && pre < nc.len() && oc[pre].1 == nc[pre].1 {
            pre += 1;
        }
        let mut suf = 0usize;
        while suf < oc.len() - pre && suf < nc.len() - pre
            && oc[oc.len() - 1 - suf].1 == nc[nc.len() - 1 - suf].1
        {
            suf += 1;
        }
        if pre + suf > 0 {
            let ob = |i: usize| if i >= oc.len() { old.len() } else { oc[i].0 };
            let nb = |i: usize| if i >= nc.len() { text.len() } else { nc[i].0 };
            let (del_s, del_e) = (ob(pre), ob(oc.len() - suf));
            let ins = text[nb(pre)..nb(nc.len() - suf)].to_string();
            if del_s == del_e && ins.is_empty() {
                return Ok(()); // 内容没变
            }
            let backup = para.runs.clone();
            let r: Result<(), String> = (|| {
                tracked_delete_span(para, del_s, del_e, author, next_id, date)?;
                if !ins.is_empty() {
                    let mut tpl_idx = 0usize;
                    let mut c = 0usize;
                    for (i, r) in para.runs.iter().enumerate() {
                        let l = r.text.len();
                        if del_s >= c && del_s < c + l.max(1) { tpl_idx = i; break; }
                        c += l;
                        tpl_idx = i;
                    }
                    let mut run = para.runs.get(tpl_idx).cloned().unwrap_or_default();
                    run.text = ins;
                    run.revision = Some(new_rev(author, *next_id, date));
                    *next_id += 1;
                    run.comment_range_start.clear();
                    run.comment_range_end.clear();
                    run.comment_ref = None;
                    run.footnote_ref = None;
                    run.endnote_ref = None;
                    // 插在被删区间之后（自己的插入被真删时 removed 为 0 以外，此处 simple 保证无此情形的错位）
                    insert_run_at(para, del_e, run)?;
                }
                Ok(())
            })();
            if r.is_ok() {
                return Ok(());
            }
            para.runs = backup;
        }
    }
    tracked_delete_span(para, 0, total, author, next_id, date)?;
    if !text.is_empty() {
        let rev = new_rev(author, *next_id, date);
        *next_id += 1;
        para.runs.push(Run { text: text.to_string(), revision: Some(rev), ..Default::default() });
    }
    Ok(())
}

/// 新插入的整段：段内所有有文字的 run 都记为插入。
pub fn mark_new_paragraph(para: &mut Paragraph, author: &str, next_id: &mut u32, date: &str) {
    for r in para.runs.iter_mut() {
        if !r.text.is_empty() {
            r.revision = Some(new_rev(author, *next_id, date));
            *next_id += 1;
        }
    }
}

/// 段内正则替换（修订版）：每处匹配旧文字标删除，新文字记为插入；已被标删除的文字不参与匹配。
/// 返回替换次数。
pub fn tracked_replace_in_paragraph(
    para: &mut Paragraph,
    regex: &Regex,
    replacement: &str,
    author: &str,
    next_id: &mut u32,
    date: &str,
) -> usize {
    let full: String = para.runs.iter().map(|r| r.text.as_str()).collect();
    let mut del: Vec<(usize, usize)> = Vec::new();
    let mut c = 0usize;
    for r in &para.runs {
        let l = r.text.len();
        if matches!(&r.revision, Some(rv) if rv.kind == RevisionKind::Delete) {
            del.push((c, c + l));
        }
        c += l;
    }
    let ms: Vec<(usize, usize)> = regex
        .find_iter(&full)
        .map(|m| (m.start(), m.end()))
        .filter(|(s, e)| s < e && !del.iter().any(|(ds, de)| *s < *de && *e > *ds))
        .collect();
    let mut count = 0usize;
    for (s, e) in ms.into_iter().rev() {
        // 格式模板：匹配起点所在的 run
        let mut template: Option<Run> = None;
        let mut cc = 0usize;
        for r in &para.runs {
            let l = r.text.len();
            if s >= cc && s < cc + l {
                template = Some(r.clone());
                break;
            }
            cc += l;
        }
        let Some(mut tpl) = template else { continue };
        let removed = match tracked_delete_span(para, s, e, author, next_id, date) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !replacement.is_empty() {
            tpl.text = replacement.to_string();
            tpl.revision = Some(new_rev(author, *next_id, date));
            *next_id += 1;
            tpl.comment_range_start.clear();
            tpl.comment_range_end.clear();
            tpl.comment_ref = None;
            tpl.footnote_ref = None;
            tpl.endnote_ref = None;
            tpl.formula = None;
            tpl.field = None;
            let _ = insert_run_at(para, e - removed, tpl);
        }
        count += 1;
    }
    count
}

/// 整个文档（或段落范围）的修订版替换，返回值与 find_replace::replace_text_in_document 同型。
pub fn tracked_replace_in_document(
    doc: &mut WriterDocument,
    regex: &Regex,
    replacement: &str,
    para_range: Option<(usize, usize)>,
    author: &str,
) -> ReplaceTextResult {
    let (rs, re) = para_range.unwrap_or((0, doc.body.len()));
    let date = now_iso_utc();
    let mut result = ReplaceTextResult::default();
    for idx in rs..re.min(doc.body.len()) {
        let mut id = next_revision_id(doc);
        let BodyElement::Paragraph(para) = &mut doc.body[idx] else { continue };
        let n = tracked_replace_in_paragraph(para, regex, replacement, author, &mut id, &date);
        if n > 0 {
            result.total_count += n;
            result.affected_para_indices.push(idx);
        }
    }
    result
}

/// 修订模式下的范围删除：区间内文字标删除；跨段时各段并成一段（合并本身不记修订，与前台一致）。
pub fn tracked_delete_range(
    doc: &mut WriterDocument,
    pa: usize,
    oa: usize,
    pb: usize,
    ob: usize,
    author: &str,
) -> Result<(), String> {
    if pa > pb || pb >= doc.body.len() {
        return Err(format!("段落范围 {pa}..{pb} 越界（body 共 {} 个元素）", doc.body.len()));
    }
    for i in pa..=pb {
        if !matches!(doc.body[i], BodyElement::Paragraph(_)) {
            return Err("修订模式：选区内含表格或图片，暂不支持删除".to_string());
        }
    }
    let date = now_iso_utc();
    for i in pa..=pb {
        let mut id = next_revision_id(doc);
        let BodyElement::Paragraph(p) = &mut doc.body[i] else { unreachable!() };
        let start = if i == pa { oa } else { 0 };
        let end = if i == pb { ob } else { text_len(&p.runs) };
        tracked_delete_span(p, start, end, author, &mut id, &date)?;
    }
    if pb > pa {
        let mut tail: Vec<Run> = Vec::new();
        for i in (pa + 1)..=pb {
            if let BodyElement::Paragraph(p) = &doc.body[i] {
                tail.extend(p.runs.clone());
            }
        }
        let BodyElement::Paragraph(first) = &mut doc.body[pa] else { unreachable!() };
        first.runs.extend(tail);
        doc.body.drain((pa + 1)..=pb);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "2026-11-07T00:00:00Z";

    fn run(t: &str) -> Run {
        Run { text: t.into(), ..Default::default() }
    }
    fn rev_run(t: &str, kind: RevisionKind, author: &str) -> Run {
        Run { text: t.into(), revision: Some(Revision { kind, id: 1, author: author.into(), date: String::new(), under: None }), ..Default::default() }
    }
    fn para(runs: Vec<Run>) -> Paragraph {
        Paragraph { runs, ..Default::default() }
    }
    fn kinds(p: &Paragraph) -> Vec<Option<RevisionKind>> {
        p.runs.iter().map(|r| r.revision.as_ref().map(|v| v.kind.clone())).collect()
    }

    #[test]
    fn replace_marks_delete_then_insert() {
        let mut p = para(vec![run("Hello World")]);
        let re = Regex::new("World").unwrap();
        let mut id = 1;
        let n = tracked_replace_in_paragraph(&mut p, &re, "Rust", "AI", &mut id, D);
        assert_eq!(n, 1);
        let texts: Vec<&str> = p.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, vec!["Hello ", "World", "Rust"]);
        assert_eq!(kinds(&p), vec![None, Some(RevisionKind::Delete), Some(RevisionKind::Insert)]);
        assert_eq!(p.runs[2].revision.as_ref().unwrap().author, "AI");
    }

    #[test]
    fn replace_skips_already_deleted_text() {
        let mut p = para(vec![rev_run("foo", RevisionKind::Delete, "Bob"), run("foo")]);
        let re = Regex::new("foo").unwrap();
        let mut id = 5;
        let n = tracked_replace_in_paragraph(&mut p, &re, "X", "AI", &mut id, D);
        assert_eq!(n, 1);
    }

    #[test]
    fn set_paragraph_text_deletes_old_inserts_new() {
        let mut p = para(vec![run("abc")]);
        let mut id = 1;
        tracked_set_paragraph_text(&mut p, "xyz", "AI", &mut id, D).unwrap();
        assert_eq!(p.runs.len(), 2);
        assert_eq!(kinds(&p), vec![Some(RevisionKind::Delete), Some(RevisionKind::Insert)]);
        assert_eq!(p.runs[1].text, "xyz");
    }

    #[test]
    fn set_paragraph_text_diff_only_marks_appended() {
        let mut p = para(vec![run("AB CD")]);
        let mut id = 1;
        tracked_set_paragraph_text(&mut p, "AB CD EF", "AI", &mut id, D).unwrap();
        let texts: Vec<&str> = p.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, vec!["AB CD", " EF"]);
        assert_eq!(kinds(&p), vec![None, Some(RevisionKind::Insert)]);
    }

    #[test]
    fn set_paragraph_text_diff_middle_change() {
        let mut p = para(vec![run("Hello World!")]);
        let mut id = 1;
        tracked_set_paragraph_text(&mut p, "Hello Rust!", "AI", &mut id, D).unwrap();
        let texts: Vec<&str> = p.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, vec!["Hello ", "World", "Rust", "!"]);
        assert_eq!(kinds(&p), vec![None, Some(RevisionKind::Delete), Some(RevisionKind::Insert), None]);
    }

    #[test]
    fn insert_text_at_tracked_splits_and_merges() {
        let mut p = para(vec![run("Hello")]);
        insert_text_at(&mut p, 2, "X", Some(new_rev("AI", 1, D))).unwrap();
        let texts: Vec<&str> = p.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, vec!["He", "X", "llo"]);
        assert_eq!(kinds(&p), vec![None, Some(RevisionKind::Insert), None]);
        insert_text_at(&mut p, 3, "Y", Some(new_rev("AI", 2, D))).unwrap();
        assert_eq!(p.runs.len(), 3);
        assert_eq!(p.runs[1].text, "XY");
    }

    #[test]
    fn insert_text_at_plain_does_not_join_revision() {
        let mut p = para(vec![rev_run("ab", RevisionKind::Insert, "u")]);
        insert_text_at(&mut p, 2, "Z", None).unwrap();
        assert_eq!(p.runs.len(), 2);
        assert_eq!(p.runs[1].text, "Z");
        assert!(p.runs[1].revision.is_none());
    }

    #[test]
    fn delete_own_insert_really_removes() {
        let mut p = para(vec![rev_run("abc", RevisionKind::Insert, "AI")]);
        let mut id = 2;
        let removed = tracked_delete_span(&mut p, 0, 3, "AI", &mut id, D).unwrap();
        assert_eq!(removed, 3);
        assert!(p.runs.is_empty());
    }
}

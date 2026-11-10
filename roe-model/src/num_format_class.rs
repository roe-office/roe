//! Excel 数字格式串的类型分类(日期/时间/日期时间/经过时长/其它)。
//!
//! 只做分类,不做渲染。规则参照 LOK SvNumFormatType 的思路:扫描格式串时见到
//! 哪类 token 决定类型;只看第一段(正数段)。独立实现,不依赖 roe-core,
//! 供 roe-polars-bridge 把日期列转成 Polars Date/Datetime。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumFormatClass {
    /// 只含日期成分(y/d/月份 m)
    Date,
    /// 只含时间成分(h/s/分钟 m)
    Time,
    /// 日期 + 时间
    DateTime,
    /// 经过时长 [h] [m] [s]
    Duration,
    /// 其它(数值、文本、General 等)
    Other,
}

pub fn classify_num_format(fmt: &str) -> NumFormatClass {
    let chars: Vec<char> = fmt.chars().collect();
    // 收集 y/d/h/s/m 的序列(连续重复合并),用于判断 m 是月份还是分钟
    let mut seq: Vec<char> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            // 只看第一段(正数段)
            ';' => break,
            // 引号字面量
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                i += 1;
            }
            // 反斜杠转义、_x 留白、*x 填充:都跳过下一个字符
            '\\' | '_' | '*' => {
                i += 2;
            }
            // 方括号:[h]/[mm]/[s] 为经过时长;颜色、条件、[$-409] 等忽略
            '[' => {
                let mut inner = String::new();
                i += 1;
                while i < chars.len() && chars[i] != ']' {
                    inner.push(chars[i]);
                    i += 1;
                }
                i += 1;
                let l = inner.to_ascii_lowercase();
                if let Some(first) = l.chars().next() {
                    if matches!(first, 'h' | 'm' | 's') && l.chars().all(|x| x == first) {
                        return NumFormatClass::Duration;
                    }
                }
            }
            // AM/PM、A/P 必须先剔除,否则其中的 m 会被当成月份
            'a' | 'A' => {
                let rest: String = chars[i..].iter().take(5).collect::<String>().to_ascii_uppercase();
                if rest.starts_with("AM/PM") {
                    i += 5;
                } else if rest.starts_with("A/P") {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            'y' | 'Y' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' | 'm' | 'M' => {
                let l = c.to_ascii_lowercase();
                if seq.last() != Some(&l) {
                    seq.push(l);
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    let mut has_date = false;
    let mut has_time = false;
    for (idx, &l) in seq.iter().enumerate() {
        match l {
            'y' | 'd' => has_date = true,
            'h' | 's' => has_time = true,
            'm' => {
                // 紧跟 h 之后或紧接 s 之前的 m 是分钟,其余是月份
                let after_h = idx > 0 && seq[idx - 1] == 'h';
                let before_s = seq.get(idx + 1) == Some(&'s');
                if after_h || before_s {
                    has_time = true;
                } else {
                    has_date = true;
                }
            }
            _ => {}
        }
    }
    match (has_date, has_time) {
        (true, true) => NumFormatClass::DateTime,
        (true, false) => NumFormatClass::Date,
        (false, true) => NumFormatClass::Time,
        (false, false) => NumFormatClass::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use NumFormatClass::*;

    #[test]
    fn test_classify_date() {
        assert_eq!(classify_num_format("m/d/yyyy"), Date);
        assert_eq!(classify_num_format("yyyy-mm-dd"), Date);
        assert_eq!(classify_num_format("d-mmm-yy"), Date);
        assert_eq!(classify_num_format("mmm-yy"), Date);
        assert_eq!(classify_num_format("[$-409]yyyy"), Date);
        assert_eq!(classify_num_format(r#"yyyy"年"m"月"d"日""#), Date);
        assert_eq!(classify_num_format(r#"m"月"d"日""#), Date);
    }

    #[test]
    fn test_classify_time() {
        assert_eq!(classify_num_format("h:mm"), Time);
        assert_eq!(classify_num_format("h:mm:ss"), Time);
        assert_eq!(classify_num_format("h:mm AM/PM"), Time);
        assert_eq!(classify_num_format("mm:ss"), Time);
        assert_eq!(classify_num_format("mm:ss.0"), Time);
        assert_eq!(classify_num_format(r#"h"时"mm"分""#), Time);
        assert_eq!(classify_num_format(r#"AM/PM h"时"mm"分""#), Time);
    }

    #[test]
    fn test_classify_datetime() {
        assert_eq!(classify_num_format("m/d/yyyy h:mm"), DateTime);
        assert_eq!(classify_num_format("yyyy-mm-dd hh:mm:ss"), DateTime);
        assert_eq!(classify_num_format("yyyy/m/d h:mm AM/PM"), DateTime);
    }

    #[test]
    fn test_classify_duration() {
        assert_eq!(classify_num_format("[h]:mm:ss"), Duration);
        assert_eq!(classify_num_format("[hh]:mm"), Duration);
        assert_eq!(classify_num_format("[mm]:ss"), Duration);
        assert_eq!(classify_num_format("[s]"), Duration);
    }

    #[test]
    fn test_classify_other() {
        assert_eq!(classify_num_format(""), Other);
        assert_eq!(classify_num_format("General"), Other);
        assert_eq!(classify_num_format("0.00"), Other);
        assert_eq!(classify_num_format("#,##0.00"), Other);
        assert_eq!(classify_num_format("0%"), Other);
        assert_eq!(classify_num_format("0.00E+00"), Other);
        assert_eq!(classify_num_format("@"), Other);
        assert_eq!(classify_num_format("[Red]0.00"), Other);
        assert_eq!(classify_num_format("[>100]0"), Other);
    }

    #[test]
    fn test_classify_quoted_and_escaped() {
        // 引号里的 d 不算日期成分
        assert_eq!(classify_num_format(r#""d"0"#), Other);
        assert_eq!(classify_num_format(r#"0"days""#), Other);
        // 反斜杠转义的 d 不算
        assert_eq!(classify_num_format(r"\d0"), Other);
        // 只看第一段:第二段有日期字符也不影响
        assert_eq!(classify_num_format("0.00;[Red]yyyy"), Other);
    }
}

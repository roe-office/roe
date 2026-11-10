// 升级前后渲染对比基准。只依赖公开函数 chart_model_to_svg 与 ChartModel 的 JSON 形态，
// 不触碰 charts-rs 的任何类型，所以旧基线与新基线都能原样编译、原样运行。
// 用法: cargo run -p roe-core --example golden_charts -- <输出目录>
use roe_core::chart_render::chart_model_to_svg;
use roe_model::impress::ChartModel;
use serde_json::{json, Map, Value};
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};

type Obj = Map<String, Value>;

struct Case {
    name: String,
    w: u32,
    h: u32,
    model: Value,
}

const CN: [&str; 4] = ["一月", "二月", "三月", "四月"];
const EN: [&str; 4] = ["Q1", "Q2", "Q3", "Q4"];
const A: [f64; 4] = [120.0, 200.0, 150.0, 80.0];
const B: [f64; 4] = [90.0, 130.0, 170.0, 110.0];
const C: [f64; 4] = [60.0, 80.0, 100.0, 140.0];

fn ser(name: &str, data: &[f64]) -> Value {
    json!({ "name": name, "data": data })
}
fn ser_line(name: &str, data: &[f64]) -> Value {
    json!({ "name": name, "data": data, "as_line": true })
}
fn model(kind: &str, labels: &[&str], series: Vec<Value>) -> Obj {
    let mut o = Obj::new();
    o.insert("kind".into(), json!(kind));
    o.insert("x_labels".into(), json!(labels));
    o.insert("series".into(), Value::Array(series));
    o
}
fn set(mut o: Obj, k: &str, v: Value) -> Obj {
    o.insert(k.to_string(), v);
    o
}
fn full(o: Obj) -> Obj {
    let o = set(o, "title", json!("季度销售对比"));
    let o = set(o, "x_axis_label", json!("月份"));
    let o = set(o, "y_axis_label", json!("金额(万元)"));
    let o = set(o, "show_legend", json!(true));
    set(o, "data_labels", json!({ "show_val": true, "explicit": true, "position": "Auto" }))
}
fn bare(o: Obj) -> Obj {
    set(o, "show_legend", json!(false))
}
fn add(v: &mut Vec<Case>, name: impl Into<String>, w: u32, h: u32, o: Obj) {
    v.push(Case { name: name.into(), w, h, model: Value::Object(o) });
}

fn cases() -> Vec<Case> {
    let mut v: Vec<Case> = Vec::new();
    let two = || vec![ser("销售额", &A), ser("利润", &B)];
    let two_en = || vec![ser("Sales", &A), ser("Profit", &B)];
    let three = || vec![ser("销售额", &A), ser("利润", &B), ser("成本", &C)];

    // 1. 类型 × 分组 × 完整(中文) / 精简(英文)
    for kind in ["Bar", "HorizontalBar", "Line", "Area"] {
        for grouping in ["Standard", "Stacked", "PercentStacked"] {
            let k = kind.to_lowercase();
            let g = grouping.to_lowercase();
            let o = set(full(model(kind, &CN, two())), "grouping", json!(grouping));
            add(&mut v, format!("kind_{k}_{g}_full_cn"), 600, 400, o);
            let o = set(bare(model(kind, &EN, two_en())), "grouping", json!(grouping));
            add(&mut v, format!("kind_{k}_{g}_bare_en"), 600, 400, o);
        }
    }

    // 2. 柱间距：gapWidth × overlap（Office 对齐项）
    let gaps = [0.0_f64, 50.0, 150.0, 500.0];
    let overlaps: [(&str, Option<f64>); 3] = [("none", None), ("m50", Some(-50.0)), ("p100", Some(100.0))];
    for g in gaps {
        for (olabel, ov) in overlaps {
            let mut o = set(full(model("Bar", &CN, two())), "gap_width_pct", json!(g));
            if let Some(x) = ov {
                o = set(o, "overlap_pct", json!(x));
            }
            add(&mut v, format!("gap_bar_s2_g{}_ov{}", g as i32, olabel), 600, 400, o);
        }
        let o = set(set(full(model("Bar", &CN, two())), "gap_width_pct", json!(g)), "grouping", json!("Stacked"));
        add(&mut v, format!("gap_bar_stacked_g{}", g as i32), 600, 400, o);
        let o = set(full(model("HorizontalBar", &CN, two())), "gap_width_pct", json!(g));
        add(&mut v, format!("gap_hbar_s2_g{}", g as i32), 600, 400, o);
        let o = set(set(full(model("HorizontalBar", &CN, two())), "gap_width_pct", json!(g)), "grouping", json!("Stacked"));
        add(&mut v, format!("gap_hbar_stacked_g{}", g as i32), 600, 400, o);
    }
    for (olabel, ov) in overlaps {
        let mut o = set(full(model("Bar", &CN, three())), "gap_width_pct", json!(150.0));
        if let Some(x) = ov {
            o = set(o, "overlap_pct", json!(x));
        }
        add(&mut v, format!("gap_bar_s3_g150_ov{olabel}"), 600, 400, o);
    }
    let o = set(full(model("HorizontalBar", &CN, three())), "gap_width_pct", json!(150.0));
    add(&mut v, "gap_hbar_s3_g150", 600, 400, o);
    for g in [150.0_f64, 500.0] {
        let s = vec![ser("销售额", &A), ser("利润", &B), ser_line("增长率", &C)];
        let o = set(full(model("Bar", &CN, s)), "gap_width_pct", json!(g));
        add(&mut v, format!("gap_combo_g{}", g as i32), 600, 400, o);
    }

    // 3. 趋势线（升级后预期不再显示，这里留底）
    for kind in ["Line", "Area"] {
        let s = vec![json!({"name":"销售额","data":A,"trendline":true}), ser("利润", &B)];
        add(&mut v, format!("trend_{}", kind.to_lowercase()), 600, 400, full(model(kind, &CN, s)));
    }

    // 4. 数据标签位置
    for pos in ["Auto", "Center"] {
        for grouping in ["Standard", "Stacked"] {
            let o = set(model("Bar", &CN, two()), "grouping", json!(grouping));
            let o = set(o, "data_labels", json!({ "show_val": true, "explicit": true, "position": pos }));
            let o = set(o, "show_legend", json!(true));
            add(&mut v, format!("label_bar_{}_{}", pos.to_lowercase(), grouping.to_lowercase()), 600, 400, o);
        }
    }

    // 5. 饼 / 环形：描边 × 标签(默认/隐藏/显示)
    let pl = ["华东", "华南", "华北", "西南", "其他"];
    let pd = [35.0, 25.0, 20.0, 12.0, 8.0];
    let white = json!({ "r": 255, "g": 255, "b": 255, "a": 255 });
    let label_modes: [(&str, Option<Value>); 3] = [
        ("default", None),
        ("hidden", Some(json!({ "show_val": false, "explicit": true, "position": "Auto" }))),
        ("shown", Some(json!({ "show_val": true, "explicit": true, "position": "Auto" }))),
    ];
    for kind in ["Pie", "Doughnut"] {
        for border in [false, true] {
            for (lname, lval) in label_modes.iter() {
                let mut o = set(model(kind, &pl, vec![ser("占比", &pd)]), "title", json!("区域占比"));
                o = set(o, "show_legend", json!(true));
                if border {
                    o = set(o, "slice_border_color", white.clone());
                }
                if let Some(l) = lval {
                    o = set(o, "data_labels", l.clone());
                }
                add(&mut v, format!("{}_border{}_labels_{}", kind.to_lowercase(), border as i32, lname), 500, 400, o);
            }
        }
    }
    for hole in [40.0_f64, 75.0] {
        let mut o = set(model("Doughnut", &pl, vec![ser("占比", &pd)]), "title", json!("区域占比"));
        o = set(o, "slice_border_color", white.clone());
        o = set(o, "hole_size_pct", json!(hole));
        add(&mut v, format!("doughnut_hole{}", hole as i32), 500, 400, o);
    }
    let legacy = vec![ser("华东", &[35.0]), ser("华南", &[25.0]), ser("华北", &[20.0])];
    add(&mut v, "pie_legacy_one_point_per_series", 500, 400, set(model("Pie", &["a"], legacy), "title", json!("旧语义")));
    let tiny = [90.0, 5.0, 3.0, 1.5, 0.5];
    let o = set(model("Pie", &pl, vec![ser("占比", &tiny)]), "data_labels", json!({ "show_val": true, "explicit": true, "position": "Auto" }));
    add(&mut v, "pie_tiny_slices", 500, 400, o);
    let outer = [30.0, 28.0, 22.0, 10.0, 10.0];
    let third = [20.0, 20.0, 20.0, 20.0, 20.0];
    for (n, hole, border) in [(2, 60.0_f64, true), (3, 75.0, true), (2, 60.0, false)] {
        let mut s = vec![ser("内环", &pd), ser("外环", &outer)];
        if n == 3 {
            s.push(ser("最外环", &third));
        }
        let mut o = set(model("Doughnut", &pl, s), "title", json!("多环"));
        o = set(o, "show_legend", json!(true));
        o = set(o, "hole_size_pct", json!(hole));
        if border {
            o = set(o, "slice_border_color", white.clone());
        }
        add(&mut v, format!("rings{}_hole{}_border{}", n, hole as i32, border as i32), 500, 400, o);
    }

    // 6. 中文长图例（量宽 / 换行 / 与标题关系）
    let long = || vec![ser("华东地区销售额", &A), ser("华南地区销售额", &B), ser("华北地区销售额", &C)];
    for (w, h) in [(600_u32, 400_u32), (320, 240)] {
        for kind in ["Bar", "Line"] {
            add(&mut v, format!("legend_long_{}_{}x{}", kind.to_lowercase(), w, h), w, h, full(model(kind, &CN, long())));
        }
    }
    let ll = ["华东地区销售额", "华南地区销售额", "华北地区销售额", "西南地区销售额"];
    let o = set(model("Pie", &ll, vec![ser("占比", &[35.0, 25.0, 20.0, 20.0])]), "title", json!("区域占比"));
    add(&mut v, "legend_long_pie", 500, 400, set(o, "show_legend", json!(true)));

    // 7. 散点 / 雷达
    add(&mut v, "scatter_full", 600, 400, full(model("Scatter", &["1", "2", "3", "4"], two())));
    add(&mut v, "scatter_bare", 600, 400, bare(model("Scatter", &["1", "2", "3", "4"], two_en())));
    let rl = ["攻击", "防御", "速度", "生命", "魔法"];
    let rs = || vec![ser("甲", &[80.0, 60.0, 90.0, 70.0, 50.0]), ser("乙", &[60.0, 85.0, 55.0, 90.0, 75.0])];
    add(&mut v, "radar_full", 500, 400, full(model("Radar", &rl, rs())));
    add(&mut v, "radar_bare", 500, 400, bare(model("Radar", &rl, rs())));

    // 8. 尺寸、窄带、负值、边界
    add(&mut v, "size_bar_300x200", 300, 200, full(model("Bar", &CN, two())));
    add(&mut v, "size_bar_800x500", 800, 500, full(model("Bar", &CN, two())));
    let months: Vec<String> = (1..=12).map(|i| format!("{i}月")).collect();
    let mrefs: Vec<&str> = months.iter().map(|s| s.as_str()).collect();
    let d12: Vec<f64> = (0..12).map(|i| 50.0 + (i as f64 * 37.0) % 90.0).collect();
    let s12 = || vec![ser("销售额", &d12), ser("利润", &d12), ser("成本", &d12)];
    add(&mut v, "narrow_bar_12_300x200", 300, 200, full(model("Bar", &mrefs, s12())));
    add(&mut v, "narrow_bar_12_200x150", 200, 150, full(model("Bar", &mrefs, s12())));
    add(&mut v, "narrow_hbar_12_300x200", 300, 200, full(model("HorizontalBar", &mrefs, s12())));
    let neg = [50.0, -30.0, 20.0, -10.0];
    add(&mut v, "neg_bar", 600, 400, full(model("Bar", &CN, vec![ser("盈亏", &neg)])));
    add(&mut v, "neg_hbar", 600, 400, full(model("HorizontalBar", &CN, vec![ser("盈亏", &neg)])));
    add(&mut v, "one_cat_bar", 600, 400, full(model("Bar", &["全年"], vec![ser("销售额", &[120.0])])));
    add(&mut v, "one_cat_hbar", 600, 400, full(model("HorizontalBar", &["全年"], vec![ser("销售额", &[120.0])])));
    add(&mut v, "edge_empty_series", 600, 400, full(model("Bar", &CN, vec![])));
    add(&mut v, "edge_all_zero", 600, 400, full(model("Bar", &CN, vec![ser("零", &[0.0, 0.0, 0.0, 0.0])])));
    let big = [1.2e6, 3.4e6, 2.1e6, 0.8e6];
    add(&mut v, "edge_big_numbers", 600, 400, full(model("Bar", &CN, vec![ser("销售额", &big)])));
    v
}

fn main() {
    let dir = std::env::args().nth(1).expect("用法: golden_charts <输出目录>");
    std::fs::create_dir_all(&dir).unwrap();
    std::panic::set_hook(Box::new(|_| {}));
    let cases = cases();
    let mut manifest = String::from("name\tstatus\tbytes\tsize\n");
    let mut html = String::from(
        "<meta charset=utf-8><style>body{font:12px sans-serif;background:#ddd}\
         .c{display:inline-block;margin:6px;background:#fff;vertical-align:top}\
         .c img{display:block;max-width:420px}</style>",
    );
    let (mut ok, mut none, mut panic, mut bad, mut nondet) = (0, 0, 0, 0, 0);
    for c in &cases {
        let m: ChartModel = match serde_json::from_value(c.model.clone()) {
            Ok(m) => m,
            Err(e) => {
                bad += 1;
                writeln!(manifest, "{}\tBADMODEL: {e}\t0\t{}x{}", c.name, c.w, c.h).unwrap();
                continue;
            }
        };
        let render = || catch_unwind(AssertUnwindSafe(|| chart_model_to_svg(&m, c.w, c.h)));
        let (status, bytes) = match render() {
            Ok(Some(svg)) => {
                let same = matches!(render(), Ok(Some(ref s2)) if *s2 == svg);
                std::fs::write(format!("{dir}/{}.svg", c.name), &svg).unwrap();
                // PNG 走 roe 真实管线(系统字体 + Noto Sans CJK SC 回退 + 外框)
                let png_ok = catch_unwind(AssertUnwindSafe(|| {
                    roe_core::chart_render::render_chart_to_rgba(&m, c.w, c.h)
                }))
                .ok()
                .flatten()
                .and_then(|buf| image::RgbaImage::from_raw(c.w, c.h, buf))
                .map(|img| img.save(format!("{dir}/{}.png", c.name)).is_ok())
                .unwrap_or(false);
                if !png_ok {
                    eprintln!("PNG 失败: {}", c.name);
                }
                write!(html, "<div class=c><img src=\"{}.png\"><div>{}</div></div>", c.name, c.name).unwrap();
                if same { ok += 1; ("OK", svg.len()) } else { nondet += 1; ("NONDET", svg.len()) }
            }
            Ok(None) => { none += 1; ("NONE", 0) }
            Err(_) => { panic += 1; ("PANIC", 0) }
        };
        writeln!(manifest, "{}\t{}\t{}\t{}x{}", c.name, status, bytes, c.w, c.h).unwrap();
    }
    std::fs::write(format!("{dir}/manifest.tsv"), manifest).unwrap();
    std::fs::write(format!("{dir}/index.html"), html).unwrap();
    println!("cases={} ok={ok} none={none} panic={panic} badmodel={bad} nondet={nondet}", cases.len());
}

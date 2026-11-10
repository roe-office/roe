// 统计图渲染黄金基线。只依赖 stat_chart_to_svg / render_stat_chart_to_rgba 与 StatChartModel,
// 自画版与 charts-rs 版同一份源码都能编译。
// 用法: cargo run -p roe-core --example golden_stat -- <输出目录> <chartEx xml 目录>
use roe_core::stat_chart_render::{render_stat_chart_to_rgba, stat_chart_to_svg};
use roe_model::stat_chart::{StatChartKind, StatChartModel, StatData, StatSeries};
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};

struct Case {
    name: String,
    w: u32,
    h: u32,
    model: StatChartModel,
}

fn mk(
    kind: StatChartKind,
    layout: &str,
    vals: &[f64],
    cats: Vec<Vec<&str>>,
    tweak: impl FnOnce(&mut StatSeries),
) -> StatChartModel {
    let mut s = StatSeries {
        name: "s".into(),
        layout_id: layout.into(),
        format_idx: 0,
        data_id: 0,
        bin_width: None,
        bin_count: None,
        interval_closed_right: true,
        quartile_exclusive: false,
        mean_line: false,
        mean_marker: false,
        show_outliers: false,
        show_non_outliers: false,
        subtotals: vec![],
        data_labels: None,
    };
    tweak(&mut s);
    let gap = match kind {
        StatChartKind::Histogram => Some(0.0),
        StatChartKind::BoxWhisker => Some(1.0),
        StatChartKind::Waterfall => Some(0.5),
        StatChartKind::Funnel => Some(0.06),
        _ => None,
    };
    StatChartModel {
        kind,
        title_present: true,
        title: None,
        show_legend: false,
        legend_pos: None,
        show_grid: true,
        gap_width: gap,
        data: vec![StatData {
            id: 0,
            categories: cats
                .into_iter()
                .map(|l| l.into_iter().map(String::from).collect())
                .collect(),
            values: vals.iter().map(|v| Some(*v)).collect(),
            value_kind: if matches!(kind, StatChartKind::Treemap | StatChartKind::Sunburst) {
                "size".into()
            } else {
                "val".into()
            },
        }],
        series: vec![s],
    }
}

fn cases(xml_dir: &str) -> Vec<Case> {
    let mut v = Vec::new();
    let names = ["waterfall", "funnel", "histogram", "box", "sunburst", "treemap"];
    for (i, n) in names.iter().enumerate() {
        let p = format!("{xml_dir}/chartEx{}.xml", i + 1);
        match std::fs::read_to_string(&p) {
            Ok(xml) => match roe_parser::ooxml::chartex::parse_chartex_xml(&xml) {
                Some(m) => v.push(Case { name: format!("wps_{}_{}", i + 1, n), w: 800, h: 500, model: m }),
                None => eprintln!("解析失败: {p}"),
            },
            Err(e) => eprintln!("读取失败 {p}: {e}"),
        }
    }
    let wf: Vec<f64> = vec![100.0, 20.0, 50.0, -40.0, 130.0, -60.0, 70.0, 140.0];
    let wc = vec!["类别1", "类别2", "类别3", "类别4", "类别5", "类别6", "类别7", "类别8"];
    let t = |m: StatChartModel, name: &str| Case { name: name.into(), w: 640, h: 400, model: m };
    v.push(t(mk(StatChartKind::Waterfall, "waterfall", &wf, vec![wc.clone()], |s| s.subtotals = vec![4, 7]), "wf_subtotals"));
    v.push(t(mk(StatChartKind::Waterfall, "waterfall", &wf, vec![wc.clone()], |_| {}), "wf_no_subtotals"));
    v.push(t(mk(StatChartKind::Waterfall, "waterfall", &[-10.0, -20.0, 5.0, -3.0], vec![vec!["a", "b", "c", "d"]], |_| {}), "wf_negative_start"));
    v.push(t(mk(StatChartKind::Waterfall, "waterfall", &[100.0, -30.0, 0.0, 50.0, 120.0], vec![vec!["a", "b", "c", "d", "e"]], |s| s.subtotals = vec![2, 4]), "wf_zero_subtotal"));
    v.push(t(mk(StatChartKind::Waterfall, "waterfall", &[100.0, 20.0, 50.0, -40.0, 130.0, -60.0, 70.0, 140.0], vec![vec!["类别1", "类别2", "类别3", "类别4", "类别5", "类别6", "类别7", "类别8"]], |s| { s.subtotals = vec![4, 7]; s.data_labels = Some(roe_model::stat_chart::StatDataLabels { pos: "outEnd".to_string(), show_value: true, show_category: false, show_series: false }); }), "wf_ppt_sub"));
    v.push(t(mk(StatChartKind::Funnel, "funnel", &[5000.0, 4000.0, 3000.0, 1000.0, 250.0], vec![vec!["类别1", "类别2", "类别3", "类别4", "类别5"]], |_| {}), "funnel_5"));
    v.push(t(mk(StatChartKind::Funnel, "funnel", &[100.0, 100.0, 0.0, 30.0], vec![vec!["x", "y", "z", "w"]], |_| {}), "funnel_zero_equal"));
    let hv: Vec<f64> = (1..=25).map(|x| x as f64).collect();
    let hv2: Vec<f64> = vec![1.0, 2.0, 2.0, 3.0, 3.0, 3.0, 4.0, 4.0, 5.0, 8.0, 8.5, 9.0, 12.0, 12.0, 15.5];
    v.push(t(mk(StatChartKind::Histogram, "clusteredColumn", &hv, vec![], |s| { s.bin_width = Some(4.0); s.interval_closed_right = true; }), "hist_w4_right"));
    v.push(t(mk(StatChartKind::Histogram, "clusteredColumn", &hv, vec![], |s| { s.bin_width = Some(4.0); s.interval_closed_right = false; }), "hist_w4_left"));
    v.push(t(mk(StatChartKind::Histogram, "clusteredColumn", &hv, vec![], |s| { s.bin_count = Some(5); }), "hist_count5"));
    v.push(t(mk(StatChartKind::Histogram, "clusteredColumn", &hv2, vec![], |_| {}), "hist_auto_float_right"));
    v.push(t(mk(StatChartKind::Histogram, "clusteredColumn", &hv2, vec![], |s| s.interval_closed_right = false), "hist_auto_float_left"));
    v.push(t(mk(StatChartKind::Histogram, "clusteredColumn", &[5.0, 5.0, 5.0], vec![], |_| {}), "hist_single_value"));
    let bv: Vec<f64> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 50.0];
    v.push(t(mk(StatChartKind::BoxWhisker, "boxWhisker", &bv, vec![], |s| { s.mean_line = true; s.mean_marker = true; s.show_outliers = true; s.show_non_outliers = true; }), "box_outlier_mean_incl"));
    v.push(t(mk(StatChartKind::BoxWhisker, "boxWhisker", &bv, vec![], |s| { s.quartile_exclusive = true; s.show_outliers = true; }), "box_outlier_excl"));
    v.push(t(mk(StatChartKind::BoxWhisker, "boxWhisker", &bv, vec![], |s| { s.show_outliers = false; s.show_non_outliers = false; }), "box_no_points"));
    let tc = vec![vec!["a1", "a2", "b1", "b2", "b3", "c1"], vec!["A", "A", "B", "B", "B", "C"]];
    let tvals = [30.0, 20.0, 25.0, 15.0, 10.0, 40.0];
    v.push(t(mk(StatChartKind::Treemap, "treemap", &tvals, tc.clone(), |_| {}), "treemap_2level"));
    v.push(t(mk(StatChartKind::Sunburst, "sunburst", &tvals, tc, |_| {}), "sunburst_2level"));
    v.push(t(mk(StatChartKind::Treemap, "treemap", &[3.0, 2.0, 1.0], vec![vec!["x", "y", "z"]], |_| {}), "treemap_1level"));
    v
}

fn main() {
    let dir = std::env::args().nth(1).expect("用法: golden_stat <输出目录> <xml目录>");
    let xml_dir = std::env::args().nth(2).expect("缺少 xml 目录");
    std::fs::create_dir_all(&dir).unwrap();
    std::panic::set_hook(Box::new(|_| {}));
    let cs = cases(&xml_dir);
    let mut manifest = String::from("name\tstatus\tbytes\tsize\n");
    let mut html = String::from(
        "<meta charset=utf-8><style>body{font:12px sans-serif;background:#ddd}\
         .c{display:inline-block;margin:6px;background:#fff;vertical-align:top}\
         .c img{display:block;max-width:420px}</style>",
    );
    let (mut ok, mut none, mut pan, mut nondet) = (0, 0, 0, 0);
    for c in &cs {
        let render = || catch_unwind(AssertUnwindSafe(|| stat_chart_to_svg(&c.model, c.w, c.h, 1.0)));
        let (status, bytes) = match render() {
            Ok(Some(svg)) => {
                let same = matches!(render(), Ok(Some(ref s2)) if *s2 == svg);
                std::fs::write(format!("{dir}/{}.svg", c.name), &svg).unwrap();
                let png_ok = catch_unwind(AssertUnwindSafe(|| render_stat_chart_to_rgba(&c.model, c.w, c.h, 1.0)))
                    .ok()
                    .flatten()
                    .and_then(|buf| image::RgbaImage::from_raw(c.w, c.h, buf))
                    .map(|img| img.save(format!("{dir}/{}.png", c.name)).is_ok())
                    .unwrap_or(false);
                if !png_ok { eprintln!("PNG 失败: {}", c.name); }
                write!(html, "<div class=c><img src=\"{0}.png\"><div>{0}</div></div>", c.name).unwrap();
                if same { ok += 1; ("OK", svg.len()) } else { nondet += 1; ("NONDET", svg.len()) }
            }
            Ok(None) => { none += 1; ("NONE", 0) }
            Err(_) => { pan += 1; ("PANIC", 0) }
        };
        writeln!(manifest, "{}\t{}\t{}\t{}x{}", c.name, status, bytes, c.w, c.h).unwrap();
    }
    std::fs::write(format!("{dir}/manifest.tsv"), manifest).unwrap();
    std::fs::write(format!("{dir}/index.html"), html).unwrap();
    println!("cases={} ok={ok} none={none} panic={pan} nondet={nondet}", cs.len());
}

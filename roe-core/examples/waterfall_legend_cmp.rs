// 临时对比:瀑布图 × 图例位置。不入库。用法: cargo run -p roe-core --example waterfall_legend_cmp -- <输出目录>
use roe_core::stat_chart_render::stat_chart_to_svg;
use roe_model::stat_chart::{StatChartKind, StatChartModel, StatData, StatDataLabels, StatSeries};

fn model(pos: Option<&str>, title: Option<&str>) -> StatChartModel {
    let vals = [100.0, 20.0, 50.0, -40.0, 130.0, -60.0, 70.0, 140.0];
    let cats: Vec<String> = (1..=8).map(|i| format!("类别{i}")).collect();
    let s = StatSeries {
        name: "s".into(),
        layout_id: "waterfall".into(),
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
        subtotals: vec![4, 7],
        data_labels: Some(StatDataLabels {
            pos: "outEnd".to_string(),
            show_value: true,
            show_category: false,
            show_series: false,
        }),
    };
    StatChartModel {
        kind: StatChartKind::Waterfall,
        title_present: title.is_some(),
        title: title.map(String::from),
        show_legend: true,
        legend_pos: pos.map(String::from),
        show_grid: true,
        gap_width: Some(0.5),
        data: vec![StatData {
            id: 0,
            categories: vec![cats],
            values: vals.iter().map(|v| Some(*v)).collect(),
            value_kind: "val".into(),
        }],
        series: vec![s],
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("用法: waterfall_legend_cmp <输出目录>");
    std::fs::create_dir_all(&dir).unwrap();
    let mut html = String::from("<meta charset=utf-8><style>body{font:12px sans-serif;background:#ddd}.c{display:inline-block;margin:4px;background:#fff;vertical-align:top}.c img{display:block;width:420px}</style>");
    let poss: [Option<&str>; 5] = [None, Some("t"), Some("b"), Some("l"), Some("r")];
    for (grp, title) in [("title", Some("瀑布图")), ("notitle", None)] {
        html.push_str(&format!("<h3>{grp}</h3>"));
        for p in poss {
            let name = format!("wf_{}_{}", grp, p.unwrap_or("none"));
            match stat_chart_to_svg(&model(p, title), 600, 400, 1.0) {
                Some(svg) => {
                    std::fs::write(format!("{dir}/{name}.svg"), &svg).unwrap();
                    html.push_str(&format!("<div class=c><img src='{name}.svg'><div>{name}</div></div>"));
                }
                None => html.push_str(&format!("<div class=c>{name}: None</div>")),
            }
        }
    }
    std::fs::write(format!("{dir}/index.html"), html).unwrap();
    println!("done -> {dir}/index.html");
}

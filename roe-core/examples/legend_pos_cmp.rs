// 临时对比:各图型 × 图例位置。用法: cargo run -p roe-core --example legend_pos_cmp -- <输出目录>
use roe_core::chart_render::chart_model_to_svg;
use roe_model::impress::ChartModel;
use serde_json::json;

fn main() {
    let dir = std::env::args().nth(1).expect("用法: legend_pos_cmp <输出目录>");
    std::fs::create_dir_all(&dir).unwrap();
    let labels = ["一月", "二月", "三月", "四月"];
    let s = |n: &str, d: [f64; 4]| json!({"name": n, "data": d});
    let two = || vec![s("销售额", [120.0, 200.0, 150.0, 80.0]), s("利润", [90.0, 130.0, 170.0, 110.0])];
    let kinds = ["Bar", "HorizontalBar", "Line", "Area", "Pie", "Doughnut", "Radar", "Scatter"];
    let poss: [Option<&str>; 5] = [None, Some("Top"), Some("Bottom"), Some("Left"), Some("Right")];
    let mut html = String::from("<meta charset=utf-8><style>body{font:12px sans-serif;background:#ddd}.c{display:inline-block;margin:4px;background:#fff;vertical-align:top}.c img{display:block;width:300px}</style>");
    for k in kinds {
        html.push_str(&format!("<h3>{k}</h3>"));
        for p in poss {
            let mut m = json!({"kind": k, "title": "标题", "x_labels": labels, "series": two(), "show_legend": true});
            if let Some(p) = p { m["legend_pos"] = json!(p); }
            let name = format!("{}_{}", k.to_lowercase(), p.unwrap_or("none").to_lowercase());
            let model: ChartModel = serde_json::from_value(m).unwrap();
            match chart_model_to_svg(&model, 600, 400) {
                Some(svg) => {
                    std::fs::write(format!("{dir}/{name}.svg"), &svg).unwrap();
                    html.push_str(&format!("<div class=c><img src='{name}.svg'><div>{name}</div></div>"));
                }
                None => html.push_str(&format!("<div class=c>{name}: None</div>")),
            }
        }
    }
    for (nm, k) in [("nt_bar_right", "Bar"), ("nt_pie_right", "Pie")] {
        let m = json!({"kind": k, "x_labels": labels, "series": two(), "show_legend": true, "legend_pos": "Right"});
        let model: ChartModel = serde_json::from_value(m).unwrap();
        if let Some(svg) = chart_model_to_svg(&model, 600, 400) {
            std::fs::write(format!("{dir}/{nm}.svg"), &svg).unwrap();
            html.push_str(&format!("<div class=c><img src='{nm}.svg'><div>{nm}</div></div>"));
        }
    }
    std::fs::write(format!("{dir}/index.html"), html).unwrap();
    println!("done -> {dir}/index.html");
}

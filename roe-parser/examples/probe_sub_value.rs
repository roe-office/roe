use roe_model::calc::PivotHeaderNode;
fn dump(n: &PivotHeaderNode, d: usize) {
    println!("{}{:?} sub={} span={}", "  ".repeat(d), n.value, n.is_subtotal, n.cached_span);
    for c in &n.children { dump(c, d + 1); }
}
fn main() {
    let bytes = std::fs::read("test_data/pivot_subtotal.xlsx").expect("read");
    let doc = roe_parser::RoeDocument::open_xlsx_bytes(&bytes).expect("parse");
    let wb = match doc { roe_parser::RoeDocument::Calc(wb) => wb, _ => panic!("not calc") };
    for s in &wb.sheets {
        if let Some(m) = &s.pivot {
            println!("sheet={} values={:?}", s.name, m.value_fields.iter().map(|v| (&v.name, &v.agg)).collect::<Vec<_>>());
            println!("--- col_tree ---"); dump(&m.col_tree, 0);
            println!("--- row_tree ---"); dump(&m.row_tree, 0);
        }
    }
}

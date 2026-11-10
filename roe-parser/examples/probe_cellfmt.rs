fn main() {
    let bytes = std::fs::read("test_samples/xlsx/sc_data_xlsx__MissingPathExternal.xlsx").expect("read");
    let doc = roe_parser::RoeDocument::open_xlsx_bytes(&bytes).expect("parse");
    let wb = match doc { roe_parser::RoeDocument::Calc(wb) => wb, _ => panic!("not calc") };
    for s in wb.sheets.iter().take(1) {
        println!("sheet={} cell_formats 总数={}", s.name, s.cell_formats.len());
        let mut keys: Vec<(u32, u32)> = s.cell_formats.keys().copied().collect();
        keys.sort();
        for (r, c) in keys {
            if r <= 12 && c <= 10 {
                let f = &s.cell_formats[&(r, c)];
                println!("row={} col={} bg={:?} fg={:?} numfmt={:?} borders={}",
                    r, c, f.bg_color, f.fg_color, f.num_format, f.borders.is_some());
            }
        }
    }
}

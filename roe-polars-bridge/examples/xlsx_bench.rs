use calamine::{open_workbook_auto, Data, Reader};
use polars::prelude::*;
use std::io::Write;
use std::time::Instant;

fn cell_str(c: &Data) -> String {
    match c {
        Data::Int(n) => n.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", *f as i64) } else { format!("{}", f) }
        }
        Data::String(s) => s.clone(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => format!("{}", dt),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        _ => String::new(),
    }
}

fn csv_field(s: &str, out: &mut Vec<u8>) {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        out.push(b'"');
        out.extend_from_slice(s.replace('"', "\"\"").as_bytes());
        out.push(b'"');
    } else {
        out.extend_from_slice(s.as_bytes());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("用法: xlsx_bench <xlsx路径> <模式: open|range|strings|tocsv|full>");
    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("full");
    let t0 = Instant::now();

    if mode == "full" {
        let df = roe_polars_bridge::df_from_xlsx_file(path).unwrap();
        println!("mode=full rows={} cols={} elapsed={:.1?}", df.height(), df.width(), t0.elapsed());
        println!("{}", df.head(Some(3)));
        return;
    }

    if mode == "head" {
        use calamine::{open_workbook, Xlsx};
        let mut wb: Xlsx<_> = open_workbook(path).unwrap();
        println!("open   = {:.1?}", t0.elapsed());
        let name = wb.sheet_names().first().unwrap().clone();
        let t1 = Instant::now();
        let mut rd = wb.worksheet_cells_reader(&name).unwrap();
        println!("reader = {:.1?}  dims={:?}", t1.elapsed(), rd.dimensions());
        let t2 = Instant::now();
        let mut shown: Vec<Vec<String>> = Vec::new();
        let mut cur_row = u32::MAX;
        while let Some(cell) = rd.next_cell().unwrap() {
            let (r, _c) = cell.get_position();
            if r != cur_row {
                if shown.len() >= 11 { break; }
                shown.push(Vec::new());
                cur_row = r;
            }
            shown.last_mut().unwrap().push(format!("{:?}", cell.get_value()));
        }
        println!("head11 = {:.1?}  rows={}", t2.elapsed(), shown.len());
        println!("total  = {:.1?}", t0.elapsed());
        println!("{:?}", shown.get(1));
        return;
    }

    let mut wb = open_workbook_auto(path).unwrap();
    let t_open = t0.elapsed();
    println!("open   = {:.1?}", t_open);
    if mode == "open" { return; }

    let name = wb.sheet_names().first().unwrap().clone();
    let t1 = Instant::now();
    let range = wb.worksheet_range(&name).unwrap();
    println!("range  = {:.1?}  (rows={} cols={})", t1.elapsed(), range.height(), range.width());
    if mode == "range" { return; }

    if mode == "strings" {
        let t2 = Instant::now();
        let mut rows = range.rows();
        let headers: Vec<String> = rows.next().unwrap().iter().map(cell_str).collect();
        let mut cols: Vec<Vec<String>> = vec![Vec::with_capacity(range.height()); headers.len()];
        for row in rows {
            for (i, c) in row.iter().enumerate() {
                if i < cols.len() { cols[i].push(cell_str(c)); }
            }
        }
        println!("strings= {:.1?}", t2.elapsed());
        println!("total  = {:.1?}", t0.elapsed());
        return;
    }

    if mode == "tocsv" {
        let t2 = Instant::now();
        let out_path = "/tmp/xlsx_bench.csv";
        let mut w = std::io::BufWriter::new(std::fs::File::create(out_path).unwrap());
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        for row in range.rows() {
            buf.clear();
            for (i, c) in row.iter().enumerate() {
                if i > 0 { buf.push(b','); }
                csv_field(&cell_str(c), &mut buf);
            }
            buf.push(b'\n');
            w.write_all(&buf).unwrap();
        }
        w.flush().unwrap();
        drop(w);
        println!("write csv = {:.1?}", t2.elapsed());
        let t3 = Instant::now();
        let df = CsvReadOptions::default()
            .try_into_reader_with_file_path(Some(out_path.into())).unwrap().finish().unwrap();
        println!("read csv  = {:.1?}  rows={} cols={}", t3.elapsed(), df.height(), df.width());
        println!("total  = {:.1?}", t0.elapsed());
        println!("{:?}", df.schema());
        return;
    }
    panic!("未知模式 {mode}");
}

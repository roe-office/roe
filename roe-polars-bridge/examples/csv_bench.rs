use polars::prelude::*;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("用法: csv_bench <csv路径> <模式>");
    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("all");
    let t = Instant::now();
    let df = match mode {
        // 默认推断 100 行
        "infer100" => CsvReadOptions::default()
            .try_into_reader_with_file_path(Some(path.into())).unwrap().finish().unwrap(),
        // 全表推断
        "infer_all" => CsvReadOptions::default()
            .with_infer_schema_length(None)
            .try_into_reader_with_file_path(Some(path.into())).unwrap().finish().unwrap(),
        // 全 String
        "string" => CsvReadOptions::default()
            .with_infer_schema_length(Some(0))
            .try_into_reader_with_file_path(Some(path.into())).unwrap().finish().unwrap(),
        // 我们现在的写法:全表推断 + File 句柄
        "handle" => CsvReadOptions::default()
            .with_infer_schema_length(None)
            .into_reader_with_file_handle(std::fs::File::open(path).unwrap())
            .finish().unwrap(),
        // LazyCsvReader 路径读
        "lazy_all" => LazyCsvReader::new(PlRefPath::new(path))
            .with_infer_schema_length(None)
            .finish().unwrap().collect().unwrap(),
        m => panic!("未知模式 {m}"),
    };
    println!("mode={mode} rows={} cols={} elapsed={:.1?}", df.height(), df.width(), t.elapsed());
}

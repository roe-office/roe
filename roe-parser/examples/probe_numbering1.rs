use roe_parser::ooxml::docx::DocxParser;
use roe_model::writer::BodyElement;

fn main() {
    let home = std::env::var("HOME").unwrap();
    let path = format!("{}/桌面/sw_ooxmlexport_data__numbering1.docx", home);
    let data = std::fs::read(&path).expect("read docx");
    let doc = DocxParser::parse(&data).expect("parse");
    let mut n = 0;
    for el in doc.body.iter() {
        if let BodyElement::Paragraph(p) = el {
            let txt: String = p.runs.iter().map(|r| r.text.as_str()).collect();
            if txt.trim().is_empty() { continue; }
            println!("--- para {} style={:?}", n, p.style.0);
            println!("  list_info={:?}", p.list_info);
            println!("  indent_left={:?} indent_first={:?}", p.indent_left, p.indent_first);
            for (i, r) in p.runs.iter().take(2).enumerate() {
                println!("  run{} text={:?} bold={:?} color={:?} size={:?}", i, r.text.chars().take(14).collect::<String>(), r.bold, r.color, r.font_size);
            }
            n += 1;
            if n >= 8 { break; }
        }
    }
    println!("list_styles keys: {:?}", doc.styles.list_styles.keys().collect::<Vec<_>>());
}

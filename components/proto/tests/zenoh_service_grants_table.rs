//! Host regression of the Zenoh service grants table decoder.
//!
//! The tables are not built here. The slice writes them from the Python encoder to the file named by
//! `SLIME_GRANTS_TABLES`, one a line as a name and its bytes in hex, and this test decodes each with the decoder in
//! `slime-proto` and prints a verdict. `scripts/lib/zenoh_grants_table_exam.py` owns what each verdict must be.

use slime_proto::zenoh_service_grants_table::{Row, Table};

fn from_hex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0, "an odd number of hex digits");
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("hex"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest(row: &Row<'_>) -> String {
    format!(
        "{}:{}:{}{}",
        std::str::from_utf8(row.holder).expect("ASCII"),
        std::str::from_utf8(row.key).expect("ASCII"),
        if row.publish { 'p' } else { '-' },
        if row.subscribe { 's' } else { '-' }
    )
}

#[test]
fn every_table_decodes_to_its_rows_or_is_refused() {
    // `cargo test -p slime-proto` runs this with no tables file. The slice always sets the variable and requires every
    // line this test prints, so a run that skipped here cannot pass the slice.
    let Ok(path) = std::env::var("SLIME_GRANTS_TABLES") else {
        return;
    };
    let text = std::fs::read_to_string(path).expect("tables file");
    let mut tables = 0;
    for line in text.lines() {
        let (name, bytes) = line.split_once(' ').expect("name and bytes");
        let bytes = from_hex(bytes);
        let verdict = match Table::decode(&bytes) {
            Ok(table) => {
                let rows: Vec<String> = (0..table.row_count())
                    .map(|index| digest(&table.row(index).expect("a decoded table has valid rows")))
                    .collect();
                format!("rows {} {}", table.row_count(), rows.join(";"))
            }
            Err(_) => "refused".to_string(),
        };
        println!("[zenoh-exam] grantstable {name} {} {verdict}", hex(&bytes));
        tables += 1;
    }
    assert!(tables > 0, "the tables file is empty");
    println!("[zenoh-exam] grantstable ok every-table-decodes-to-its-rows-or-is-refused");
}

use std::{env, fs, path::PathBuf};

type Row = [u32; 3];

#[derive(Default)]
struct Tables {
    upper: Vec<Row>,
    lower: Vec<Row>,
    context: Vec<Row>,
    bytes: Vec<u8>,
    expansion: u32,
}

impl Tables {
    fn mapping(&mut self, c: char, mapped: String) -> Option<Row> {
        if mapped == c.to_string() {
            return None;
        }
        self.expansion = self
            .expansion
            .max(mapped.len().div_ceil(c.len_utf8()) as u32);
        Some([c as u32, c as u32, self.string(&mapped)])
    }

    fn string(&mut self, text: &str) -> u32 {
        assert!(text.len() < 16);
        let descriptor = ((self.bytes.len() as u32) << 4) | text.len() as u32;
        self.bytes.extend_from_slice(text.as_bytes());
        descriptor
    }

    fn context(&mut self, c: char, class: u32) {
        if class == 0 {
            return;
        }
        if let Some(last) = self.context.last_mut()
            && last[1] + 1 == c as u32
            && last[2] == class
        {
            last[1] = c as u32;
        } else {
            self.context.push([c as u32, c as u32, class]);
        }
    }

    fn encode(mut self) -> Vec<u8> {
        let final_sigma = self.string("ς");
        let header = [
            self.upper.len() as u32,
            self.lower.len() as u32,
            self.context.len() as u32,
            final_sigma,
            self.expansion,
        ];
        let mut encoded = Vec::new();
        for word in header.into_iter().chain(
            self.upper
                .into_iter()
                .chain(self.lower)
                .chain(self.context)
                .flatten(),
        ) {
            encoded.extend_from_slice(&word.to_le_bytes());
        }
        encoded.extend(self.bytes);
        encoded
    }
}

fn context_class(c: char, probe: &mut String) -> u32 {
    probe.clear();
    probe.push(c);
    probe.push('Σ');
    if probe.to_lowercase().ends_with('ς') {
        return 1;
    }
    probe.insert(0, 'A');
    if probe.to_lowercase().ends_with('ς') {
        2
    } else {
        0
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mut tables = Tables::default();
    let mut probe = String::with_capacity(8);
    for c in (0..=0x10ffff).filter_map(char::from_u32) {
        if let Some(row) = tables.mapping(c, c.to_uppercase().collect()) {
            tables.upper.push(row);
        }
        if let Some(row) = tables.mapping(c, c.to_lowercase().collect()) {
            tables.lower.push(row);
        }
        tables.context(c, context_class(c, &mut probe));
    }
    let path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("unicode-case.bin");
    fs::write(path, tables.encode()).unwrap();
}

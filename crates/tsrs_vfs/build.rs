#[cfg(feature = "compressed-stdlib")]
mod packed {
    use std::{env, fmt::Write, fs, path::PathBuf};

    include!("src/bundled/embed_generated.rs");

    fn literal(value: usize) -> String {
        if value < 1_000 {
            return value.to_string();
        }
        if value < 1_000_000 {
            return format!("{}_{:03}", value / 1_000, value % 1_000);
        }
        format!("{}_{:03}_{:03}", value / 1_000_000, value / 1_000 % 1_000, value % 1_000)
    }

    pub fn build() {
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let mut bytes = Vec::new();
        let mut entries = String::from("pub const FILES: &[(&str, usize, usize, usize)] = &[\n");
        for (name, contents) in EMBEDDED_CONTENTS {
            println!("cargo:rerun-if-changed={name}");
            let start = bytes.len();
            bytes.extend(miniz_oxide::deflate::compress_to_vec_zlib(contents.as_bytes(), 9));
            writeln!(
                entries,
                "({name:?}, {}, {}, {}),",
                literal(start),
                literal(bytes.len()),
                literal(contents.len())
            )
            .unwrap();
        }
        entries.push_str("];\n");
        fs::write(out.join("libraries.rs"), entries).unwrap();
        fs::write(out.join("libraries.zlib"), bytes).unwrap();
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/bundled/embed_generated.rs");
    #[cfg(feature = "compressed-stdlib")]
    packed::build();
}

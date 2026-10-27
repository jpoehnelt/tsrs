use std::sync::{LazyLock, OnceLock};

use rustc_hash::FxHashMap;

include!(concat!(env!("OUT_DIR"), "/libraries.rs"));

struct Entry {
    compressed: &'static [u8],
    bytes: usize,
    text: OnceLock<String>,
}

static INDEX: LazyLock<FxHashMap<&'static str, Entry>> = LazyLock::new(|| {
    let archive = include_bytes!(concat!(env!("OUT_DIR"), "/libraries.zlib"));
    FILES
        .iter()
        .map(|&(name, start, end, bytes)| {
            (name, Entry { compressed: &archive[start..end], bytes, text: OnceLock::new() })
        })
        .collect()
});

// One immutable owner per library keeps compiler/parser borrows valid across
// snapshots and threads. Unrequested libraries retain only their compressed bytes.
pub fn contents(path: &str) -> Option<&'static str> {
    let entry = INDEX.get(path)?;
    Some(entry.text.get_or_init(|| {
        let mut bytes = vec![0; entry.bytes];
        let length = miniz_oxide::inflate::decompress_slice_iter_to_slice(
            &mut bytes,
            std::iter::once(entry.compressed),
            true,
            false,
        )
        .expect("invalid packaged standard library");
        assert_eq!(length, entry.bytes, "packaged standard library length changed");
        String::from_utf8(bytes).expect("packaged standard libraries must be UTF-8")
    }))
}

pub fn length(path: &str) -> Option<usize> {
    INDEX.get(path).map(|entry| entry.bytes)
}

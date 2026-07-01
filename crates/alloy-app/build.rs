use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let bundled_font = Path::new(&out_dir).join("funnel_sans_font.bin");
    let source_font = Path::new("assets/fonts/FunnelSans.ttf");

    println!("cargo:rerun-if-changed={}", source_font.display());
    if source_font.is_file() {
        std::fs::copy(source_font, bundled_font).expect("copy Funnel Sans bundle font");
    } else {
        std::fs::write(bundled_font, []).expect("write empty Funnel Sans bundle font");
    }
}

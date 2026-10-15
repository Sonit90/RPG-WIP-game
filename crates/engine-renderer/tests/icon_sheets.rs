//! Dev tool: renders every icon under `assets/icons/<category>/` onto one contact sheet per
//! category, so the set can be judged side by side.
//!
//! Run with `cargo test -p engine-renderer --test icon_sheets -- --ignored`; the sheets are
//! written to `docs/images/icons/<category>.png`, icons in file-name order, left to right.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "dev tool: a failure should be loud"
)]

use std::path::Path;

use resvg::{tiny_skia, usvg};

const CELL: u32 = 132;
const GAP: u32 = 10;
const COLUMNS: u32 = 8;

fn render(path: &Path) -> tiny_skia::Pixmap {
    let data = std::fs::read(path).expect("readable icon");
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).expect("valid svg");
    let size = tree.size();
    let scale = (CELL as f32 / size.width()).min(CELL as f32 / size.height());
    let transform = tiny_skia::Transform::from_translate(
        (CELL as f32 - size.width() * scale) * 0.5,
        (CELL as f32 - size.height() * scale) * 0.5,
    )
    .pre_scale(scale, scale);
    let mut pixmap = tiny_skia::Pixmap::new(CELL, CELL).unwrap();
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap
}

#[test]
#[ignore = "writes docs/images/icons; run by hand"]
fn write_contact_sheets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let icons = root.join("assets/icons");
    let out = root.join("../../docs/images/icons");
    std::fs::create_dir_all(&out).unwrap();
    for category in std::fs::read_dir(&icons).unwrap() {
        let category = category.unwrap().path();
        if !category.is_dir() {
            continue;
        }
        let mut files: Vec<_> = std::fs::read_dir(&category)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
            .collect();
        files.sort();
        let rows = (files.len() as u32).div_ceil(COLUMNS);
        let width = COLUMNS * (CELL + GAP) + GAP;
        let height = rows * (CELL + GAP) + GAP;
        let mut sheet = tiny_skia::Pixmap::new(width, height).unwrap();
        sheet.fill(tiny_skia::Color::from_rgba8(38, 38, 44, 255));
        for (index, file) in files.iter().enumerate() {
            let x = GAP + (index as u32 % COLUMNS) * (CELL + GAP);
            let y = GAP + (index as u32 / COLUMNS) * (CELL + GAP);
            sheet.draw_pixmap(
                x as i32,
                y as i32,
                render(file).as_ref(),
                &tiny_skia::PixmapPaint::default(),
                tiny_skia::Transform::identity(),
                None,
            );
        }
        let name = category.file_name().unwrap().to_string_lossy().into_owned();
        sheet.save_png(out.join(format!("{name}.png"))).unwrap();
        println!(
            "{name}: {}",
            files
                .iter()
                .map(|f| f.file_stem().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

fn write_if_changed(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    std::fs::write(path, contents)
}

fn main() {
    // 1. Export iconflow fonts to ui/assets/fonts/ directory and generate Slint imports
    let fonts_dir = std::path::Path::new("ui/assets/fonts");
    std::fs::create_dir_all(fonts_dir).unwrap();

    let mut slint_imports = String::new();
    slint_imports.push_str("// Generated automatically by build.rs. DO NOT EDIT.\n");

    for font in iconflow::fonts() {
        let font_path = fonts_dir.join(format!("{}.ttf", font.family));
        write_if_changed(&font_path, font.bytes).unwrap();
        // Append to Slint imports file
        slint_imports.push_str(&format!("import \"./assets/fonts/{}.ttf\";\n", font.family));
    }

    // Add dummy component to make the import valid in Slint
    slint_imports.push_str("export component Fonts {}\n");

    write_if_changed(
        std::path::Path::new("ui/imported_fonts.slint"),
        slint_imports.as_bytes(),
    )
    .unwrap();

    // 2. Compile Slint UI
    slint_build::compile("ui/app.slint").unwrap();
}

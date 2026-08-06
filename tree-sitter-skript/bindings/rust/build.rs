fn main() {
    let src_dir = std::path::Path::new("src");

    let mut config = cc::Build::new();
    config.include(src_dir);
    config
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-trigraphs");

    let parser = src_dir.join("parser.c");
    let scanner = src_dir.join("scanner.c");
    config.file(&parser).file(&scanner);
    println!("cargo:rerun-if-changed={}", parser.to_str().unwrap());
    println!("cargo:rerun-if-changed={}", scanner.to_str().unwrap());

    config.compile("tree-sitter-skript");
}

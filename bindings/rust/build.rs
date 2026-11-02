use cc::Build;

fn main() {
    let mut config = Build::new();
    config.std("c11").include("src");

    #[cfg(target_env = "msvc")]
    config.flag("-utf-8");

    for source in ["src/parser.c", "src/scanner.c"] {
        config.file(source);
        println!("cargo:rerun-if-changed={source}");
    }

    config.compile("tree-sitter-cmd");
}

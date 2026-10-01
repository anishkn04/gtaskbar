fn main() {
    // Compiles data/ui/*.gresource.xml into a GResource bundle. The `data/ui`
    // directory is the primary source dir; `data` is added so the XML can
    // reference icons via relative `../icons/...` paths.
    glib_build_tools::compile_resources(
        &["data/ui", "data"],
        "data/ui/gtaskbar.gresource.xml",
        "gtaskbar.gresource",
    );
}

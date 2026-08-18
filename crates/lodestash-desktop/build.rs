fn main() {
    tauri_build::build();

    // Give **test** binaries the Common-Controls v6 manifest the app binary already gets.
    //
    // Tauri's window layer (tao/muda/rfd) imports comctl32 v6-only entry points —
    // `SetWindowSubclass`, `RemoveWindowSubclass`, `DefSubclassProc`, `TaskDialogIndirect`. The app
    // exe carries a manifest declaring that assembly, so the loader binds comctl32 v6. A `cargo
    // test` binary carries no manifest, binds the v5 comctl32 in system32, finds none of those
    // exports, and dies with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) **before the first test
    // runs** — the whole binary, not just the test that reached the symbol.
    //
    // That is why this crate's most concurrent code (the writer thread and its extract queue) had
    // no unit tests: any test that kept those symbols reachable made the binary unloadable, which
    // reads as a broken test rather than a missing manifest.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' \
             name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
             processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}

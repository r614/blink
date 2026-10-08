//! The `dishonored_runtime` executable: everything lives in the library (`src/lib.rs`), so the
//! tests in `tests/` link the normal library build.

fn main() {
    dishonored_runtime::main();
}

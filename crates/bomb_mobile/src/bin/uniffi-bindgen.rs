//! `cargo run -p bomb_mobile --features bindgen --bin uniffi-bindgen -- generate …` (see `scripts/build-ios.sh`).
fn main() {
    uniffi::uniffi_bindgen_main()
}

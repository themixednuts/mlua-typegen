pub const RUSTC_DEV: &str = "rustc-dev";
pub const LLVM_TOOLS: &str = "llvm-tools-preview";

pub fn required_components() -> &'static [&'static str] {
    if cfg!(unix) {
        &[RUSTC_DEV, LLVM_TOOLS]
    } else {
        &[RUSTC_DEV]
    }
}

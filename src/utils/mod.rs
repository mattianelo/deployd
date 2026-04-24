pub mod archive;
pub mod fomod_resolver;
pub mod nxm_handler;
pub mod paths;
pub mod plugin_header;
pub mod plugins_txt;
pub mod portal;

/// Debug-only log macro. Compiles to a no-op in release builds (`--release`).
/// `cfg!(debug_assertions)` is a compile-time constant; the optimizer eliminates the dead branch.
/// Import with `use crate::dlog;` in any module.
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            eprintln!($($arg)*)
        }
    };
}

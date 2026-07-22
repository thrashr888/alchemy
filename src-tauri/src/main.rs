// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The macOS debug linker warns that the binary's `__eh_frame` section exceeds
// the 16MB compact-unwind limit ("performance of exception handling might be
// affected"). It's cosmetic and dev-build-only — the debug profile already
// trims debuginfo to line-tables-only, and release codegen doesn't hit it.
#![cfg_attr(debug_assertions, allow(linker_messages))]

fn main() {
    alchemy_lib::run()
}

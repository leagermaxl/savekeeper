//! Rebuilds the crate when the embedded `rules/` folder changes: `include_dir!`
//! tracks the contents of known files but not files added or removed.

fn main() {
    println!("cargo:rerun-if-changed=../../rules");
}

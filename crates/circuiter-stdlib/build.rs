fn main() {
    // `include_dir!` doesn't track the embedded files, so rebuild when they change.
    println!("cargo:rerun-if-changed=library");
}

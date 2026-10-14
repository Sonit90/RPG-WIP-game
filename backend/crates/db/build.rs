// `sqlx::migrate!` embeds the migrations folder at compile time; without this, adding a
// migration does not rebuild the crate and the API starts with a stale set.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}

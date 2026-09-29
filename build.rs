// `sqlx::migrate!()` embeds `migrations/` at compile time, but Cargo only
// tracks Rust sources. Rebuild when a migration is added, removed or edited.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}

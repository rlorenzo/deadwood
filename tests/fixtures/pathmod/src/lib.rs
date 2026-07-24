#[path = "renamed_file.rs"]
mod alias;

pub fn entry() -> u32 {
    alias::child::seven()
}

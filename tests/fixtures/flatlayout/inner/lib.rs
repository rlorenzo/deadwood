pub mod thing;

/// Keeps `thing::used` alive so this fixture's only findings are dead files.
pub fn entry() {
    thing::used();
}

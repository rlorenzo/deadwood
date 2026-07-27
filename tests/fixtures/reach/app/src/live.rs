//! The chain that must stay quiet. `main` calls `start`, `start` calls
//! `middle`, `middle` calls `leaf`, and reachability has to carry the entry
//! point all the way down. A finding anywhere in here is the false positive
//! this whole check is shaped around avoiding.

pub fn start() -> u32 {
    middle()
}

pub fn middle() -> u32 {
    leaf()
}

pub fn leaf() -> u32 {
    1
}

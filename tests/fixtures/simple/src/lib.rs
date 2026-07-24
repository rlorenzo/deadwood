mod used;

pub fn entry() -> u32 {
    used::helper()
}

pub fn dead_fn() -> u32 {
    42
}

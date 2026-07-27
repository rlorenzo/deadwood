//! A private module: nothing outside the crate can name what is in here, so
//! these are not surface roots. They stay quiet only because `exported::entry`
//! reaches them.

pub fn worker() -> u32 {
    detail()
}

pub fn detail() -> u32 {
    2
}

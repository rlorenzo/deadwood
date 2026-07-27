//! The chain from the issue, in full. Nothing names `orphan`, so nothing
//! names `helper` either however plainly `orphan` spells it, and nothing
//! names `deeper` below that.
//!
//! Under reference counting only `orphan` was reported; deleting it surfaced
//! `helper` on the next run and `deeper` on the run after that. All three come
//! out together now.

pub fn orphan() -> u32 {
    helper()
}

pub fn helper() -> u32 {
    deeper()
}

pub fn deeper() -> u32 {
    1
}

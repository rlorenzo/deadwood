include!(concat!(env!("OUT_DIR"), "/thunks.rs"));

pub mod imp {
    pub fn called_from_thunks() {}
}

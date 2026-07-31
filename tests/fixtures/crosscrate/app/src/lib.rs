use engine_core::api::Handle;

fn run() -> u32 {
    engine_core::start() + wheel::spin()
}

fn handle() -> Handle {
    Handle
}

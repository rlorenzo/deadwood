fn via_alias() -> u32 {
    motor::aliased_only()
}

fn also_via_alias() -> motor::api::Handle {
    motor::api::Handle
}

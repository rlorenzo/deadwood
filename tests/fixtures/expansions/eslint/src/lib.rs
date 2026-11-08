lint::rules! { alpha }

pub fn go() {
    lint::warn_once!();
}

#[allow(dead_code)]
struct Key;

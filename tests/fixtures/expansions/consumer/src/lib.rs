use facade::Key;

#[derive(Key)]
struct Keyed;

pub fn keyed() -> usize {
    std::mem::size_of::<Keyed>()
}

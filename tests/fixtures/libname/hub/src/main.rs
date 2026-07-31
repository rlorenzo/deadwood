fn main() {
    // `rim` is `rim-parts`' lib name; reporting the entry unused against
    // this call was #62's finding shape.
    println!("{}", rim::radius() + phantom_crate::hubcap());
}

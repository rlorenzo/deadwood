mod used;

pub fn entry() -> u32 {
    used::helper()
}

pub fn dead_fn() -> u32 {
    42
}

// The clap idiom, six crates strong there: an item compiled only for
// rustdoc's doctest build, whose consumer is rustdoc itself. Referenced by
// nothing, deliberately — deleting it would silently drop the README's
// doctest coverage — so it must not appear in the unused list above.
#[cfg(doctest)]
#[doc = "```\nassert!(1 + 1 == 2);\n```"]
pub struct ReadmeDoctests;

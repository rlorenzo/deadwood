//! Lexical scopes: locals, parameters and generic parameters that share a
//! name with a module item.
//!
//! Every `pub` item here is either shadowed by a binding — and so genuinely
//! unreferenced — or named exactly once, from inside the scope where a
//! binding of that name is live. The drivers are private on purpose: only the
//! `pub` items above them are reportable, so the expected findings are the
//! claim itself and nothing else. The code compiles, which makes Rust the
//! authority on what each construct means.
#![allow(dead_code, non_snake_case)]

// -- shadowed, and named nowhere else: the findings this fixture is for -----

/// Shadowed by the `let` in `shadowed_by_a_local`.
pub fn helper() -> u32 {
    1
}

/// Shadowed by the parameter of `shadowed_by_a_parameter`.
pub fn width() -> u32 {
    2
}

/// Shadowed by the binding a tuple-struct pattern introduces.
pub fn value() -> u32 {
    3
}

/// Shadowed by the generic parameter of `wrap`, in the type namespace.
pub struct Marker;

// -- named for real, and a binding of the same name must not hide it -------

/// Named only as a *type*, inside the scope of a `let` binding of the same
/// name. The struct has a field because a bare pattern can shadow a braced
/// struct and can never shadow a unit one: `let Cfg = ..;` beside `pub struct
/// Cfg;` is rejected outright (E0530), which is why the resolver reads a bare
/// pattern naming a unit struct as a use.
pub struct Cfg {
    pub n: u32,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { n: 0 }
    }
}

/// Named only by its own initializer, before the binding takes effect.
pub fn seeded() -> u32 {
    4
}

/// Named only from a `let ... else` block, which runs where the binding does
/// not exist yet.
pub fn fallback() -> u32 {
    5
}

/// Named only from a `match` arm other than the one binding its name.
pub fn armed() -> u32 {
    6
}

/// Named only after the block whose local shares its name has ended.
pub fn scoped() -> u32 {
    7
}

/// Named only by a tuple-struct pattern, which is a use and not a binding.
pub struct Pair(pub u32);

impl Default for Pair {
    fn default() -> Self {
        Pair(8)
    }
}

/// Named only by a bare `const` pattern, which is also a use, not a binding.
pub const LIMIT: u32 = 9;

/// Named only through a qualified path, which a local can never shadow.
pub mod deep {
    pub fn thing() -> u32 {
        10
    }
}

// -- the drivers -----------------------------------------------------------

fn shadowed_by_a_local() -> u32 {
    let helper = 11;
    helper
}

fn shadowed_by_a_parameter(width: u32) -> u32 {
    width
}

fn ordering() -> u32 {
    // The initializer still names the item: the binding starts after it.
    let seeded = seeded();
    seeded
}

fn namespaces() -> u32 {
    // Binds `Cfg` in the value namespace...
    let mut Cfg = 12;
    Cfg += 1;
    // ...while the type namespace still resolves to the struct.
    let _typed: Cfg = Default::default();
    Cfg
}

fn wrap<Marker>(marker: Marker) -> Marker {
    marker
}

fn destructuring() -> u32 {
    // `Pair` is a use of the struct; only `value` binds.
    let Pair(value) = Default::default();
    value
}

fn classify(v: u32) -> u32 {
    match v {
        // A bare name that resolves to a `const` is a pattern *use* of it.
        LIMIT => 0,
        other => other,
    }
}

fn diverging(v: Option<u32>) -> u32 {
    let Some(armed) = v else {
        return fallback();
    };
    armed
}

fn matching(v: Option<u32>) -> u32 {
    match v {
        Some(armed) => armed,
        None => armed(),
    }
}

fn block_exit() -> u32 {
    {
        let scoped = 13;
        let _ = scoped;
    }
    scoped()
}

fn qualified() -> u32 {
    let deep = 14;
    // A local named `deep` shadows no *path* through the module `deep`.
    deep + deep::thing()
}

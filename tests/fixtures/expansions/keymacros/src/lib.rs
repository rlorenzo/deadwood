use proc_macro::TokenStream;

#[proc_macro_derive(Key)]
pub fn derive_key(input: TokenStream) -> TokenStream {
    let name = name_of(input);
    let _store = manifest_path("tag_store");
    let _helped = keyhelper::helper_tokens();
    quote::quote! { impl ::cache::Key for #name {} }.into()
}

#[proc_macro_attribute]
pub fn marked(_args: TokenStream, item: TokenStream) -> TokenStream {
    let item = proc_macro2::TokenStream::from(item);
    quote::quote! { const _: ::marker::Mark = ::marker::Mark; #item }.into()
}

fn manifest_path(_key: &str) -> proc_macro2::TokenStream {
    proc_macro2::TokenStream::new()
}

fn name_of(input: TokenStream) -> proc_macro2::Ident {
    syn::parse::<syn::DeriveInput>(input)
        .expect("Key can only paint a struct")
        .ident
}

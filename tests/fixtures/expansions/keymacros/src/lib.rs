use proc_macro::TokenStream;

#[proc_macro_derive(Key)]
pub fn derive_key(input: TokenStream) -> TokenStream {
    let name = name_of(input);
    quote::quote! { impl ::cache::Key for #name {} }.into()
}

fn name_of(input: TokenStream) -> proc_macro2::Ident {
    syn::parse::<syn::DeriveInput>(input).unwrap().ident
}

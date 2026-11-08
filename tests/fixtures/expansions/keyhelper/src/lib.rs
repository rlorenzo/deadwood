pub fn helper_tokens() -> proc_macro2::TokenStream {
    quote::quote! { impl ::helped::Helped for X {} }
}

pub fn context_tokens() -> proc_macro2::TokenStream {
    quote::quote! { const CONTEXT: ::jsonish::Value = ::jsonish::Value; }
}

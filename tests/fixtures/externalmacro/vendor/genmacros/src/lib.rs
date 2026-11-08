use proc_macro::TokenStream;

#[proc_macro]
pub fn generate_context(_input: TokenStream) -> TokenStream {
    codegen::context_tokens().into()
}

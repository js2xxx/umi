use proc_macro2::{Ident, Span, TokenStream};
use quote::quote_spanned;

pub enum InferredBound {
    Send,
    Sync,
}

impl InferredBound {
    fn as_str(&self) -> &str {
        match self {
            InferredBound::Send => "Send",
            InferredBound::Sync => "Sync",
        }
    }

    pub fn spanned_path(&self, span: Span) -> TokenStream {
        let ident = Ident::new(self.as_str(), span);
        quote_spanned!(span=> ::core::marker::#ident)
    }
}

impl PartialEq<InferredBound> for Ident {
    fn eq(&self, bound: &InferredBound) -> bool {
        self == bound.as_str()
    }
}

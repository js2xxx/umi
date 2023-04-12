use proc_macro2::Span;
use syn::{
    parse::{Parse, ParseStream},
    Error, ItemFn, Result,
};

pub struct Item(pub ItemFn);

impl Parse for Item {
    fn parse(input: ParseStream) -> Result<Self> {
        let func = input.parse::<ItemFn>()?;
        let sig = &func.sig;
        if sig.asyncness.is_none() {
            return Err(Error::new(Span::call_site(), "expect an async fn"));
        }
        Ok(Item(func))
    }
}

mod bound;
mod expand;
mod lifetime;
mod parse;
mod receiver;

use proc_macro::TokenStream;
use syn::parse_macro_input;

use crate::parse::Item;

/// Attribute to an async function to make an async handler.
///
/// The real async handler looks like this:
///
/// ```ignore
/// fn real_handler<'a>(_: &'a u32, _: Lifetimed<'a, 'a, u32>, ..)
///     -> Pin<Box<dyn Future<Output = ..> + Send + 'a>> {
///     Box::pin(async move {
///         // Real content here...
///     })
/// }
/// ```
///
/// All the possible lifetimes must be equivalent the same lifetime as the
/// output future, if a custom implementation is preferred.
///
/// The implementation is based on [`async-trait`](https://github.com/dtolnay/async-trait).
#[proc_macro_attribute]
pub fn async_handler(args: TokenStream, input: TokenStream) -> TokenStream {
    assert!(args.is_empty(), "expect no arguments");
    let mut item = parse_macro_input!(input as Item);
    expand::expand(&mut item);
    let func = item.0;
    quote::quote!(#func).into()
}

use std::mem;

use proc_macro2::{Ident, Span};
use quote::{format_ident, quote, quote_spanned};
use syn::{
    parse_quote, parse_quote_spanned, punctuated::Punctuated, visit_mut::VisitMut, Attribute,
    Block, FnArg, GenericArgument, GenericParam, Pat, PatIdent, PathArguments, Receiver,
    ReturnType, Signature, Token, Type, WhereClause,
};

use crate::{
    bound::InferredBound,
    lifetime::{AddLifetimeToImplTrait, CollectLifetimes},
    parse::Item,
    receiver::{has_self_in_block, has_self_in_sig, mut_pat, ReplaceSelf},
};

pub fn expand(item: &mut Item) {
    let func = &mut item.0;

    let sig = &mut func.sig;
    let block = &mut func.block;
    let has_self = has_self_in_sig(sig) || has_self_in_block(block);
    func.attrs.push(parse_quote!(#[must_use]));
    transform_block(sig, block);
    transform_sig(sig, has_self);
    func.attrs.push(lint_suppress_with_body());
}

fn transform_sig(sig: &mut Signature, has_self: bool) {
    let default_span = sig.asyncness.take().unwrap().span;
    sig.fn_token.span = default_span;

    let (ret_arrow, ret) = match &sig.output {
        ReturnType::Default => (Token![->](default_span), quote_spanned!(default_span=> ())),
        ReturnType::Type(arrow, ret) => (*arrow, quote!(#ret)),
    };

    let mut lifetimes = CollectLifetimes;
    for arg in sig.inputs.iter_mut() {
        match arg {
            FnArg::Receiver(arg) => lifetimes.visit_receiver_mut(arg),
            FnArg::Typed(arg) => lifetimes.visit_type_mut(&mut arg.ty),
        }
    }
    let generics = mem::take(&mut sig.generics.params)
        .into_iter()
        .filter_map(|mut param| {
            match &mut param {
                GenericParam::Type(param) => {
                    let param_name = &param.ident;
                    let span = match param.colon_token.take() {
                        Some(colon_token) => colon_token.span,
                        None => param_name.span(),
                    };
                    let bounds = mem::replace(&mut param.bounds, Punctuated::new());
                    where_clause_or_default(&mut sig.generics.where_clause)
                        .predicates
                        .push(parse_quote_spanned!(span=> #param_name: 'async_trait + #bounds));
                }
                GenericParam::Lifetime(_) => return None,
                GenericParam::Const(_) => {}
            }
            Some(param)
        });
    sig.generics.params = generics.collect();

    if sig.generics.lt_token.is_none() {
        sig.generics.lt_token = Some(Token![<](sig.ident.span()));
    }
    if sig.generics.gt_token.is_none() {
        sig.generics.gt_token = Some(Token![>](sig.paren_token.span.join()));
    }

    sig.generics
        .params
        .push(parse_quote_spanned!(default_span=> 'async_trait));

    if has_self {
        let bounds: &[InferredBound] = if let Some(receiver) = sig.receiver() {
            match receiver.ty.as_ref() {
                // self: &Self
                Type::Reference(ty) if ty.mutability.is_none() => &[InferredBound::Sync], /* self: Arc<Self> */
                Type::Path(ty)
                    if {
                        let segment = ty.path.segments.last().unwrap();
                        segment.ident == "Arc"
                            && match &segment.arguments {
                                PathArguments::AngleBracketed(arguments) => {
                                    arguments.args.len() == 1
                                        && match &arguments.args[0] {
                                            GenericArgument::Type(Type::Path(arg)) => {
                                                arg.path.is_ident("Self")
                                            }
                                            _ => false,
                                        }
                                }
                                _ => false,
                            }
                    } =>
                {
                    &[InferredBound::Sync, InferredBound::Send]
                }
                _ => &[InferredBound::Send],
            }
        } else {
            &[InferredBound::Send]
        };

        let bounds = bounds.iter().map(|bound| bound.spanned_path(default_span));

        where_clause_or_default(&mut sig.generics.where_clause)
            .predicates
            .push(parse_quote_spanned! {default_span=>
                Self: #(#bounds +)* 'async_trait
            });
    }

    for (i, arg) in sig.inputs.iter_mut().enumerate() {
        match arg {
            FnArg::Receiver(receiver) => {
                if receiver.reference.is_none() {
                    receiver.mutability = None;
                }
            }
            FnArg::Typed(arg) => {
                if !matches!(*arg.ty, Type::Reference(_)) {
                    if let Pat::Ident(pat) = &mut *arg.pat {
                        pat.by_ref = None;
                        pat.mutability = None;
                    } else {
                        let positional = positional_arg(i, &arg.pat);
                        let m = mut_pat(&mut arg.pat);
                        arg.pat = parse_quote!(#m #positional);
                    }
                }
                AddLifetimeToImplTrait.visit_type_mut(&mut arg.ty);
            }
        }
    }

    let bounds = quote_spanned!(default_span => ::core::marker::Send + 'async_trait);
    sig.output = parse_quote_spanned! {default_span =>
        #ret_arrow ::core::pin::Pin<Box<
            dyn ::core::future::Future<Output = #ret> + #bounds
        >>
    };
}

fn transform_block(sig: &mut Signature, block: &mut Block) {
    let mut self_span = None;
    let decls = sig
        .inputs
        .iter()
        .enumerate()
        .map(|(i, arg)| match arg {
            FnArg::Receiver(Receiver {
                self_token,
                mutability,
                ..
            }) => {
                let ident = Ident::new("__self", self_token.span);
                self_span = Some(self_token.span);
                quote!(let #mutability #ident = #self_token;)
            }
            FnArg::Typed(arg) => {
                // If there is a #[cfg(...)] attribute that selectively enables
                // the parameter, forward it to the variable.
                //
                // This is currently not applied to the `self` parameter.
                let attrs = arg.attrs.iter().filter(|attr| attr.path().is_ident("cfg"));

                if let Type::Reference(_) = *arg.ty {
                    quote!()
                } else if let Pat::Ident(PatIdent {
                    ident, mutability, ..
                }) = &*arg.pat
                {
                    quote! {
                        #(#attrs)*
                        let #mutability #ident = #ident;
                    }
                } else {
                    let pat = &arg.pat;
                    let ident = positional_arg(i, pat);
                    if let Pat::Wild(_) = **pat {
                        quote! {
                            #(#attrs)*
                            let #ident = #ident;
                        }
                    } else {
                        quote! {
                            #(#attrs)*
                            let #pat = {
                                let #ident = #ident;
                                #ident
                            };
                        }
                    }
                }
            }
        })
        .collect::<Vec<_>>();

    if let Some(span) = self_span {
        let mut replace_self = ReplaceSelf(span);
        replace_self.visit_block_mut(block);
    }

    let stmts = &block.stmts;
    let let_ret = match &mut sig.output {
        ReturnType::Default => quote_spanned! {block.brace_token.span=>
            #(#decls)*
            let _: () = { #(#stmts)* };
        },
        ReturnType::Type(_, ret) => {
            quote_spanned! {block.brace_token.span=>
                if let ::core::option::Option::Some(__ret) = ::core::option::Option::None::<#ret> {
                    return __ret;
                }
                #(#decls)*
                let __ret: #ret = { #(#stmts)* };
                #[allow(unreachable_code)]
                __ret
            }
        }
    };
    let box_pin = quote_spanned!(block.brace_token.span=>
        Box::pin(async move { #let_ret })
    );
    block.stmts = parse_quote!(#box_pin);
}

fn positional_arg(i: usize, pat: &Pat) -> Ident {
    let span: Span = syn::spanned::Spanned::span(pat);
    #[cfg(not(no_span_mixed_site))]
    let span = span.resolved_at(Span::mixed_site());
    format_ident!("__arg{}", i, span = span)
}

fn lint_suppress_with_body() -> Attribute {
    parse_quote! {
        #[allow(
            clippy::async_yields_async,
            clippy::let_unit_value,
            clippy::no_effect_underscore_binding,
            clippy::shadow_same,
            clippy::type_complexity,
            clippy::type_repetition_in_bounds,
            clippy::used_underscore_binding
        )]
    }
}

fn where_clause_or_default(clause: &mut Option<WhereClause>) -> &mut WhereClause {
    clause.get_or_insert_with(|| WhereClause {
        where_token: Default::default(),
        predicates: Punctuated::new(),
    })
}

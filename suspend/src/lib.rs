#![feature(box_syntax)]

extern crate proc_macro;

use syn::{TypeBareFn, ImplItemMethod, Block, Stmt, Expr, *};

use quote::quote;
use std::any::Any;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use proc_macro2::Span;
use std::iter::FromIterator;
use syn::token::Token;
use quote::ToTokens;

#[proc_macro_attribute]
pub fn suspend(metadata: proc_macro::TokenStream, input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    if let Ok(ref mut item) = syn::parse::<ItemImpl>(input.clone()) {
        Vis {}.visit_item_impl_mut(item);
        quote!(#item).into()
    } else if let Ok(ref mut method) = syn::parse(input) {
        Vis {}.visit_impl_item_method_mut(method);
        //panic!("method : {}", method.to_token_stream());
        quote!(#method).into()
    } else {
        panic!("Need an impl or a method item")
    }
}

use syn::visit_mut::*;

struct Vis {}

impl VisitMut for Vis {
    fn visit_expr_mut(&mut self, base_expr: &mut Expr) {
        if let Expr::Await(ref mut expr) = base_expr {
            self.visit_expr_mut(&mut expr.base);
            let base = &expr.base;
            let block = parse2::<Block>(
                quote::quote_spanned! {  expr.await_token.span =>
                    {
                        let (mut tmp, res) =  self.suspend(#base).await;
                        self = tmp;
                        res
                    }
                }).unwrap();
            *base_expr = Expr::Block(ExprBlock {
                block,
                label: None,
                attrs: vec![],
            });
        } else {
            visit_expr_mut(self, base_expr);
        }
    }
}

#[test]
fn test_suspending() {
    let code = r##"
#[suspend::suspend]
async fn handle(self: &mut Context<Self>, msg: TestMessage) -> i32 {
    self.x += 1;
    let res = delay_for(Duration::from_secs((2 * x) as _)).await.await;
    self.x -= 1;
    return self.x;
}
"##;

    let generated = suspend_impl(syn::parse_str(code).unwrap());
    panic!("Generated : {:?}", generated.to_string())
}
#![feature(box_syntax)]

extern crate proc_macro;

use syn::*;
use syn::visit_mut::*;

use quote::{
    quote, quote_spanned, ToTokens,
};
use proc_macro2::Span;

use darling::FromMeta;

#[derive(Debug, FromMeta)]
struct SuspendMeta {
    #[darling(rename = "self")]
    self_ident: Ident,
}


#[proc_macro_attribute]
pub fn suspend(_metadata: proc_macro::TokenStream, input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let attr_args = parse_macro_input!(_metadata as AttributeArgs);
    let meta = SuspendMeta::from_list(&attr_args).unwrap_or(
        SuspendMeta {
            self_ident: Ident::new("self", Span::call_site())
        }
    );


    if let Ok(ref mut item) = syn::parse::<ItemImpl>(input.clone()) {
        Vis {
            self_ident: meta.self_ident,
        }.visit_item_impl_mut(item);
        quote!(#item).into()
    } else if let Ok(ref mut method) = syn::parse(input.clone()) {
        Vis {
            self_ident: meta.self_ident,
        }.visit_impl_item_method_mut(method);
        //panic!("method : {}", method.to_token_stream());
        quote!(#method).into()
    } else if let Ok(ref mut expr) = syn::parse(input.clone()) {
        Vis {
            self_ident: meta.self_ident,
        }.visit_expr_mut(expr);
        panic!("expr : {}", expr.to_token_stream());
        quote!(#expr).into()
    } else {
        panic!("Need an impl or a method item")
    }
}

struct Vis {
    self_ident: Ident,
}

impl VisitMut for Vis {
    fn visit_attribute_mut(&mut self, a: &mut Attribute) {
        if let Ok(meta) = a.parse_meta() {
            let self_ident_new = if let Ok(susp_meta) = SuspendMeta::from_meta(&meta) {
                susp_meta.self_ident
            } else {
                Ident::new("self", Span::call_site())
            };
            self.self_ident = self_ident_new;
        }
    }

    fn visit_expr_mut(&mut self, base_expr: &mut Expr) {
        if let Expr::Await(ref mut expr) = base_expr {
            self.visit_expr_mut(&mut expr.base);
            let base = &expr.base;
            let ident = &self.self_ident;
            let syntax = quote_spanned! {
                expr.await_token.span => {
                    let _fut = #base;
                    let (mut _tmp, _res) =  #ident.suspend(_fut).await;
                    #ident = _tmp;
                    _res
                }
            };

            let block = parse2::<Block>(syntax).unwrap();
            *base_expr = Expr::Block(ExprBlock {
                block,
                label: None,
                attrs: vec![],
            });
        } else {
            let old_self_ident = self.self_ident.clone();
            visit_expr_mut(self, base_expr);
            self.self_ident = old_self_ident;
        }
    }


    fn visit_local_mut(&mut self, i: &mut Local) {
        let old_self_ident = self.self_ident.clone();
        visit_local_mut(self,i);
        self.self_ident = old_self_ident;
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
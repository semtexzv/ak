#![feature(box_syntax, futures_api, await_macro, async_await, arbitrary_self_types, try_trait, specialization, impl_trait_in_bindings)]
#![allow(unused_imports, unused_mut, unused_variables, dead_code)]

pub mod prelude;
pub mod actor;
pub mod addr;
pub mod context;
pub mod types;

pub(crate) use crate::prelude::*;

pub(crate) use crate::actor::*;
pub(crate) use crate::context::*;
pub(crate) use crate::types::*;

macro_rules! suspend {
    ($this: ident, $expr : expr) => {{
         let (mut tmp, res) = await!($this.suspend($expr));
         $this = tmp;
         res
    }};
}

macro_rules! try_suspend {
    ($this: ident, $expr : expr) => {{
         let (mut tmp, res) = await!($this.suspend($expr));
         $this = tmp;
         if let Err(res) = res {
            return ($this,Err(res.into()));
         }
         res.unwrap()
    }};
}
#[cfg(test)]
mod tests {
    use super::*;

    fn computation() -> Box<Future<Output=()> + Unpin> {
        panic!("Unimplemented")
    }

    fn computation_res() -> Box<Future<Output=Result<(), String>> + Unpin> {
        panic!("Unimplemented")
    }

    struct TestMessage;

    struct TestActor {
        x: i32,
    }

    impl Actor for TestActor {}

    impl Handler<TestMessage> for TestActor {
        type Result = Result<(), String>;

        fn handle(mut self: Context<Self>, m: TestMessage) -> Return<Self, Self::Result> {
            Return::fut(async {
                let computed = suspend!(self, computation());
                self.x += 1;
                let comp2= try_suspend!(self, computation_res());

                self.spawn(computation());
                self.x += 1;
                (self, Ok(()))
            })
        }
    }

    #[test]
    fn test_size() {
        let ta = TestActor {
            x: 0
        };

        let ctx = Context::new(ta);
        let _ = ctx.handle(TestMessage);
    }
}
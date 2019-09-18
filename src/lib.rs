#![feature(box_syntax, arbitrary_self_types, try_trait, specialization)]
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
         let (mut tmp, res) = $this.suspend($expr).await;
         $this = tmp;
         res
    }};
}

macro_rules! try_suspend {
    ($this: ident, $expr : expr) => {{
         let (mut tmp, res) = $this.suspend($expr).await;
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
    use crate::addr::Message;

    fn computation() -> Box<dyn Future<Output=()> + Unpin> {
        panic!("Unimplemented")
    }

    fn computation_res() -> Box<dyn Future<Output=Result<(), String>> + Unpin> {
        panic!("Unimplemented")
    }

    struct TestMessage;

    impl Message for TestMessage {
        type Result = ();
    }

    struct TestActor {
        x: i32,
    }

    impl Actor for TestActor {}

    impl Handler<TestMessage> for TestActor {
        type Result = Result<(), String>;

        fn handle(mut self: Context<Self>, m: TestMessage) -> Return<Self, Self::Result> {
            async {
                let computed = suspend!(self, computation());
                self.x += 1;
                let comp2 = try_suspend!(self, computation_res());

                self.spawn(computation());
                self.x += 1;
                (self, Ok(()))
            }.into()
        }
    }

    #[tokio::test]
    async fn test_basic() {
        let ta = TestActor {
            x: 0
        };

        let addr = Context::new(|| ta);
        let res = addr.send(TestMessage).boxed_local().await;
        std::thread::sleep_ms(200);
        //let res = ctx.handle(TestMessage);
        //panic!("Res : {:?}", res);
    }
}
#![feature(arbitrary_self_types)]
#![feature(type_alias_impl_trait)]

use tokio::*;
use std::future::Future;
use futures::future::FutureExt;
use futures::future::LocalBoxFuture;

pub(crate) use ak::prelude::*;
pub(crate) use ak::actor::*;
pub(crate) use ak::context::*;
pub(crate) use ak::types::*;


use ak::addr::Message;

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

impl TestActor {
    async fn bla(&self) -> i32 {
        self.x
    }
}


impl Handler<TestMessage> for TestActor {
    type Result = impl Future<Output=()>;

    fn handle(mut self: &mut Context<Self>, msg: TestMessage) -> Self::Result {
        self.x += 1;

        let this = self.get_ref();

        async {
            let (this2,compute) = this.within(|this| async move {
                this.bla().await
            }).await;


            let y = compute;
            print!("y {:?}", y);
        };
        async {}
    }
}

#[tokio::main]
async fn main() {
    let ta = TestActor {
        x: 0
    };

    let addr = Context::start(|| ta);

    for i in 0..50 {
        let res = addr.send(TestMessage).boxed_local().await;
        assert_eq!(res, Ok(()));
    }
}
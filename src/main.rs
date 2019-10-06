#![feature(arbitrary_self_types)]
#![feature(type_alias_impl_trait)]
#![feature(generators, generator_trait)]

use std::future::Future;
use futures::future::FutureExt;
use futures::future::LocalBoxFuture;

pub(crate) use ak::prelude::*;
pub(crate) use ak::actor::*;
pub(crate) use ak::context::*;
pub(crate) use ak::types::*;

use async_trait::async_trait;
use ak::addr::Message;
use tokio::timer::delay_for;
use std::time::Duration;
use std::ops::Generator;
use futures::StreamExt;

fn computation() -> Box<dyn Future<Output=()> + Unpin> {
    panic!("Unimplemented")
}

fn computation_res() -> Box<dyn Future<Output=Result<(), String>> + Unpin> {
    panic!("Unimplemented")
}

struct TestMessage;

impl Message for TestMessage {
    type Result = i32;
}

struct TestActor {
    x: i32,
}

impl Actor for TestActor {}

impl TestActor {
    async fn bla(&mut self) -> &i32 {
        &self.x
    }
}

impl Handler<TestMessage> for TestActor {
    type Future = impl Future<Output=i32> + 'static;


    #[suspend::suspend]
    fn handle(mut self: ContextRef<Self>, msg: TestMessage) -> Self::Future {
        async move {
            self.x += 1;
            let x = self.x;
            println!("Suspending {:?}", x);
            self.stop();
            if self.x > 1 {
                delay_for(Duration::from_secs((2 * x) as _)).await;
            }
            println!("Continuing {:?}", x);
            return x;
        }
    }
}

#[tokio::main]
async fn main() {
    let ta = TestActor {
        x: 0
    };

    let addr = Context::start(|| ta);

    let mut sent = vec![];
    for i in 0..3i32 {
        sent.push(addr.send(TestMessage).boxed_local());
        println!("Sent")
    }
    sent.pop();
    let res = futures::future::join_all(sent).await;

    for i in res.into_iter() {
        println!("Received count : {:?}", i)
    }
}
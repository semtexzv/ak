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
use ak::addr::{Message, Addr};
use tokio::timer::delay_for;
use std::time::Duration;
use std::ops::Generator;
use futures::StreamExt;
use std::sync::mpsc::Sender;

struct Payload(usize);

impl Message for Payload {
    type Result = ();
}

struct Node {
    limit: usize,
    next: Addr<Node>,
}

impl Actor for Node {}

impl Handler<Payload> for Node {
    type Future = impl Future<Output=()> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: Payload) -> Self::Future {
        async move {
            if msg.0 >= self.limit {
                println!("Reached limit of {} (payload was {})", self.limit, msg.0);
                self.stop();
                return;
            }
            self.next.send(Payload(msg.0 + 1)).await;
        }
    }
}


const NUM_NODES : usize = 500;
const NUM_MSGS : usize = 500;

#[tokio::main]
async fn main() {

    /*
    let node =
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
    */
}
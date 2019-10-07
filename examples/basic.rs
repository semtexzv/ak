#![feature(arbitrary_self_types)]
#![feature(type_alias_impl_trait)]
#![feature(stmt_expr_attributes)]
#![feature(proc_macro_hygiene)]

pub use ak::*;
use std::time::SystemTime;


struct Payload(usize);

impl Message for Payload {
    type Result = ();
}

struct Node {
    id: usize,
    limit: usize,
    next: Option<Addr<Node>>,
}

impl Actor for Node {}

impl Handler<Payload> for Node {
    type Future = impl Future<Output=()> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: Payload) -> Self::Future {
        async move {
            if msg.0 >= self.limit {
                println!("Reached limit of {} (payload was {}) on node {}", self.limit, msg.0, self.id);
                self.stop();
                return;
            }
            if let Some(next) = &self.next {
                next.send(Payload(msg.0 + 1)).await;
            } else {
                panic!("Nodes werent properly chained");
            }
        }
    }
}

struct FirstAddr(Addr<Node>);

impl Message for FirstAddr { type Result = (); }

impl Handler<FirstAddr> for Node {
    type Future = impl Future<Output=()> + 'static;

    #[ak::suspend]
    fn handle(mut self: ContextRef<Self>, msg: FirstAddr) -> Self::Future {
        async {
            if let Some(next) = &self.next {
                next.send(msg).await;
            } else {
                self.next = Some(msg.0);
            }
        }
    }
}


const NUM_NODES: usize = 1000;
const NUM_MSGS: usize = 1000;


fn main() {
    ak::rt::System::run(|| {
        fn create(limit: usize, count: usize) -> Option<Addr<Node>> {
            if count > 0 {
                Some(Node::start(move |_| Node {
                    id: NUM_NODES - count,
                    limit,
                    next: create(limit, count - 1),
                }))
            } else {
                None
            }
        };

        ak::rt::spawn(async {
            let first_node = create(NUM_NODES * NUM_MSGS, NUM_NODES).unwrap();
            first_node.send(FirstAddr(first_node.clone())).await;

            let t = SystemTime::now();
            first_node.send(Payload(0)).await;

            let elapsed = t.elapsed().unwrap();
            println!("Elapsed : {}.{:06} seconds", elapsed.as_secs(), elapsed.subsec_micros());
        });
    });
}
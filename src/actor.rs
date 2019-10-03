use crate::prelude::*;
use crate::context::AsyncRef;
use crate::{Context};
use crate::addr::{Message, Response};


pub trait Actor: Sized + 'static {}


pub trait Handler<M: Message>: Actor {
    type Result: Response<Self, M>;
    fn handle(self: &mut Context<Self>, msg: M) -> Self::Result;
}


#[async_trait::async_trait]
pub trait AsyncHandler<M: Message>: Actor {
    type Result: Into<M::Result>;
    async fn handle_async(this: AsyncRef<Self>, msg: M) -> Self::Result;
}

use tokio::prelude::Future;
use tokio::spawn;

pub fn run(f: impl Future<Output=()>) {
    tokio_executor::current_thread::block_on_all(async {
        f.await;
    })
}

pub fn start(fut: impl Future<Output=()> + 'static) {
    tokio_executor::current_thread::spawn(fut);
}
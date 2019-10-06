use tokio::prelude::Future;

use tokio::runtime::current_thread::Runtime;

thread_local! {

};

pub fn run(f: impl Future<Output=()>) {
    tokio::runtime::current_thread::Runtime::new().unwrap().block_on(async {
        f.await;
    });
}

pub fn spawn<F, Fut>(f: F)
    where F: FnOnce() -> Fut ,
          Fut: Future<Output=()> + 'static
{
    tokio::runtime::current_thread::spawn(f());
}
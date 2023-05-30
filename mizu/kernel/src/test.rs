use alloc::{
    string::ToString,
    sync::{Arc, Weak},
    vec,
};
use core::pin::pin;

use futures_util::{stream, StreamExt};
use umifs::{traits::Entry, types::OpenOptions};

use crate::task::InitTask;

#[allow(dead_code)]
pub async fn comp2(rt: Arc<dyn Entry>) {
    let oo = OpenOptions::RDONLY;
    let perm = Default::default();

    let scripts = ["run-dynamic.sh", "run-static.sh"];

    let rt2 = rt.clone();
    let stream = stream::iter(scripts)
        .then(|sh| rt2.clone().open(sh.as_ref(), oo, perm))
        .flat_map(|res| {
            let (sh, _) = res.unwrap();
            let io = sh.to_io().unwrap();
            umio::lines(io).map(|res| res.unwrap())
        });
    let mut cmd = pin!(stream);

    let (runner, _) = rt.open("runtest".as_ref(), oo, perm).await.unwrap();
    let runner = Arc::new(crate::mem::new_phys(runner.to_io().unwrap(), true));

    log::warn!("Start testing");
    while let Some(cmd) = cmd.next().await {
        log::info!("Executing cmd {cmd:?}");

        let init = InitTask::from_elf(
            Weak::new(),
            &runner,
            crate::mem::new_virt(),
            cmd.split(' ').map(|s| s.to_string()).collect(),
            vec![],
        )
        .await
        .unwrap();
        let task = init.spawn().unwrap();
        let code = task.wait().await;
        log::info!("cmd {cmd:?} returned with {code:?}\n");
    }

    log::warn!("Goodbye!");
}

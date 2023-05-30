use alloc::{
    string::{String, ToString},
    sync::{Arc, Weak},
    vec,
};
use core::pin::pin;

use futures_util::{stream, StreamExt};
use sygnal::Sig;
use umifs::{path::Path, traits::Entry, types::OpenOptions};

use crate::task::InitTask;

#[allow(dead_code)]
pub async fn libc_test(rt: Arc<dyn Entry>) {
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

async fn run_busybox(rt: Arc<dyn Entry>, script: impl Into<String>) -> (i32, Option<Sig>) {
    let oo = OpenOptions::RDONLY;
    let perm = Default::default();

    let (busybox, _) = rt.open("busybox".as_ref(), oo, perm).await.unwrap();
    let busybox = crate::mem::new_phys(busybox.to_io().unwrap(), true);

    let task = crate::task::InitTask::from_elf(
        Default::default(),
        &Arc::new(busybox),
        crate::mem::new_virt(),
        vec!["busybox".into(), "sh".into(), script.into()],
        vec!["PATH=/".into(), "LD_LIBRARY_PATH=/".into()],
    )
    .await
    .unwrap();

    let task = task.spawn().unwrap();
    task.wait().await
}

async fn print_file(path: impl AsRef<Path>) {
    let (file, _) = crate::fs::open(path.as_ref(), Default::default(), Default::default())
        .await
        .unwrap();
    let mut lines = core::pin::pin!(umio::lines(file.to_io().unwrap()));
    while let Some(result) = lines.next().await {
        log::info!("{}", result.unwrap());
    }
}

#[allow(dead_code)]
pub async fn busybox(rt: Arc<dyn Entry>) {
    let exit = run_busybox(rt, "busybox_testcode.sh").await;
    log::info!("Busybox test returned with {exit:?}");

    log::info!("result.txt:");
    print_file("result.txt").await;

    log::info!("test.txt:");
    print_file("test.txt").await;
}

#[allow(dead_code)]
pub async fn lua(rt: Arc<dyn Entry>) {
    let exit = run_busybox(rt, "lua_testcode.sh").await;
    log::info!("Lua test returned with {exit:?}");
}

#[allow(dead_code)]
pub async fn lmbench(rt: Arc<dyn Entry>) {
    let exit = run_busybox(rt, "lmbench_testcode.sh").await;
    log::info!("LMBench test returned with {exit:?}");
}

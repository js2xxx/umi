use alloc::sync::Arc;
use core::pin::pin;

use futures_util::{stream, StreamExt};
use sygnal::Sig;
use umifs::{path::Path, types::OpenOptions};

use crate::task::Command;

#[allow(dead_code)]
pub async fn libc() {
    let oo = OpenOptions::RDONLY;
    let perm = Default::default();

    let scripts = ["run-dynamic.sh", "run-static.sh"];

    let stream = stream::iter(scripts)
        .then(|sh| crate::fs::open(sh.as_ref(), oo, perm))
        .flat_map(|res| {
            let (sh, _) = res.unwrap();
            let io = sh.to_io().unwrap();
            umio::lines(io).map(|res| res.unwrap())
        });
    let mut cmd = pin!(stream);

    let (runner, _) = crate::fs::open("runtest".as_ref(), oo, perm).await.unwrap();
    let runner = Arc::new(crate::mem::new_phys(runner.to_io().unwrap(), true));

    log::warn!("Start testing");
    while let Some(cmd) = cmd.next().await {
        log::info!("Executing cmd {cmd:?}");

        let task = Command::new("/runtest")
            .image(runner.clone())
            .args(cmd.split(' '))
            .spawn()
            .await
            .unwrap();

        let code = task.wait().await;
        log::info!("cmd {cmd:?} returned with {code:?}\n");
    }

    log::warn!("Goodbye!");
}

async fn run_busybox(script: &str) -> (i32, Option<Sig>) {
    let task = Command::new("/busybox")
        .open("busybox")
        .await
        .unwrap()
        .args(["busybox", "sh", script])
        .envs(["PATH=/", "LD_LIBRARY_PATH=/"])
        .spawn()
        .await
        .unwrap();

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
pub async fn busybox() {
    let exit = run_busybox("busybox_testcode.sh").await;
    log::info!("Busybox test returned with {exit:?}");

    log::info!("result.txt:");
    print_file("result.txt").await;

    log::info!("test.txt:");
    print_file("test.txt").await;
}

#[allow(dead_code)]
pub async fn lua() {
    let exit = run_busybox("lua_testcode.sh").await;
    log::info!("Lua test returned with {exit:?}");
}

#[allow(dead_code)]
pub async fn lmbench() {
    let exit = run_busybox("lmbench_testcode.sh").await;
    log::info!("LMBench test returned with {exit:?}");
}

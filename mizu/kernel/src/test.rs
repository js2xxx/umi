use alloc::{string::ToString, sync::Arc};
use core::pin::pin;

use futures_util::{stream, StreamExt};
use sygnal::Sig;
use umifs::{path::Path, types::OpenOptions};

use crate::{println, task::Command};

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

async fn run_busybox(script: Option<&str>) -> (i32, Option<Sig>) {
    let mut cmd = Command::new("/busybox");
    cmd.open("busybox").await.unwrap();
    match script {
        Some(script) => cmd.args(["busybox", "sh", script]),
        None => cmd.args(["busybox", "sh"]),
    };
    let envs = [
        "PATH=/",
        "USER=root",
        "_=busybox",
        "SHELL=/busybox",
        "LD_LIBRARY_PATH=/",
        "LOGNAME=root",
        "HOME=/",
    ];
    let task = cmd.envs(envs).spawn().await.unwrap();

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
    let oo = OpenOptions::RDONLY;
    let perm = Default::default();

    let (txt, _) = crate::fs::open("busybox_cmd.txt".as_ref(), oo, perm)
        .await
        .unwrap();
    let txt = txt.to_io().unwrap();
    let stream = umio::lines(txt).map(|s| s.unwrap());
    let mut cmd = pin!(stream);

    let (runner, _) = crate::fs::open("busybox".as_ref(), oo, perm).await.unwrap();
    let runner = Arc::new(crate::mem::new_phys(runner.to_io().unwrap(), true));

    log::warn!("Start testing");
    while let Some(cmd) = cmd.next().await {
        if cmd.starts_with("du") || cmd.starts_with("ls") {
            continue;
        }

        let cmd = "busybox ".to_string() + cmd.trim();
        println!(">>> Executing CMD {cmd:?}");

        let task = Command::new("/busybox")
            .image(runner.clone())
            .args(cmd.split(' '))
            .spawn()
            .await
            .unwrap();

        let code = task.wait().await;
        println!(">>> CMD {cmd:?} returned with {code:?}\n");
    }

    log::info!("test.txt:");
    print_file("test.txt").await;

    log::warn!("Goodbye!");
}

#[allow(dead_code)]
pub async fn lua() {
    let exit = run_busybox(Some("lua_testcode.sh")).await;
    log::info!("Lua test returned with {exit:?}");
}

#[allow(dead_code)]
pub async fn lmbench() {
    let exit = run_busybox(Some("lmbench_testcode.sh")).await;
    log::info!("LMBench test returned with {exit:?}");
}

#[allow(dead_code)]
pub async fn busybox_interact() {
    let exit = run_busybox(None).await;
    println!("<<< {:?}", exit);
}

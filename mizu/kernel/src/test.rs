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
        if cmd.is_empty() {
            continue;
        }
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

const ENVS: [&str; 8] = [
    "PATH=/",
    "USER=root",
    "_=busybox",
    "SHELL=/busybox",
    "ENOUGH=1000000",
    "LD_LIBRARY_PATH=/",
    "LOGNAME=root",
    "HOME=/",
];

async fn run_busybox(script: Option<&str>) -> (i32, Option<Sig>) {
    let mut cmd = Command::new("/busybox");
    cmd.open("busybox").await.unwrap();
    match script {
        Some(script) => cmd.args(["busybox", "sh", script]),
        None => cmd.args(["busybox", "sh"]),
    };
    let task = cmd.envs(ENVS).spawn().await.unwrap();

    task.wait().await
}

async fn print_file(path: impl AsRef<Path>) {
    let (file, _) = crate::fs::open(path.as_ref(), Default::default(), Default::default())
        .await
        .unwrap();
    let mut lines = core::pin::pin!(umio::lines(file.to_io().unwrap()));
    while let Some(result) = lines.next().await {
        println!("{}", result.unwrap());
    }
}

#[allow(dead_code)]
pub async fn busybox_cmd() {
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
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        let cmd = "/busybox ".to_string() + cmd;
        println!(">>> Executing CMD {cmd:?}");

        let task = Command::new("/busybox")
            .image(runner.clone())
            .args(["busybox", "sh", "-c", &cmd])
            .envs(ENVS)
            .spawn()
            .await
            .unwrap();

        let code = task.wait().await;
        println!(">>> CMD {cmd:?} returned with {code:?}\n");
    }

    log::warn!("Goodbye!");
}

#[allow(dead_code)]
pub async fn busybox(print_result: bool) {
    let script = if print_result {
        "busybox_testcode_debug.sh"
    } else {
        "busybox_testcode.sh"
    };
    let exit = run_busybox(Some(script)).await;
    log::info!("Busybox test returned with {exit:?}");

    if print_result {
        println!("result.txt:");
        print_file("result.txt").await;
    }

    log::warn!("Goodbye!");
}

#[allow(dead_code)]
pub async fn lua() {
    let exit = run_busybox(Some("lua_testcode.sh")).await;
    log::info!("Lua test returned with {exit:?}");
}

#[allow(dead_code)]
pub async fn lmbench_cmd() {
    let oo = OpenOptions::RDONLY;
    let perm = Default::default();

    let (txt, _) = crate::fs::open("lmbench_cmd.txt".as_ref(), oo, perm)
        .await
        .unwrap();
    let txt = txt.to_io().unwrap();
    let stream = umio::lines(txt).map(|s| s.unwrap());
    let mut cmd = pin!(stream);

    log::warn!("Start testing");
    while let Some(cmd) = cmd.next().await {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        let cmd = if cmd.starts_with("echo") {
            "/busybox ".to_string() + cmd
        } else {
            "/".to_string() + cmd
        };
        println!(">>> Executing CMD {cmd:?}");

        let task = Command::new(cmd.split_once(' ').unwrap().0)
            .open_executable()
            .await
            .unwrap()
            .args(cmd.split(' '))
            .envs(ENVS)
            .spawn()
            .await
            .unwrap();

        let code = task.wait().await;
        println!(">>> CMD {cmd:?} returned with {code:?}\n");
    }

    log::warn!("Goodbye!");
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

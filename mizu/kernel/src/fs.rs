mod dev;
mod serial;

use alloc::{collections::BTreeMap, sync::Arc};
use core::time::Duration;

use afat32::NullTimeProvider;
use arsc_rs::Arsc;
use crossbeam_queue::ArrayQueue;
use kmem::Phys;
use ksync::{Sender, TryRecvError};
use ktime::sleep;
use spin::RwLock;
use umifs::{
    path::{Path, PathBuf},
    traits::FileSystem,
};

use crate::{dev::blocks, executor};

type FsCollection = BTreeMap<PathBuf, FsHandle>;

struct FsHandle {
    fs: Arsc<dyn FileSystem>,
    unmount: Sender<ArrayQueue<()>>,
}

static FS: RwLock<FsCollection> = RwLock::new(BTreeMap::new());

pub fn mount(path: PathBuf, fs: Arsc<dyn FileSystem>) {
    let fs2 = fs.clone();
    let (tx, rx) = ksync::bounded(1);
    let task = async move {
        loop {
            sleep(Duration::from_secs(1)).await;
            if matches!(rx.try_recv(), Ok(()) | Err(TryRecvError::Closed(Some(())))) {
                let _ = fs2.flush().await;
                break;
            }
            let _ = fs2.flush().await;
        }
    };
    executor().spawn(task).detach();
    let handle = FsHandle { fs, unmount: tx };

    let old = ksync::critical(|| FS.write().insert(path, handle));
    if let Some(old) = old {
        let _ = old.unmount.try_send(());
    }
}

pub fn unmount(path: &Path) {
    let handle = ksync::critical(|| FS.write().remove(path));
    if let Some(fs_handle) = handle {
        let _ = fs_handle.unmount.try_send(());
    }
}

pub fn get(path: &Path) -> Option<(Arsc<dyn FileSystem>, &Path)> {
    let fs = FS.read();
    let mut iter = fs.iter().rev(); // Reverse the iterator for longest-prefix matching.
    iter.find_map(|(p, handle)| match path.strip_prefix(p) {
        Ok(path) => Some((handle.fs.clone(), path)),
        Err(_) => None,
    })
}

pub async fn fs_init() {
    mount("dev".into(), Arsc::new(dev::DevFs));
    for block in blocks() {
        let block_shift = block.block_shift();
        let phys = Phys::new(block.to_io().unwrap(), 0, false);
        if let Ok(fs) =
            afat32::FatFileSystem::new(Arc::new(phys), block_shift, NullTimeProvider).await
        {
            mount("".into(), fs);
            break;
        }
    }
}

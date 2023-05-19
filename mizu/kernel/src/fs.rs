mod cache;
mod dev;
mod pipe;
mod serial;
mod tmp;

use alloc::{collections::BTreeMap, sync::Arc};
use core::{fmt, time::Duration};

use afat32::NullTimeProvider;
use arsc_rs::Arsc;
use crossbeam_queue::ArrayQueue;
use ksc::Error::{self, EACCES, ENOENT};
use ksync::{Sender, TryRecvError};
use ktime::sleep;
use spin::RwLock;
use umifs::{
    path::{Path, PathBuf},
    traits::{Entry, FileSystem},
    types::{OpenOptions, Permissions},
};

pub use self::pipe::pipe;
use crate::{dev::blocks, executor};

type FsCollection = BTreeMap<PathBuf, FsHandle>;

struct FsHandle {
    fs: Arsc<dyn FileSystem>,
    unmount: Sender<ArrayQueue<()>>,
}

impl fmt::Debug for FsHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FsHandle").finish_non_exhaustive()
    }
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
    ksync::critical(|| {
        let fs = FS.read();
        let mut iter = fs.iter().rev(); // Reverse the iterator for longest-prefix matching.
        iter.find_map(|(p, handle)| match path.strip_prefix(p) {
            Ok(path) => Some((handle.fs.clone(), path)),
            Err(_) => None,
        })
    })
}

#[inline]
pub async fn open(
    path: &Path,
    options: OpenOptions,
    perm: Permissions,
) -> Result<(Arc<dyn Entry>, bool), Error> {
    let (fs, path) = get(path).ok_or(ENOENT)?;
    let root_dir = fs.root_dir().await?;
    if path == "" || path == "." {
        Ok((root_dir, false))
    } else {
        root_dir.open(path, options, perm).await
    }
}

#[inline]
pub async fn open_dir(
    path: &Path,
    options: OpenOptions,
    perm: Permissions,
) -> Result<Arc<dyn Entry>, Error> {
    let (entry, _) = open(path, options | OpenOptions::DIRECTORY, perm).await?;
    Ok(entry)
}

pub async fn unlink(path: &Path) -> Result<(), Error> {
    let (entry, _) = open(
        path.parent().ok_or(ENOENT)?,
        OpenOptions::DIRECTORY | OpenOptions::RDWR,
        Permissions::all_same(true, true, false),
    )
    .await?;
    let Some(dir) = entry.to_dir_mut() else {
        return Err(EACCES)
    };
    dir.unlink(path, None).await
}

pub async fn fs_init() {
    mount("dev".into(), Arsc::new(dev::DevFs));
    mount("tmp".into(), Arsc::new(tmp::TmpFs::new()));
    for block in blocks() {
        let block_shift = block.block_shift();
        let phys = crate::mem::new_phys(block.to_io().unwrap(), false);
        if let Ok(fs) =
            afat32::FatFileSystem::new(Arc::new(phys), block_shift, NullTimeProvider).await
        {
            mount("".into(), cache::CachedFs::new(fs).await.unwrap());
            break;
        }
    }
}

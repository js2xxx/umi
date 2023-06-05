mod cache;
mod dev;
mod pipe;
mod serial;
mod tmp;
mod proc;

use alloc::{collections::BTreeMap, sync::Arc};
use core::{fmt, time::Duration};

use afat32::NullTimeProvider;
use arsc_rs::Arsc;
use crossbeam_queue::ArrayQueue;
use ksc::Error::{self, EACCES, ENOENT};
use ksync::channel::mpmc::{Sender, TryRecvError};
use ktime::sleep;
use spin::RwLock;
use umifs::{
    path::{Path, PathBuf},
    traits::{Entry, FileSystem},
    types::{OpenOptions, Permissions},
};
use umio::IoExt;

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
    let (tx, rx) = ksync::channel::bounded(1);
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
    mount("dev/shm".into(), Arsc::new(tmp::TmpFs::new()));
    mount("dev".into(), Arsc::new(dev::DevFs));
    mount("proc".into(), Arsc::new(proc::ProcFs));
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

#[allow(dead_code)]
pub async fn test_file() {
    let options = OpenOptions::RDWR | OpenOptions::APPEND | OpenOptions::CREAT;
    let perm = Permissions::all_same(true, true, false);

    {
        log::trace!("First attempt:");
        log::trace!("OPEN");
        let (file, created) = crate::fs::open("123.txt".as_ref(), options, perm)
            .await
            .unwrap();
        log::trace!("TEST CREATE");
        assert!(created);
        let file = file.to_io().unwrap();
        log::trace!("WRITE 1, 2, 3, 4, 5");
        file.write_all(&[1, 2, 3, 4, 5]).await.unwrap();
        file.flush().await.unwrap();
    }
    {
        log::trace!("Second attempt:");
        log::trace!("OPEN");
        let (file, created) = crate::fs::open("123.txt".as_ref(), options, perm)
            .await
            .unwrap();
        log::trace!("TEST CREATE");
        assert!(!created);
        let file = file.to_io().unwrap();
        log::trace!("WRITE 6, 7, 8, 9, 10");
        file.write_all(&[6, 7, 8, 9, 10]).await.unwrap();
        file.flush().await.unwrap();
    }
    {
        log::trace!("Third attempt:");
        log::trace!("OPEN");
        let (file, created) = crate::fs::open("123.txt".as_ref(), options, perm)
            .await
            .unwrap();
        log::trace!("TEST CREATE");
        assert!(!created);
        let file = file.to_io().unwrap();
        let mut buf = [0; 10];
        log::trace!("READ 10 ELEMENTS");
        file.read_exact_at(0, &mut buf).await.unwrap();
        assert_eq!(buf, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }
}

mod cache;
mod dev;
mod pipe;
mod proc;
mod serial;
mod tmp;

use alloc::{borrow::Cow, collections::BTreeMap, format, sync::Arc};
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

#[derive(Clone)]
struct FsHandle {
    dev: Cow<'static, str>,
    fs: Arsc<dyn FileSystem>,
    flush: Sender<ArrayQueue<()>>,
}

impl fmt::Debug for FsHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FsHandle").finish_non_exhaustive()
    }
}

static FS: RwLock<FsCollection> = RwLock::new(BTreeMap::new());

pub fn mount(path: PathBuf, dev: Cow<'static, str>, fs: Arsc<dyn FileSystem>) {
    let fs2 = fs.clone();
    let (tx, rx) = ksync::channel::bounded(1);
    let task = async move {
        loop {
            sleep(Duration::from_secs(1)).await;
            if let Err(TryRecvError::Closed(_)) = rx.try_recv() {
                let _ = fs2.flush().await;
                break;
            }
            let _ = fs2.flush().await;
        }
    };
    executor().spawn(task).detach();
    let handle = FsHandle { dev, fs, flush: tx };

    let old = ksync::critical(|| FS.write().insert(path, handle));
    if let Some(old) = old {
        let _ = old.flush.try_send(());
    }
}

pub fn sync() {
    let fs = ksync::critical(|| FS.read().clone());
    fs.values().for_each(|fs| drop(fs.flush.try_send(())))
}

pub fn unmount(path: &Path) {
    let handle = ksync::critical(|| FS.write().remove(path));
    if let Some(fs_handle) = handle {
        let _ = fs_handle.flush.try_send(());
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
    mount(
        "dev/shm".into(),
        "tmpfs".into(),
        Arsc::new(tmp::TmpFs::new()),
    );
    mount("dev".into(), "devfs".into(), Arsc::new(dev::DevFs));
    mount("proc".into(), "procfs".into(), Arsc::new(proc::ProcFs));
    mount("tmp".into(), "tmpfs".into(), Arsc::new(tmp::TmpFs::new()));
    for (index, block) in blocks().into_iter().enumerate() {
        let block_shift = block.block_shift();
        let phys = crate::mem::new_phys(block.to_io().unwrap(), false);
        if let Ok(fs) =
            afat32::FatFileSystem::new(Arc::new(phys), block_shift, NullTimeProvider).await
        {
            mount(
                "".into(),
                format!("/dev/block/{index}").into(),
                cache::CachedFs::new(fs).await.unwrap(),
            );
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

    {
        let (tx, rx) = pipe();
        let tx = tx.to_io().unwrap();
        let rx = rx.to_io().unwrap();

        let tx_task = executor().spawn(async move {
            for index in 0..100 {
                tx.write_all(&[index; 100]).await.unwrap();
            }
        });

        let mut buf = [0; 100];
        for index in 0..100 {
            rx.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, [index; 100]);
        }

        tx_task.await;
    }
}

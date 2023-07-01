use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    borrow::Borrow,
    fmt, mem,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use arsc_rs::Arsc;
use async_trait::async_trait;
use crossbeam_queue::SegQueue;
use futures_util::Future;
use hashbrown::{
    hash_map::{Entry, OccupiedEntry, VacantEntry},
    HashMap,
};
use ksc_core::{
    handler::Boxed,
    Error::{self, EINVAL, ENOENT, ENOMEM},
};
use ksync::{
    channel::{
        mpmc::{Receiver, Sender},
        unbounded,
    },
    event::Event,
};
use rand_riscv::RandomState;
use rv39_paging::{PAddr, ID_OFFSET, PAGE_SHIFT, PAGE_SIZE};
use spin::{Lazy, Mutex};
use umio::{advance_slices, ioslice_len, Io, IoExt, IoSlice, IoSliceMut, SeekFrom};

pub static ZERO: Lazy<Arsc<Frame>> = Lazy::new(|| Arsc::new(Frame::new().unwrap()));

pub struct Frame {
    base: PAddr,
    ptr: NonNull<u8>,
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Frame").field(&self.base).finish()
    }
}

unsafe impl Send for Frame {}
unsafe impl Sync for Frame {}

impl Frame {
    pub fn new() -> Result<Self, Error> {
        let laddr = crate::frame::frames()
            .allocate(NonZeroUsize::MIN)
            .ok_or(ENOMEM)?;
        unsafe { laddr.write_bytes(0, PAGE_SIZE) };
        Ok(Frame {
            base: laddr.to_paddr(ID_OFFSET),
            ptr: laddr.as_non_null().unwrap(),
        })
    }

    pub fn base(&self) -> PAddr {
        self.base
    }

    pub fn as_ptr(&self) -> NonNull<[u8]> {
        NonNull::slice_from_raw_parts(self.ptr, PAGE_SIZE)
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { self.as_ptr().as_ref() }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { self.as_ptr().as_mut() }
    }

    pub fn copy(&self, len: usize) -> Result<Frame, Error> {
        let mut f = Self::new()?;
        log::trace!("Frame::copy: self = {:?}, len = {len}", self.base);
        f[..len].copy_from_slice(&self[..len]);
        Ok(f)
    }
}

impl Deref for Frame {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for Frame {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let laddr = self.base.to_laddr(ID_OFFSET);
        unsafe { crate::frame::frames().deallocate(laddr, NonZeroUsize::MIN) }
    }
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.base.eq(&other.base)
    }
}

impl Eq for Frame {}

impl Borrow<PAddr> for Frame {
    fn borrow(&self) -> &PAddr {
        &self.base
    }
}

#[derive(Debug, Clone)]
enum FrameState {
    Shared(Arsc<Frame>, usize),
    Unique(Arsc<Frame>, usize),
}

impl FrameState {
    fn frame(&mut self, write: Option<usize>) -> (Arsc<Frame>, usize) {
        let (frame, len) = match self {
            FrameState::Shared(frame, len) => (frame, len),
            FrameState::Unique(frame, len) => (frame, len),
        };
        if let Some(new_len) = write {
            *len = (*len).max(new_len);
        }
        (frame.clone(), *len)
    }
}

enum Commit {
    Shared(Arsc<Frame>, usize),
    Unique(FrameInfo),
}

enum FrameEntry<'a> {
    NewFi(VacantEntry<'a, usize, FrameInfo, RandomState>, FrameInfo),
    Entry(OccupiedEntry<'a, usize, FrameInfo, RandomState>),
}

impl<'a> FrameEntry<'a> {
    fn get(list: &'a mut FrameList, index: usize) -> Option<Self> {
        match list.frames.entry(index) {
            Entry::Occupied(ent) => Some(FrameEntry::Entry(ent)),
            Entry::Vacant(_) => None,
        }
    }

    fn try_insert(list: &'a mut FrameList, index: usize, fi: FrameInfo) -> Self {
        match list.frames.entry(index) {
            Entry::Occupied(ent) => FrameEntry::Entry(ent),
            Entry::Vacant(ent) => FrameEntry::NewFi(ent, fi),
        }
    }

    fn get_mut(&mut self) -> &mut FrameInfo {
        match self {
            FrameEntry::NewFi(_, fi) => fi,
            FrameEntry::Entry(ent) => ent.get_mut(),
        }
    }

    fn remove(self) {
        if let FrameEntry::Entry(ent) = self {
            ent.remove();
        }
    }

    fn insert(self) {
        if let FrameEntry::NewFi(list, fi) = self {
            list.insert(fi);
        }
    }
}

#[derive(Debug)]
struct FrameInfo {
    state: Option<FrameState>,
    dirty: bool,
}

impl FrameInfo {
    fn new(frame: Arsc<Frame>, len: usize) -> Self {
        FrameInfo {
            state: Some(FrameState::Shared(frame, len)),
            dirty: false,
        }
    }

    fn truncate(&mut self, new_len: usize) {
        let (frame, len) = match &mut self.state {
            Some(FrameState::Shared(frame, len)) => (frame, len),
            Some(FrameState::Unique(frame, len)) => (frame, len),
            _ => return,
        };
        if new_len < *len {
            unsafe { frame.as_ptr().as_mut()[new_len..*len].fill(0) };
        }
        *len = new_len;
    }

    fn branch(&mut self, write: Option<usize>, cow: bool) -> Result<(Commit, bool), Error> {
        // log::trace!("branch write = {write:?} cow = {cow}");
        match mem::take(&mut self.state) {
            Some(FrameState::Shared(frame, len)) => match write {
                None => {
                    self.state = Some(FrameState::Shared(frame.clone(), len));
                    Ok((Commit::Shared(frame, len), false))
                }
                Some(new_len) if !cow => {
                    let len = len.max(new_len);
                    self.dirty = true;
                    self.state = Some(FrameState::Shared(frame.clone(), len));
                    Ok((Commit::Shared(frame, len), false))
                }
                Some(new_len) => {
                    let new_len = len.max(new_len);
                    let new_frame = frame.copy(new_len)?;
                    self.state = Some(FrameState::Unique(frame, new_len));
                    Ok((
                        Commit::Unique(FrameInfo::new(Arsc::new(new_frame), new_len)),
                        false,
                    ))
                }
            },
            Some(FrameState::Unique(frame, len)) => Ok((
                Commit::Unique(FrameInfo {
                    state: Some(FrameState::Shared(frame, len)),
                    dirty: self.dirty,
                }),
                true,
            )),
            None => Err(ENOENT),
        }
    }

    fn leaf(&mut self, write: Option<usize>) -> Result<(Arsc<Frame>, usize), Error> {
        // log::trace!("leaf write = {write:?}");
        self.dirty |= write.is_some();
        match self.state.take() {
            Some(s) => {
                let (frame, mut len) = match s {
                    FrameState::Shared(frame, len) => (frame, len),
                    FrameState::Unique(frame, len) => (frame, len),
                };
                if let Some(new_len) = write {
                    len = len.max(new_len);
                }
                self.state = Some(FrameState::Shared(frame.clone(), len));
                Ok((frame, len))
            }
            None => match write {
                Some(new_len) => {
                    let frame = Arsc::new(Frame::new()?);
                    self.state = Some(FrameState::Shared(frame.clone(), new_len));
                    Ok((frame, new_len))
                }
                None => Ok((ZERO.clone(), 0)),
            },
        }
    }

    fn get(
        mut this: FrameEntry,
        branch: bool,
        write: Option<usize>,
        cow: bool,
    ) -> Result<Commit, Error> {
        if branch {
            let (ret, remove) = this.get_mut().branch(write, cow)?;
            if remove {
                this.remove();
            } else {
                this.insert();
            }
            Ok(ret)
        } else {
            let (frame, len) = this.get_mut().leaf(write)?;
            this.insert();
            Ok(Commit::Shared(frame, len))
        }
    }
}

#[derive(Clone)]
enum Parent {
    Phys {
        phys: Arsc<Phys>,
        start: usize,
        end: Option<usize>,
    },
    Backend(Arc<dyn Io>),
}

impl fmt::Debug for Parent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Parent::Phys { phys, start, end } => f
                .debug_struct("Phys")
                .field("phys", phys)
                .field("start", start)
                .field("end", end)
                .finish(),
            Parent::Backend(..) => f.debug_struct("Backend").finish_non_exhaustive(),
        }
    }
}

impl Parent {
    async fn stream_len(&self) -> Result<usize, Error> {
        match *self {
            Parent::Phys {
                ref phys, start, ..
            } => {
                let len = phys.stream_len().await?;
                Ok(len.saturating_sub(start))
            }
            Parent::Backend(ref b) => b.stream_len().await,
        }
    }
}

#[derive(Debug)]
struct FrameList {
    parent: Option<Parent>,
    frames: HashMap<usize, FrameInfo, RandomState>,
}

#[derive(Debug, Clone)]
struct Flusher {
    sender: Sender<SegQueue<FlushData>>,
    flushed: Arsc<Event>,
    offset: usize,
}

#[derive(Debug)]
pub struct Phys {
    branch: bool,
    list: Mutex<FrameList>,
    position: AtomicUsize,
    cow: bool,
    flusher: Option<Flusher>,
}

impl Phys {
    pub fn with_backend(
        backend: Arc<dyn Io>,
        initial_pos: usize,
        cow: bool,
    ) -> (Self, impl Future<Output = ()> + Send) {
        let (sender, receiver) = unbounded();
        let flushed = Arsc::new(Event::new());
        let phys = Phys {
            branch: false,
            list: Mutex::new(FrameList {
                parent: Some(Parent::Backend(backend.clone())),
                frames: Default::default(),
            }),
            position: initial_pos.into(),
            cow,
            flusher: (!cow).then_some(Flusher {
                sender,
                flushed: flushed.clone(),
                offset: 0,
            }),
        };
        (phys, flusher(receiver, flushed, backend))
    }

    pub fn new(cow: bool) -> Phys {
        Phys {
            branch: false,
            list: Mutex::new(FrameList {
                parent: None,
                frames: Default::default(),
            }),
            position: Default::default(),
            cow,
            flusher: None,
        }
    }

    pub fn clone_as(&self, cow: bool, index_offset: usize, fixed_count: Option<usize>) -> Self {
        self.merge_sole_parent(|_| Some(()));

        let branch = ksync::critical(|| {
            let mut list = self.list.lock();

            let branch = Arsc::new(Phys {
                branch: true,
                position: Default::default(),
                list: Mutex::new(FrameList {
                    parent: list.parent.clone(),
                    frames: mem::take(&mut list.frames),
                }),
                cow: self.cow || cow,
                flusher: None,
            });

            list.parent = Some(Parent::Phys {
                phys: branch.clone(),
                start: 0,
                end: None,
            });
            drop(list);
            branch
        });

        Phys {
            branch: false,
            list: Mutex::new(FrameList {
                parent: Some(Parent::Phys {
                    phys: branch,
                    start: index_offset,
                    end: fixed_count.map(|c| c + index_offset),
                }),
                frames: Default::default(),
            }),
            position: Default::default(),
            cow,
            flusher: self.flusher.clone().and_then(|flusher| {
                (!cow).then_some(Flusher {
                    offset: flusher.offset + index_offset,
                    ..flusher
                })
            }),
        }
    }

    pub fn is_cow(&self) -> bool {
        self.cow
    }
}

impl Phys {
    fn merge_sole_parent<R, F: FnMut(&mut FrameList) -> Option<R>>(&self, mut f: F) -> Option<R> {
        loop {
            let sole_parent = ksync::critical(|| {
                let mut list = self.list.lock();

                if let Some(parent) = list.parent.take() {
                    match parent {
                        Parent::Phys { phys, start, end } => match Arsc::try_unwrap(phys) {
                            Ok(phys) => return Some((phys, start, end)),
                            Err(phys) => list.parent = Some(Parent::Phys { phys, start, end }),
                        },
                        parent => list.parent = Some(parent),
                    }
                }
                None
            });

            match sole_parent {
                None => break None,
                Some((mut parent, start, end)) => {
                    // log::trace!("merging sole parent: start_index = {start}");
                    let ret = ksync::critical(|| {
                        let mut list = self.list.lock();

                        let frames = mem::take(&mut parent.list.get_mut().frames);
                        let iter = frames.into_iter().filter_map(|(pi, fi)| {
                            let index = pi.checked_sub(start)?;
                            end.map_or(true, |end| pi < end).then_some((index, fi))
                        });
                        for (index, mut fi) in iter {
                            fi.state = match mem::take(&mut fi.state) {
                                None => None,
                                Some(FrameState::Shared(frame, len))
                                | Some(FrameState::Unique(frame, len)) => {
                                    Some(FrameState::Shared(frame, len))
                                }
                            };
                            let _ = list.frames.try_insert(index, fi);
                        }
                        list.parent = parent.list.get_mut().parent.take().map(|pp| match pp {
                            Parent::Phys {
                                phys,
                                start: pp_start,
                                ..
                            } => Parent::Phys {
                                phys,
                                start: pp_start + start,
                                end: end.map(|e| pp_start + e),
                            },
                            Parent::Backend(b) => Parent::Backend(b),
                        });

                        f(&mut list)
                    });
                    if let Some(ret) = ret {
                        return Some(ret);
                    }
                }
            }
        }
    }

    fn commit_impl(
        &self,
        index: usize,
        write: Option<usize>,
        cow: bool,
    ) -> Boxed<Result<Commit, Error>> {
        let cow = self.cow || cow;
        Box::pin(async move {
            let self_get = ksync::critical(|| {
                // log::trace!("Phys::commit_impl: return from self, index = {index}");
                let mut list = self.list.lock();
                if let Some(ent) = FrameEntry::get(&mut list, index) {
                    return FrameInfo::get(ent, self.branch, write, cow).map(Some);
                }
                Ok::<_, Error>(None)
            })?;
            if let Some(commit) = self_get {
                return Ok(commit);
            }

            let self_get = self.merge_sole_parent(|list| {
                if let Some(ent) = FrameEntry::get(list, index) {
                    return Some(FrameInfo::get(ent, self.branch, write, cow));
                }
                None
            });
            if let Some(commit) = self_get.transpose()? {
                return Ok(commit);
            }

            if let Some(parent) = ksync::critical(|| self.list.lock().parent.clone()) {
                match parent {
                    Parent::Phys {
                        phys: parent,
                        start,
                        end,
                    } => {
                        if end.map_or(true, |end| (0..(end - start)).contains(&index)) {
                            let parent_index = start + index;
                            // log::trace!(
                            //     "Phys::commit_impl: return from parent, parent index = {}",
                            //     parent_index
                            // );
                            return match parent.commit_impl(parent_index, write, cow).await {
                                Ok(s @ Commit::Shared(..)) => Ok(s),
                                Ok(Commit::Unique(fi)) => ksync::critical(|| {
                                    let mut list = self.list.lock();

                                    let ent = FrameEntry::try_insert(&mut list, index, fi);
                                    FrameInfo::get(ent, self.branch, write, cow)
                                }),
                                Err(err) => Err(err),
                            };
                        }
                    }
                    Parent::Backend(backend) => {
                        // log::trace!(
                        //     "Phys::commit_impl: copy from backend, offset {:#x}",
                        //     index << PAGE_SHIFT
                        // );
                        let mut frame = Frame::new()?;

                        let len = {
                            let mut read_len = 0;
                            let mut offset = index << PAGE_SHIFT;
                            let mut buffer = &mut frame[..];
                            loop {
                                if buffer.is_empty() {
                                    break read_len;
                                }
                                let len = backend.read_at(offset, &mut [buffer]).await?;
                                if len == 0 {
                                    break read_len;
                                }
                                offset += len;
                                read_len += len;
                                buffer = &mut buffer[len..];
                            }
                        };
                        let fi = FrameInfo::new(Arsc::new(frame), len);
                        return ksync::critical(|| {
                            let mut list = self.list.lock();
                            let ent = FrameEntry::try_insert(&mut list, index, fi);
                            FrameInfo::get(ent, self.branch, write, cow)
                        });
                    }
                }
            }

            // log::trace!("Phys::commit_impl: return new frame");

            let Some(new_len) = write else {
                return Ok(Commit::Shared(ZERO.clone(), 0));
            };

            let fi = FrameInfo::new(Arsc::new(Frame::new()?), new_len);
            ksync::critical(|| {
                let mut list = self.list.lock();
                let ent = FrameEntry::try_insert(&mut list, index, fi);
                FrameInfo::get(ent, self.branch, write, cow)
            })
        })
    }

    pub async fn commit(
        &self,
        index: usize,
        writable: Option<usize>,
    ) -> Result<(Arsc<Frame>, usize), Error> {
        log::trace!(
            "Phys::commit index = {index} {writable:?}{}",
            if self.cow { " cow" } else { "" }
        );
        assert!(!self.branch);
        match self.commit_impl(index, writable, self.cow).await {
            Ok(Commit::Shared(frame, len)) => {
                log::trace!("Phys::commit result = {frame:?}, len = {len:#x}");
                Ok((frame, len))
            }
            Ok(Commit::Unique(..)) => unreachable!(),
            Err(err) => Err(err),
        }
    }

    pub fn resize(&self, new_len: usize) {
        if new_len == 0 {
            ksync::critical(|| {
                let mut list = self.list.lock();
                list.frames.clear();
                if let Some(Parent::Phys { start, end, .. }) = &mut list.parent {
                    *end = Some(*start)
                }
            });
            return;
        }

        let (index, offset) = {
            let index = new_len >> PAGE_SHIFT;
            let offset = new_len - (index << PAGE_SHIFT);
            if offset == 0 {
                (index - 1, PAGE_SIZE)
            } else {
                (index, offset)
            }
        };

        ksync::critical(|| {
            let mut list = self.list.lock();
            list.frames.retain(|&i, _| i <= index);
            if let Some(ent) = list.frames.get_mut(&index) {
                ent.truncate(offset);
            }
            if let Some(Parent::Phys { start, end, .. }) = &mut list.parent {
                *end = Some(*start + index + 1)
            }
        })
    }

    pub async fn flush(&self, mut index: usize, force_dirty: Option<bool>) -> Result<(), Error> {
        let Some(mut flusher) = self.flusher.clone() else {
            return Ok(())
        };

        self.merge_sole_parent(|_| Some(()));

        let mut storage = None;
        let mut this = self;

        loop {
            let data = ksync::critical(|| {
                let mut list = this.list.lock();
                list.frames.get_mut(&index).and_then(|fi| {
                    let dirty = mem::replace(&mut fi.dirty, false);
                    let dirty = force_dirty.unwrap_or(dirty);
                    dirty
                        .then(|| fi.state.as_mut().map(|s| s.frame(None)))
                        .flatten()
                })
            });

            if let Some((frame, len)) = data {
                let flushed = flusher.flushed.listen();

                let _ = flusher
                    .sender
                    .send(FlushData::Single((index + flusher.offset, frame, len)))
                    .await;

                flushed.await;

                break Ok(());
            }

            let parent = ksync::critical(|| this.list.lock().parent.clone());
            let Some(Parent::Phys { phys, start, end }) = parent else {
                break Ok(())
            };
            if Arsc::count(&phys) > 1 {
                break Ok(());
            }

            let Some(pi) = start.checked_add(index)
                .filter(|&i| i <= end.unwrap_or(usize::MAX))
             else {
                break Ok(())
            };

            flusher.offset -= start;
            index = pi;
            this = &**storage.insert(phys);
        }
    }

    pub async fn flush_all(&self) -> Result<(), Error> {
        // log::trace!("Phys::flush_all len = {}", self.position.load(SeqCst));
        let Some(mut flusher) = self.flusher.clone() else {
            return Ok(())
        };

        let mut storage = None;
        let mut this = self;
        let mut start_index = 0;
        let mut end_index = None;

        loop {
            let data = ksync::critical(|| {
                let mut list = this.list.lock();
                let iter = list.frames.iter_mut().filter_map(|(&index, fi)| {
                    if !(start_index <= index && end_index.map_or(true, |e| index < e)) {
                        return None;
                    }
                    let dirty = mem::replace(&mut fi.dirty, false);
                    dirty
                        .then(|| fi.state.as_mut().map(|s| s.frame(None)))
                        .flatten()
                        .map(|(frame, len)| (index + flusher.offset, frame, len))
                });
                iter.collect::<Vec<_>>()
            });
            if !data.is_empty() {
                let flushed = flusher.flushed.listen();

                let _ = flusher.sender.send(FlushData::Multiple(data)).await;

                flushed.await;
            }

            let parent = ksync::critical(|| this.list.lock().parent.clone());
            let Some(Parent::Phys { phys, start, end }) = parent else {
                break Ok(())
            };
            start_index += start;
            end_index = match (end_index, end) {
                (None, end) => end,
                (Some(end), None) => Some(end + start),
                (Some(end), Some(e)) => Some((end + start).min(e)),
            };

            flusher.offset -= start;
            this = &**storage.insert(phys);
        }
    }
}

impl Drop for Phys {
    fn drop(&mut self) {
        let Some(mut flusher) = self.flusher.clone() else {
            return;
        };

        let mut storage = None;
        let mut this = self;
        let mut start_index = 0;
        let mut end_index = None;

        loop {
            if flusher.sender.is_closed() {
                break;
            }
            let list = this.list.get_mut();
            let data = list.frames.iter_mut().filter_map(|(&index, fi)| {
                if !(start_index <= index && end_index.map_or(true, |e| index < e)) {
                    return None;
                }
                let dirty = mem::replace(&mut fi.dirty, false);
                dirty
                    .then(|| fi.state.as_mut().map(|s| s.frame(None)))
                    .flatten()
                    .map(|(frame, len)| (index + flusher.offset, frame, len))
            });
            let data = data.collect::<Vec<_>>();
            if !data.is_empty() {
                let _ = flusher.sender.try_send(FlushData::Multiple(data));
            }

            let Some(Parent::Phys { phys, start, end }) = list.parent.take() else {
                break
            };
            start_index += start;
            end_index = match (end_index, end) {
                (None, end) => end,
                (Some(end), None) => Some(end + start),
                (Some(end), Some(e)) => Some((end + start).min(e)),
            };

            flusher.offset -= start;
            let phys = storage.insert(phys);
            match Arsc::get_mut(phys) {
                Some(phys) => this = phys,
                None => break,
            }
        }
    }
}

#[async_trait]
impl Io for Phys {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        let pos = match whence {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(pos) => {
                let mut len = self.position.load(SeqCst);
                if let Some(parent) = ksync::critical(|| self.list.lock().parent.clone()) {
                    len = len.max(parent.stream_len().await?)
                }
                let pos = pos.checked_add(len.try_into()?);
                pos.ok_or(EINVAL)?.try_into()?
            }
            SeekFrom::Current(pos) => {
                let pos = pos.checked_add(self.position.load(SeqCst).try_into()?);
                pos.ok_or(EINVAL)?.try_into()?
            }
        };
        log::trace!("Phys::seek whence = {whence:?}, pos = {pos}");
        self.position.store(pos, SeqCst);
        Ok(pos)
    }

    async fn read_at(&self, offset: usize, mut buffer: &mut [IoSliceMut]) -> Result<usize, Error> {
        log::trace!(
            "Phys::read_at {offset:#x}, buffer len = {} {}",
            ioslice_len(&buffer),
            if self.cow { "cow" } else { "" }
        );

        let ioslice_len = ioslice_len(&buffer);
        let (start, end) = (offset, offset.checked_add(ioslice_len).ok_or(EINVAL)?);
        if start == end {
            return Ok(0);
        }

        let ((start_page, start_offset), (end_page, end_offset)) = offsets(start, end);

        if start_page == end_page {
            let (frame, end) = self.commit(start_page, None).await?;

            Ok(copy_from_frame(
                &mut buffer,
                &frame,
                start_offset,
                end_offset.min(end),
            ))
        } else {
            let mut read_len = 0;
            {
                let (frame, end) = self.commit(start_page, None).await?;
                read_len += copy_from_frame(&mut buffer, &frame, start_offset, end);
                if end < PAGE_SIZE || buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let (frame, end) = self.commit(index, None).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, end);
                if end < PAGE_SIZE || buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            {
                let (frame, end) = self.commit(end_page, None).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, end_offset.min(end));
            }

            Ok(read_len)
        }
    }

    async fn write_at(&self, offset: usize, mut buffer: &mut [IoSlice]) -> Result<usize, Error> {
        log::trace!(
            "Phys::write_at {offset:#x}, buffer len = {} {}",
            ioslice_len(&buffer),
            if self.cow { "cow" } else { "" }
        );

        let ioslice_len = ioslice_len(&buffer);
        let (start, end) = (offset, offset.checked_add(ioslice_len).ok_or(EINVAL)?);
        if start == end {
            return Ok(0);
        }

        let ((start_page, start_offset), (end_page, end_offset)) = offsets(start, end);

        if start_page == end_page {
            let (frame, _) = self.commit(start_page, Some(end_offset)).await?;

            Ok(copy_to_frame(&mut buffer, &frame, start_offset, end_offset))
        } else {
            let mut written_len = 0;
            {
                let (frame, _) = self.commit(start_page, Some(PAGE_SIZE)).await?;
                let len = copy_to_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let (frame, _) = self.commit(index, Some(PAGE_SIZE)).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            {
                let (frame, _) = self.commit(end_page, Some(end_offset)).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, end_offset);
                written_len += len;
            }

            Ok(written_len)
        }
    }

    async fn flush(&self) -> Result<(), Error> {
        self.flush_all().await
    }
}

fn offsets(start: usize, end: usize) -> ((usize, usize), (usize, usize)) {
    let start_page = start >> PAGE_SHIFT;
    let start_offset = start - (start_page << PAGE_SHIFT);

    let (end_page, end_offset) = {
        let end_page = end >> PAGE_SHIFT;
        let end_offset = end - (end_page << PAGE_SHIFT);
        if end_offset == 0 {
            (end_page - 1, PAGE_SIZE)
        } else {
            (end_page, end_offset)
        }
    };

    ((start_page, start_offset), (end_page, end_offset))
}

fn copy_from_frame(
    buffer: &mut &mut [IoSliceMut],
    frame: &Frame,
    mut start: usize,
    end: usize,
) -> usize {
    let mut read_len = 0;
    loop {
        if buffer.is_empty() || end == start {
            break read_len;
        }
        let buf = &mut buffer[0];
        if buf.is_empty() {
            *buffer = &mut mem::take(buffer)[1..];
            continue;
        }
        let len = buf.len().min(end - start);
        buf[..len].copy_from_slice(&frame[start..][..len]);

        read_len += len;
        start += len;
        advance_slices(buffer, len);
    }
}

fn copy_to_frame(
    buffer: &mut &mut [IoSlice],
    frame: &Frame,
    mut start: usize,
    end: usize,
) -> usize {
    let mut written_len = 0;
    loop {
        if buffer.is_empty() || end == start {
            break written_len;
        }
        let buf = buffer[0];
        if buf.is_empty() {
            *buffer = &mut mem::take(buffer)[1..];
            continue;
        }
        let len = buf.len().min(end - start);
        unsafe {
            let mut src = frame.as_ptr();
            src.as_mut()[start..][..len].copy_from_slice(&buf[..len])
        }
        written_len += len;
        start += len;
        advance_slices(buffer, len);
    }
}

enum FlushData {
    Single((usize, Arsc<Frame>, usize)),
    Multiple(Vec<(usize, Arsc<Frame>, usize)>),
}

async fn flusher(rx: Receiver<SegQueue<FlushData>>, flushed: Arsc<Event>, backend: Arc<dyn Io>) {
    loop {
        let Ok(data) = rx.recv().await else { break };
        match data {
            FlushData::Single((index, frame, len)) => {
                let _ = backend
                    .write_all_at(index << PAGE_SHIFT, &frame[..len])
                    .await;
            }
            FlushData::Multiple(data) => {
                for (index, frame, len) in data {
                    let _ = backend
                        .write_all_at(index << PAGE_SHIFT, &frame[..len])
                        .await;
                }
            }
        }
        // log::trace!("BACKEND flush");
        let _ = backend.flush().await;
        flushed.notify_additional(1);
    }
}

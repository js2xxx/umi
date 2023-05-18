use alloc::{boxed::Box, sync::Arc};
use core::{
    borrow::Borrow,
    fmt, mem,
    num::NonZeroUsize,
    ptr::NonNull,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

use async_trait::async_trait;
use hashbrown::{hash_map::Entry, HashMap};
use ksc_core::{
    handler::Boxed,
    Error::{self, EINVAL, ENOENT, ENOMEM},
};
use ksync::Mutex;
use rand_riscv::RandomState;
use rv39_paging::{PAddr, ID_OFFSET, PAGE_SHIFT, PAGE_SIZE};
use spin::Lazy;
use umio::{advance_slices, ioslice_len, Io, IoSlice, IoSliceMut, SeekFrom};

static ZERO: Lazy<Arc<Frame>> = Lazy::new(|| Arc::new(Frame::new().unwrap()));

#[derive(Debug)]
pub struct Frame {
    base: PAddr,
    ptr: NonNull<u8>,
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
        f.as_mut_slice()[..len].copy_from_slice(&self.as_slice()[..len]);
        Ok(f)
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
    Shared(Arc<Frame>, usize),
    Unique(Arc<Frame>, usize),
}

impl FrameState {
    fn frame(&mut self, write: Option<usize>) -> (Arc<Frame>, usize) {
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
    Shared(Arc<Frame>, usize),
    Unique(FrameInfo),
}

#[derive(Debug)]
struct FrameInfo {
    state: Option<FrameState>,
    dirty: bool,
    pin: usize,
}

impl FrameInfo {
    fn new(frame: Arc<Frame>, len: usize, dirty: bool, pin: bool) -> Self {
        FrameInfo {
            state: Some(FrameState::Shared(frame, len)),
            dirty,
            pin: pin as usize,
        }
    }

    fn branch(
        &mut self,
        write: Option<usize>,
        pin: bool,
        cow: bool,
    ) -> Result<(Commit, bool), Error> {
        match mem::take(&mut self.state) {
            Some(FrameState::Shared(frame, len)) => match write {
                None => {
                    self.state = Some(FrameState::Shared(frame.clone(), len));
                    self.pin += pin as usize;
                    Ok((Commit::Shared(frame, len), false))
                }
                Some(new_len) if !cow => {
                    let len = len.max(new_len);
                    self.state = Some(FrameState::Shared(frame.clone(), len));
                    self.pin += pin as usize;
                    Ok((Commit::Shared(frame, len), false))
                }
                Some(new_len) => {
                    let new_len = len.max(new_len);
                    let new_frame = frame.copy(new_len)?;
                    self.state = Some(FrameState::Unique(frame, new_len));
                    Ok((
                        Commit::Unique(FrameInfo {
                            state: Some(FrameState::Shared(Arc::new(new_frame), new_len)),
                            dirty: self.dirty,
                            pin: pin as usize,
                        }),
                        false,
                    ))
                }
            },
            Some(FrameState::Unique(frame, len)) => Ok((
                Commit::Unique(FrameInfo {
                    state: Some(FrameState::Shared(frame, len)),
                    dirty: self.dirty,
                    pin: self.pin + pin as usize,
                }),
                true,
            )),
            None => Err(ENOENT),
        }
    }

    fn leaf(&mut self, write: Option<usize>, pin: bool) -> Result<(Arc<Frame>, usize), Error> {
        self.dirty |= write.is_some();
        self.pin += pin as usize;
        match &mut self.state {
            Some(s) => Ok(s.frame(write)),
            None => match write {
                Some(new_len) => {
                    let frame = Arc::new(Frame::new()?);
                    self.state = Some(FrameState::Shared(frame.clone(), new_len));
                    Ok((frame, new_len))
                }
                None => Ok((ZERO.clone(), 0)),
            },
        }
    }
}

#[derive(Clone)]
enum Parent {
    Phys {
        phys: Arc<Phys>,
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
    branch: bool,
    parent: Option<Parent>,
    frames: HashMap<usize, FrameInfo, RandomState>,
}

impl FrameList {
    fn commit_impl(
        &mut self,
        index: usize,
        write: Option<usize>,
        pin: bool,
        cow: bool,
    ) -> Boxed<Result<Commit, Error>> {
        Box::pin(async move {
            let entry = match self.frames.entry(index) {
                Entry::Occupied(mut ent) => {
                    return Ok(if self.branch {
                        let (ret, remove) = ent.get_mut().branch(write, pin, cow)?;
                        if remove {
                            ent.remove();
                        }
                        ret
                    } else {
                        let (frame, len) = ent.get_mut().leaf(write, pin)?;
                        Commit::Shared(frame, len)
                    })
                }
                Entry::Vacant(ent) => ent,
            };

            if let Some(ref parent) = self.parent {
                match parent {
                    Parent::Phys { phys, start, end } => {
                        let mut list = phys.list.lock().await;
                        if end.map_or(true, |end| (0..(end - start)).contains(&index)) {
                            let parent_index = start + index;
                            let cow = cow || phys.cow;
                            return match list.commit_impl(parent_index, write, pin, cow).await {
                                Ok(s @ Commit::Shared(..)) => Ok(s),
                                Ok(Commit::Unique(mut fi)) => Ok(if self.branch {
                                    Commit::Unique(fi)
                                } else {
                                    let (frame, len) = fi.state.as_mut().unwrap().frame(None);
                                    entry.insert(fi);
                                    Commit::Shared(frame, len)
                                }),
                                Err(err) => Err(err),
                            };
                        }
                    }
                    Parent::Backend(backend) => {
                        let mut frame = Frame::new()?;

                        let len = {
                            let mut read_len = 0;
                            let mut offset = index << PAGE_SHIFT;
                            let mut buffer = frame.as_mut_slice();
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
                        let frame = Arc::new(frame);
                        entry.insert(FrameInfo::new(frame.clone(), len, write.is_some(), pin));
                        return Ok(Commit::Shared(frame, len));
                    }
                }
            }

            let Some(new_len) = write else {
                return Ok(Commit::Shared(ZERO.clone(), 0));
            };

            let frame = Arc::new(Frame::new()?);
            let fi = FrameInfo::new(frame.clone(), new_len, write.is_some(), pin);
            Ok(if self.branch {
                Commit::Unique(fi)
            } else {
                entry.insert(fi);
                Commit::Shared(frame, new_len)
            })
        })
    }

    async fn commit(
        &mut self,
        index: usize,
        write: Option<usize>,
        pin: bool,
        cow: bool,
    ) -> Result<(Arc<Frame>, usize), Error> {
        assert!(!self.branch);
        match self.commit_impl(index, write, pin, cow).await {
            Ok(Commit::Shared(frame, len)) => Ok((frame, len)),
            Ok(Commit::Unique(..)) => unreachable!(),
            Err(err) => Err(err),
        }
    }

    fn clone_as(&mut self, cow: bool, start: usize, end: Option<usize>) -> Phys {
        let branch = Arc::new(Phys {
            position: Default::default(),
            list: Mutex::new(FrameList {
                branch: true,
                parent: self.parent.clone(),
                frames: mem::take(&mut self.frames),
            }),
            cow: false,
        });

        self.parent = Some(Parent::Phys {
            phys: branch.clone(),
            start: 0,
            end: None,
        });

        Phys {
            position: Default::default(),
            list: Mutex::new(FrameList {
                branch: false,
                parent: Some(Parent::Phys {
                    phys: branch,
                    start,
                    end,
                }),
                frames: Default::default(),
            }),
            cow,
        }
    }
}

#[derive(Debug)]
pub struct Phys {
    list: Mutex<FrameList>,
    position: AtomicUsize,
    cow: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CreateSub {
    pub index_offset: usize,
    pub fixed_count: Option<usize>,
}

impl Phys {
    pub fn new(backend: Arc<dyn Io>, initial_pos: usize, cow: bool) -> Self {
        Phys {
            list: Mutex::new(FrameList {
                branch: false,
                parent: Some(Parent::Backend(backend)),
                frames: Default::default(),
            }),
            position: initial_pos.into(),
            cow,
        }
    }

    pub fn new_anon(cow: bool) -> Phys {
        Phys {
            list: Mutex::new(FrameList {
                branch: false,
                parent: None,
                frames: Default::default(),
            }),
            position: Default::default(),
            cow,
        }
    }

    pub async fn clone_as(self: &Arc<Self>, cow: bool, create_sub: Option<CreateSub>) -> Self {
        let (start, end) = create_sub.map_or((0, None), |cs| {
            (cs.index_offset, cs.fixed_count.map(|c| c + cs.index_offset))
        });
        self.list.lock().await.clone_as(!cow, start, end)
    }

    pub fn is_cow(&self) -> bool {
        self.cow
    }
}

impl Phys {
    pub async fn commit(
        &self,
        index: usize,
        writable: Option<usize>,
        pin: bool,
    ) -> Result<(Arc<Frame>, usize), Error> {
        log::trace!(
            "Phys::commit index = {index} {writable:?} {} {}",
            if pin { "pin" } else { "" },
            if self.cow { "cow" } else { "" }
        );
        let mut list = self.list.lock().await;
        list.commit(index, writable, pin, self.cow).await
    }

    pub async fn flush(
        &self,
        _index: usize,
        _force_dirty: Option<bool>,
        _unpin: bool,
    ) -> Result<(), Error> {
        // TODO: flush.
        Ok(())
    }

    pub async fn flush_all(&self) -> Result<(), Error> {
        // TODO: flush all.
        Ok(())
    }
}

#[async_trait]
impl Io for Phys {
    async fn seek(&self, whence: SeekFrom) -> Result<usize, Error> {
        let pos = match whence {
            SeekFrom::Start(pos) => pos,
            SeekFrom::End(pos) => {
                let mut len = self.position.load(SeqCst);
                if let Some(ref parent) = self.list.lock().await.parent {
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
            let (frame, end) = self.commit(start_page, None, false).await?;

            Ok(copy_from_frame(
                &mut buffer,
                &frame,
                start_offset,
                end_offset.min(end),
            ))
        } else {
            let mut read_len = 0;
            {
                let (frame, end) = self.commit(start_page, None, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, start_offset, end);
                if end < PAGE_SIZE || buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let (frame, end) = self.commit(index, None, false).await?;
                read_len += copy_from_frame(&mut buffer, &frame, 0, end);
                if end < PAGE_SIZE || buffer.is_empty() {
                    return Ok(read_len);
                }
            }
            {
                let (frame, end) = self.commit(end_page, None, false).await?;
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
            let (frame, _) = self.commit(start_page, Some(end_offset), false).await?;

            Ok(copy_to_frame(&mut buffer, &frame, start_offset, end_offset))
        } else {
            let mut written_len = 0;
            {
                let (frame, _) = self.commit(start_page, Some(PAGE_SIZE), false).await?;
                let len = copy_to_frame(&mut buffer, &frame, start_offset, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            for index in (start_page + 1)..end_page {
                let (frame, _) = self.commit(index, Some(PAGE_SIZE), false).await?;
                let len = copy_to_frame(&mut buffer, &frame, 0, PAGE_SIZE);
                written_len += len;
                if buffer.is_empty() {
                    return Ok(written_len);
                }
            }
            {
                let (frame, _) = self.commit(end_page, Some(end_offset), false).await?;
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
        if buffer.is_empty() {
            break read_len;
        }
        let buf = &mut buffer[0];
        let len = buf.len().min(end - start);
        if len == 0 {
            break read_len;
        }
        unsafe {
            let src = frame.as_ptr();
            buf[..len].copy_from_slice(&src.as_ref()[start..][..len]);
        }
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
        if buffer.is_empty() {
            break written_len;
        }
        let buf = buffer[0];
        let len = buf.len().min(end - start);
        if len == 0 {
            break written_len;
        }
        unsafe {
            let mut src = frame.as_ptr();
            src.as_mut()[start..][..len].copy_from_slice(&buf[..len])
        }
        written_len += len;
        start += len;
        advance_slices(buffer, len);
    }
}

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{cmp, iter, num, pin::pin, slice, str};

use async_trait::async_trait;
use futures_util::{stream, Stream, StreamExt};
use ksc_core::Error::{self, EEXIST, EINVAL, EIO, EISDIR, ENOENT, ENOSYS, ENOTDIR, ENOTEMPTY};
use umifs::{
    path::Path,
    traits::{Directory, DirectoryMut, Entry, Io, IoExt},
    types::{FileType, Metadata, OpenOptions, Permissions},
};
use umio::{IoPoll, IoSlice, IoSliceMut, SeekFrom};

use crate::{
    dirent::{
        DirEntryData, DirFileEntryData, DirLfnEntryData, FileAttributes, ShortName, DIR_ENTRY_SIZE,
        LFN_ENTRY_LAST_FLAG, LFN_PART_LEN, SFN_PADDING, SFN_SIZE,
    },
    DirEntry, FatFile, TimeProvider,
};

enum DirEntryOrShortName<T: TimeProvider> {
    DirEntry(DirEntry<T>),
    ShortName([u8; SFN_SIZE]),
}

pub struct FatDir<T: TimeProvider> {
    file: FatFile<T>,
}

impl<T: TimeProvider> FatDir<T> {
    pub(crate) fn new(file: FatFile<T>) -> Self {
        FatDir { file }
    }

    fn should_skip_entry(&self, raw_entry: &DirEntryData, skip_volume: bool) -> bool {
        if raw_entry.is_deleted() {
            return true;
        }
        match raw_entry {
            DirEntryData::File(sfn_entry) => skip_volume && sfn_entry.is_volume(),
            DirEntryData::Lfn(_) => false,
        }
    }

    pub async fn next_dirent(
        &self,
        last_pos: Option<u64>,
        skip_volume: bool,
    ) -> Result<Option<DirEntry<T>>, Error> {
        let abs_start_pos = self.file.abs_start_pos().await.unwrap();

        let mut lfn_builder = LongNameBuilder::new();
        let mut offset = match last_pos {
            Some(last) => (last - abs_start_pos) as usize + DIR_ENTRY_SIZE as usize,
            None => 0,
        };
        let mut begin_offset = offset;
        loop {
            let mut buf = [0; DIR_ENTRY_SIZE as usize];
            if let Err(err) = self.file.read_exact_at(offset, &mut buf).await {
                return if err == EIO { Ok(None) } else { Err(err) };
            }
            let (_, raw_entry) = DirEntryData::parse(&buf)?;
            if raw_entry.is_end() {
                return Ok(None);
            }
            if self.should_skip_entry(&raw_entry, skip_volume) {
                lfn_builder.clear();
                offset += DIR_ENTRY_SIZE as usize;
                begin_offset = offset;
                continue;
            }
            match raw_entry {
                DirEntryData::File(data) => {
                    let abs_pos = abs_start_pos + offset as u64;
                    lfn_builder.validate_chksum(data.name());
                    let short_name = ShortName::new(data.name());
                    break Ok(Some(DirEntry {
                        data,
                        short_name,
                        lfn_utf16: lfn_builder.into_buf(),
                        entry_pos: abs_pos,
                        offset_range: begin_offset as u64
                            ..(offset as u64 + u64::from(DIR_ENTRY_SIZE)),
                        fs: self.file.fs.clone(),
                    }));
                }
                DirEntryData::Lfn(lfn) => lfn_builder.process(&lfn),
            }
            offset += DIR_ENTRY_SIZE as usize;
        }
    }

    pub fn iter(
        &self,
        skip_volume: bool,
    ) -> impl Stream<Item = Result<DirEntry<T>, Error>> + Send + '_ {
        stream::unfold((self, None), move |(this, last_pos)| async move {
            let ret = this.next_dirent(last_pos, skip_volume).await;
            ret.transpose().map(|ret| {
                let last_pos = ret.as_ref().ok().map(|d| d.entry_pos);
                (ret, (this, last_pos))
            })
        })
    }
}

impl<T: TimeProvider> FatDir<T> {
    async fn find_entry(
        &self,
        name: &str,
        is_dir: Option<bool>,
        mut short_name_gen: Option<&mut ShortNameGenerator>,
    ) -> Result<DirEntry<T>, Error> {
        let mut iter = pin!(self.iter(true));
        while let Some(r) = iter.next().await {
            let e = r?;
            // compare name ignoring case
            if e.eq_name(name) {
                // check if file or directory is expected
                if is_dir.is_some() && Some(e.is_dir()) != is_dir {
                    return if e.is_dir() {
                        Err(EISDIR)
                    } else {
                        Err(ENOTDIR)
                    };
                }
                return Ok(e);
            }
            // update short name generator state
            if let Some(ref mut gen) = short_name_gen {
                gen.add_existing(e.raw_short_name());
            }
        }
        Err(ENOENT)
    }

    async fn check_for_existence(
        &self,
        name: &str,
        is_dir: Option<bool>,
    ) -> Result<DirEntryOrShortName<T>, Error> {
        let mut short_name_gen = ShortNameGenerator::new(name);
        loop {
            // find matching entry
            let r = self
                .find_entry(name, is_dir, Some(&mut short_name_gen))
                .await;
            match r {
                // file not found - continue with short name generation
                Err(ENOENT) => {}
                // unexpected error - return it
                Err(err) => return Err(err),
                // directory already exists - return it
                Ok(e) => return Ok(DirEntryOrShortName::DirEntry(e)),
            };
            // try to generate short name
            if let Ok(name) = short_name_gen.generate() {
                return Ok(DirEntryOrShortName::ShortName(name));
            }
            // there were too many collisions in short name generation
            // try different checksum in the next iteration
            short_name_gen.next_iteration();
        }
    }

    pub async fn open(&self, path: &Path) -> Result<DirEntry<T>, Error> {
        let mut storage: Option<Self> = None;
        let mut node = self;

        let mut comps = path.components().peekable();
        while let Some(comp) = comps.next() {
            if comps.peek().is_some() {
                let e = node.find_entry(comp.as_str(), Some(true), None).await?;
                node = storage.insert(e.to_dir().await?);
            } else {
                let e = node.find_entry(comp.as_str(), None, None).await?;
                return Ok(e);
            }
        }
        Err(EINVAL)
    }

    pub async fn open_file(&self, path: &Path) -> Result<FatFile<T>, Error> {
        let mut storage: Option<Self> = None;
        let mut node = self;

        let mut comps = path.components().peekable();
        while let Some(comp) = comps.next() {
            if comps.peek().is_some() {
                let e = node.find_entry(comp.as_str(), Some(true), None).await?;
                node = storage.insert(e.to_dir().await?);
            } else {
                let e = node.find_entry(comp.as_str(), Some(false), None).await?;
                return e.to_file().await;
            }
        }
        Err(EINVAL)
    }

    pub async fn open_dir(&self, path: &Path) -> Result<Self, Error> {
        let mut storage: Option<Self> = None;
        let mut node = self;

        for comp in path.components() {
            let e = node.find_entry(comp.as_str(), Some(true), None).await?;
            node = storage.insert(e.to_dir().await?);
        }
        storage.ok_or(EINVAL)
    }

    pub async fn create_file(&self, path: &Path) -> Result<(FatFile<T>, bool), Error> {
        let mut storage: Option<Self> = None;
        let mut node = self;

        let mut comps = path.components().peekable();
        while let Some(comp) = comps.next() {
            if comps.peek().is_some() {
                let e = node.find_entry(comp.as_str(), Some(true), None).await?;
                node = storage.insert(e.to_dir().await?);
            } else {
                let name = comp.as_str();
                let r = node.check_for_existence(name, Some(false)).await?;
                return match r {
                    // file does not exist - create it
                    DirEntryOrShortName::ShortName(short_name) => {
                        let sfn_entry = node.create_sfn_entry(
                            short_name,
                            FileAttributes::from_bits_truncate(0),
                            None,
                        );
                        Ok((
                            node.write_entry(name, sfn_entry).await?.to_file().await?,
                            true,
                        ))
                    }
                    // file already exists - return it
                    DirEntryOrShortName::DirEntry(e) => Ok((e.to_file().await?, false)),
                };
            }
        }
        Err(EINVAL)
    }

    pub async fn create_dir(&self, path: &Path) -> Result<(FatDir<T>, bool), Error> {
        let mut storage: Option<Self> = None;
        let mut node = self;

        let mut comps = path.components().peekable();
        while let Some(comp) = comps.next() {
            if comps.peek().is_some() {
                let e = node.find_entry(comp.as_str(), Some(true), None).await?;
                node = storage.insert(e.to_dir().await?);
            } else {
                let name = comp.as_str();
                let r = node.check_for_existence(name, Some(false)).await?;
                return match r {
                    // directory does not exist - create it
                    DirEntryOrShortName::ShortName(short_name) => {
                        // alloc cluster for directory data
                        let cluster = node.file.fs.alloc_cluster(None, true).await?;
                        // create entry in parent directory
                        let sfn_entry = node.create_sfn_entry(
                            short_name,
                            FileAttributes::DIRECTORY,
                            Some(cluster),
                        );
                        let entry = node.write_entry(name, sfn_entry).await?;
                        let dir = entry.to_dir().await?;
                        // create special entries "." and ".."
                        let dot_sfn = ShortNameGenerator::generate_dot();
                        let sfn_entry = node.create_sfn_entry(
                            dot_sfn,
                            FileAttributes::DIRECTORY,
                            entry.first_cluster(),
                        );
                        dir.write_entry(".", sfn_entry).await?;
                        let dotdot_sfn = ShortNameGenerator::generate_dotdot();
                        let sfn_entry = node.create_sfn_entry(
                            dotdot_sfn,
                            FileAttributes::DIRECTORY,
                            node.file.first_cluster().await,
                        );
                        dir.write_entry("..", sfn_entry).await?;
                        Ok((dir, true))
                    }
                    // directory already exists - return it
                    DirEntryOrShortName::DirEntry(e) => Ok((e.to_dir().await?, false)),
                };
            }
        }
        Err(EINVAL)
    }

    pub async fn remove(&self, path: &Path, is_dir: Option<bool>) -> Result<(), Error> {
        let mut storage: Option<Self> = None;
        let mut node = self;

        let mut comps = path.components().peekable();
        while let Some(comp) = comps.next() {
            if comps.peek().is_some() {
                let e = node.find_entry(comp.as_str(), Some(true), None).await?;
                node = storage.insert(e.to_dir().await?);
            } else {
                let name = comp.as_str();
                let e = node.find_entry(name, None, None).await?;
                if is_dir.is_some() && Some(e.is_dir()) != is_dir {
                    return if e.is_dir() {
                        Err(EISDIR)
                    } else {
                        Err(ENOTDIR)
                    };
                }

                if e.is_dir() && !e.to_dir().await?.is_empty().await? {
                    return Err(ENOTEMPTY);
                }
                // free data
                if let Some(n) = e.first_cluster() {
                    node.file.fs.fat.free(n).await?;
                }
                // free long and short name entries
                e.free_all_entries(&self.file).await?;

                return Ok(());
            }
        }
        Err(EINVAL)
    }

    pub async fn rename(
        &self,
        src_path: &Path,
        dst_dir: &FatDir<T>,
        dst_path: &Path,
    ) -> Result<(), Error> {
        let mut src_storage: Option<Self> = None;
        let mut dst_storage: Option<Self> = None;
        let mut src_node = self;
        let mut dst_node = dst_dir;

        let mut comps = src_path.components().peekable();
        let src_name = loop {
            let Some(comp) = comps.next() else { return Err(EINVAL) };
            if comps.peek().is_none() {
                break comp.as_str();
            }
            let e = src_node.find_entry(comp.as_str(), Some(true), None).await?;
            src_node = src_storage.insert(e.to_dir().await?);
        };

        let mut comps = dst_path.components().peekable();
        let dst_name = loop {
            let Some(comp) = comps.next() else { return Err(EINVAL) };
            if comps.peek().is_none() {
                break comp.as_str();
            }
            let e = dst_node.find_entry(comp.as_str(), Some(true), None).await?;
            dst_node = dst_storage.insert(e.to_dir().await?);
        };

        self.rename_internal(src_name, dst_dir, dst_name).await
    }

    async fn rename_internal(
        &self,
        src_name: &str,
        dst_dir: &FatDir<T>,
        dst_name: &str,
    ) -> Result<(), Error> {
        // find existing file
        let e = self.find_entry(src_name, None, None).await?;
        // check if destionation filename is unused
        let r = dst_dir.check_for_existence(dst_name, None).await?;
        let short_name = match r {
            // destination file already exist
            DirEntryOrShortName::DirEntry(ref dst_e) => {
                // check if source and destination entry is the same
                if e.is_same_entry(dst_e) {
                    // nothing to do
                    return Ok(());
                }
                // destination file exists and it is not the same as source file - fail
                return Err(EEXIST);
            }
            // destionation file does not exist, short name has been generated
            DirEntryOrShortName::ShortName(short_name) => short_name,
        };
        // free long and short name entries
        e.free_all_entries(&self.file).await?;
        // save new directory entry
        let sfn_entry = e.data.renamed(short_name);
        dst_dir.write_entry(dst_name, sfn_entry).await?;
        Ok(())
    }
}

#[async_trait]
impl<T: TimeProvider> Io for FatDir<T> {
    async fn seek(&self, _: SeekFrom) -> Result<usize, Error> {
        Err(EISDIR)
    }

    async fn read_at(&self, _: usize, _: &mut [IoSliceMut]) -> Result<usize, Error> {
        Err(EISDIR)
    }

    async fn write_at(&self, _: usize, _: &mut [IoSlice]) -> Result<usize, Error> {
        Err(EISDIR)
    }

    async fn flush(&self) -> Result<(), Error> {
        self.file.flush().await
    }
}

#[async_trait]
impl<T: TimeProvider> Entry for FatDir<T> {
    async fn open(
        self: Arc<Self>,
        path: &Path,
        options: OpenOptions,
        _perm: Permissions,
    ) -> Result<(Arc<dyn Entry>, bool), Error> {
        if path == "" || path == "." {
            return if options.contains(OpenOptions::CREAT | OpenOptions::EXCL) {
                Err(EEXIST)
            } else {
                Ok((self, false))
            };
        }
        Ok(
            match (
                options.contains(OpenOptions::CREAT),
                options.contains(OpenOptions::DIRECTORY),
            ) {
                (false, false) => {
                    let dirent = (*self).open(path).await?;
                    (
                        if dirent.is_dir() {
                            Arc::new(dirent.to_dir().await?)
                        } else {
                            Arc::new(dirent.to_file().await?)
                        },
                        false,
                    )
                }
                (false, true) => (Arc::new(self.open_dir(path).await?), false),
                (true, false) => {
                    let (file, created) = self.create_file(path).await?;
                    // if !created && options.contains(OpenOptions::EXCL) {
                    //     return Err(EEXIST);
                    // }
                    (Arc::new(file), created)
                }
                (true, true) => {
                    let (dir, created) = self.create_dir(path).await?;
                    // if !created && options.contains(OpenOptions::EXCL) {
                    //     return Err(EEXIST);
                    // }
                    (Arc::new(dir), created)
                }
            },
        )
    }

    async fn metadata(&self) -> Metadata {
        Metadata {
            ty: FileType::DIR,
            ..self.file.metadata().await
        }
    }

    fn to_dir(self: Arc<Self>) -> Option<Arc<dyn Directory>> {
        Some(self as _)
    }

    fn to_dir_mut(self: Arc<Self>) -> Option<Arc<dyn DirectoryMut>> {
        Some(self as _)
    }
}
impl<T: TimeProvider> IoPoll for FatDir<T> {}

#[async_trait]
impl<T: TimeProvider> Directory for FatDir<T> {
    async fn next_dirent(
        &self,
        last: Option<&umifs::types::DirEntry>,
    ) -> Result<Option<umifs::types::DirEntry>, Error> {
        let last = last.map(|last| last.metadata.offset);
        let dirent = self.next_dirent(last, true).await?;

        let fm = self.file.metadata().await;
        Ok(dirent.map(|d| umifs::types::DirEntry {
            name: d.file_name(),
            metadata: Metadata {
                ty: if d.is_dir() {
                    FileType::DIR
                } else {
                    FileType::FILE
                },
                len: d.len() as usize,
                offset: d.entry_pos,
                perm: Permissions::all(),
                block_size: fm.block_size,
                block_count: fm.block_count,
                last_access: None,
                last_modified: None,
                last_created: None,
            },
        }))
    }
}

#[async_trait]
impl<T: TimeProvider> DirectoryMut for FatDir<T> {
    async fn rename(
        self: Arc<Self>,
        src_path: &Path,
        dst_parent: Arc<dyn DirectoryMut>,
        dst_path: &Path,
    ) -> Result<(), Error> {
        let Ok(dst_parent) = dst_parent.into_any().downcast::<Self>() else {
            return Err(ENOSYS)
        };
        (*self).rename(src_path, &dst_parent, dst_path).await
    }

    async fn link(
        self: Arc<Self>,
        _: &Path,
        _: Arc<dyn DirectoryMut>,
        _: &Path,
    ) -> Result<(), Error> {
        Err(ENOSYS)
    }

    async fn unlink(&self, path: &Path, expect_dir: Option<bool>) -> Result<(), Error> {
        self.remove(path, expect_dir).await
    }
}

impl<T: TimeProvider> FatDir<T> {
    async fn find_free_entries(&self, num_entries: u32) -> Result<u64, Error> {
        let mut first_free: u32 = 0;
        let mut num_free: u32 = 0;
        let mut i: u32 = 0;
        loop {
            let mut buf = [0; DIR_ENTRY_SIZE as usize];
            let len = self
                .file
                .read_at((i * DIR_ENTRY_SIZE) as usize, &mut [&mut buf])
                .await?;
            if len < DIR_ENTRY_SIZE as usize {
                // first unused entry at the end - all remaining space can be used
                if num_free == 0 {
                    first_free = i;
                }
                return Ok(u64::from(first_free * DIR_ENTRY_SIZE));
            }

            let (_, raw_entry) = DirEntryData::parse(&buf)?;
            if raw_entry.is_end() {
                // first unused entry - all remaining space can be used
                if num_free == 0 {
                    first_free = i;
                }
                return Ok(u64::from(first_free * DIR_ENTRY_SIZE));
            } else if raw_entry.is_deleted() {
                // free entry - calculate number of free entries in a row
                if num_free == 0 {
                    first_free = i;
                }
                num_free += 1;
                if num_free == num_entries {
                    // enough space for new file
                    return Ok(u64::from(first_free * DIR_ENTRY_SIZE));
                }
            } else {
                // used entry - start counting from 0
                num_free = 0;
            }
            i += 1;
        }
    }

    fn create_sfn_entry(
        &self,
        short_name: [u8; SFN_SIZE],
        attrs: FileAttributes,
        first_cluster: Option<u32>,
    ) -> DirFileEntryData {
        let mut raw_entry = DirFileEntryData::new(short_name, attrs);
        raw_entry.set_first_cluster(first_cluster);
        let now = self.file.fs.time_provider.get_current_date_time();
        raw_entry.set_created(now);
        raw_entry.set_accessed(now.date);
        raw_entry.set_modified(now);
        raw_entry
    }

    async fn alloc_and_write_lfn_entries(
        &self,
        lfn_utf16: &LfnBuffer,
        short_name: &[u8; SFN_SIZE],
    ) -> Result<(u64, u64), Error> {
        // get short name checksum
        let lfn_chsum = lfn_checksum(short_name);
        // create LFN entries generator
        let lfn_iter = LfnEntriesGenerator::new(lfn_utf16.as_ucs2_units(), lfn_chsum);
        // find space for new entries (multiple LFN entries and 1 SFN entry)
        let num_lfn_entries = lfn_iter.len();
        let start_pos = self.find_free_entries(num_lfn_entries as u32 + 1).await?;
        // write LFN entries before SFN entry
        for (index, lfn_entry) in lfn_iter.enumerate() {
            let offset = start_pos as usize + index * DIR_ENTRY_SIZE as usize;
            self.file
                .write_all_at(offset, &lfn_entry.to_bytes())
                .await?;
        }
        Ok((
            start_pos,
            start_pos + num_lfn_entries as u64 * u64::from(DIR_ENTRY_SIZE),
        ))
    }

    async fn write_entry(
        &self,
        name: &str,
        raw_entry: DirFileEntryData,
    ) -> Result<DirEntry<T>, Error> {
        fn encode_lfn_utf16(name: &str) -> LfnBuffer {
            LfnBuffer::from_ucs2_units(name.encode_utf16())
        }

        // check if name doesn't contain unsupported characters
        validate_long_name(name)?;
        // convert long name to UTF-16
        let lfn_utf16 = encode_lfn_utf16(name);
        // write LFN entries
        let (start_pos, entry_pos) = self
            .alloc_and_write_lfn_entries(&lfn_utf16, raw_entry.name())
            .await?;
        // write short name entry
        self.file
            .write_all_at(entry_pos as usize, &raw_entry.to_bytes())
            .await?;
        // return new logical entry descriptor
        let short_name = ShortName::new(raw_entry.name());
        Ok(DirEntry {
            data: raw_entry,
            short_name,
            lfn_utf16,
            entry_pos,
            offset_range: start_pos..(entry_pos + u64::from(DIR_ENTRY_SIZE)),

            fs: self.file.fs.clone(),
        })
    }

    async fn is_empty(&self) -> Result<bool, Error> {
        // check if directory contains no files
        let mut iter = pin!(self.iter(true));
        while let Some(r) = iter.next().await {
            let e = r?;
            let name = e.short_file_name_as_bytes();
            // ignore special entries "." and ".."
            if name != b"." && name != b".." {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[rustfmt::skip]
fn validate_long_name(name: &str) -> Result<(), Error> {
    // check if length is valid
    if name.is_empty() {
        return Err(EINVAL);
    }
    if name.len() > MAX_LONG_NAME_LEN {
        return Err(EINVAL);
    }
    // check if there are only valid characters
    for c in name.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9'
            | '\u{80}'..='\u{FFFF}'
            | '$' | '%' | '\'' | '-' | '_' | '@' | '~' | '`' | '!' | '(' | ')' | '{' | '}' | '.' | ' ' | '+' | ','
            | ';' | '=' | '[' | ']' | '^' | '#' | '&' => {},
            _ => return Err(EINVAL),
        }
    }
    Ok(())
}

fn lfn_checksum(short_name: &[u8; SFN_SIZE]) -> u8 {
    let mut chksum = num::Wrapping(0_u8);
    for b in short_name {
        chksum = (chksum << 7) + (chksum >> 1) + num::Wrapping(*b);
    }
    chksum.0
}

#[derive(Clone)]
pub(crate) struct LfnBuffer {
    ucs2_units: Vec<u16>,
}

const MAX_LONG_NAME_LEN: usize = 255;

const MAX_LONG_DIR_ENTRIES: usize = (MAX_LONG_NAME_LEN + LFN_PART_LEN - 1) / LFN_PART_LEN;

impl LfnBuffer {
    fn new() -> Self {
        Self {
            ucs2_units: Vec::<u16>::new(),
        }
    }

    fn from_ucs2_units<I: Iterator<Item = u16>>(usc2_units: I) -> Self {
        Self {
            ucs2_units: usc2_units.collect(),
        }
    }

    fn clear(&mut self) {
        self.ucs2_units.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.ucs2_units.len()
    }

    fn set_len(&mut self, len: usize) {
        self.ucs2_units.resize(len, 0_u16);
    }

    pub(crate) fn as_ucs2_units(&self) -> &[u16] {
        &self.ucs2_units
    }
}

struct LongNameBuilder {
    buf: LfnBuffer,
    chksum: u8,
    index: u8,
}

impl LongNameBuilder {
    fn new() -> Self {
        Self {
            buf: LfnBuffer::new(),
            chksum: 0,
            index: 0,
        }
    }

    fn clear(&mut self) {
        self.buf.clear();
        self.index = 0;
    }

    fn into_buf(mut self) -> LfnBuffer {
        // Check if last processed entry had index 1
        if self.index == 1 {
            self.truncate();
        } else if !self.is_empty() {
            log::warn!("unfinished LFN sequence {}", self.index);
            self.clear();
        }
        self.buf
    }

    fn truncate(&mut self) {
        // Truncate 0 and 0xFFFF characters from LFN buffer
        let ucs2_units = &self.buf.ucs2_units;
        let new_len = ucs2_units
            .iter()
            .rposition(|c| *c != 0xFFFF && *c != 0)
            .map_or(0, |n| n + 1);
        self.buf.set_len(new_len);
    }

    fn is_empty(&self) -> bool {
        // Check if any LFN entry has been processed
        // Note: index 0 is not a valid index in LFN and can be seen only after struct
        // initialization
        self.index == 0
    }

    fn process(&mut self, data: &DirLfnEntryData) {
        let is_last = (data.order() & LFN_ENTRY_LAST_FLAG) != 0;
        let index = data.order() & 0x1F;
        if index == 0 || usize::from(index) > MAX_LONG_DIR_ENTRIES {
            // Corrupted entry
            log::warn!("currupted lfn entry! {:x}", data.order());
            self.clear();
            return;
        }
        if is_last {
            // last entry is actually first entry in stream
            self.index = index;
            self.chksum = data.checksum();
            self.buf.set_len(usize::from(index) * LFN_PART_LEN);
        } else if self.index == 0 || index != self.index - 1 || data.checksum() != self.chksum {
            // Corrupted entry
            log::warn!(
                "currupted lfn entry! {:x} {:x} {:x} {:x}",
                data.order(),
                self.index,
                data.checksum(),
                self.chksum
            );
            self.clear();
            return;
        } else {
            // Decrement LFN index only for non-last entries
            self.index -= 1;
        }
        let pos = LFN_PART_LEN * usize::from(index - 1);
        // copy name parts into LFN buffer
        data.copy_name_to_slice(&mut self.buf.ucs2_units[pos..pos + 13]);
    }

    fn validate_chksum(&mut self, short_name: &[u8; SFN_SIZE]) {
        if self.is_empty() {
            // Nothing to validate - no LFN entries has been processed
            return;
        }
        let chksum = lfn_checksum(short_name);
        if chksum != self.chksum {
            log::warn!(
                "checksum mismatch {:x} {:x} {:?}",
                chksum,
                self.chksum,
                short_name
            );
            self.clear();
        }
    }
}

struct LfnEntriesGenerator<'a> {
    name_parts_iter: iter::Rev<slice::Chunks<'a, u16>>,
    checksum: u8,
    index: usize,
    num: usize,
    ended: bool,
}

impl<'a> LfnEntriesGenerator<'a> {
    fn new(name_utf16: &'a [u16], checksum: u8) -> Self {
        let num_entries = (name_utf16.len() + LFN_PART_LEN - 1) / LFN_PART_LEN;
        // create generator using reverse iterator over chunks - first chunk can be
        // shorter
        LfnEntriesGenerator {
            checksum,
            name_parts_iter: name_utf16.chunks(LFN_PART_LEN).rev(),
            index: 0,
            num: num_entries,
            ended: false,
        }
    }
}

const LFN_PADDING: u16 = 0xFFFF;

impl Iterator for LfnEntriesGenerator<'_> {
    type Item = DirLfnEntryData;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }

        // get next part from reverse iterator
        if let Some(name_part) = self.name_parts_iter.next() {
            let lfn_index = self.num - self.index;
            let mut order = lfn_index as u8;
            if self.index == 0 {
                // this is last name part (written as first)
                order |= LFN_ENTRY_LAST_FLAG;
            }
            debug_assert!(order > 0);
            let mut lfn_part = [LFN_PADDING; LFN_PART_LEN];
            lfn_part[..name_part.len()].copy_from_slice(name_part);
            if name_part.len() < LFN_PART_LEN {
                // name is only zero-terminated if its length is not multiplicity of
                // LFN_PART_LEN
                lfn_part[name_part.len()] = 0;
            }
            // create and return new LFN entry
            let mut lfn_entry = DirLfnEntryData::new(order, self.checksum);
            lfn_entry.copy_name_from_slice(&lfn_part);
            self.index += 1;
            Some(lfn_entry)
        } else {
            // end of name
            self.ended = true;
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.name_parts_iter.size_hint()
    }
}

// name_parts_iter is ExactSizeIterator so size_hint returns one limit
impl ExactSizeIterator for LfnEntriesGenerator<'_> {}

#[derive(Default, Debug, Clone)]
struct ShortNameGenerator {
    chksum: u16,
    long_prefix_bitmap: u16,
    prefix_chksum_bitmap: u16,
    name_fits: bool,
    lossy_conv: bool,
    exact_match: bool,
    basename_len: usize,
    short_name: [u8; SFN_SIZE],
}

impl ShortNameGenerator {
    fn new(name: &str) -> Self {
        // padded by ' '
        let mut short_name = [SFN_PADDING; SFN_SIZE];
        // find extension after last dot
        // Note: short file name cannot start with the extension
        let dot_index_opt = name[1..].rfind('.').map(|index| index + 1);
        // copy basename (part of filename before a dot)
        let basename_src = dot_index_opt.map_or(name, |dot_index| &name[..dot_index]);
        let (basename_len, basename_fits, basename_lossy) =
            Self::copy_short_name_part(&mut short_name[0..8], basename_src);
        // copy file extension if exists
        let (name_fits, lossy_conv) =
            dot_index_opt.map_or((basename_fits, basename_lossy), |dot_index| {
                let (_, ext_fits, ext_lossy) =
                    Self::copy_short_name_part(&mut short_name[8..11], &name[dot_index + 1..]);
                (basename_fits && ext_fits, basename_lossy || ext_lossy)
            });
        let chksum = Self::checksum(name);
        Self {
            chksum,
            name_fits,
            lossy_conv,
            basename_len,
            short_name,
            ..Self::default()
        }
    }

    fn generate_dot() -> [u8; SFN_SIZE] {
        let mut short_name = [SFN_PADDING; SFN_SIZE];
        short_name[0] = b'.';
        short_name
    }

    fn generate_dotdot() -> [u8; SFN_SIZE] {
        let mut short_name = [SFN_PADDING; SFN_SIZE];
        short_name[0] = b'.';
        short_name[1] = b'.';
        short_name
    }

    fn copy_short_name_part(dst: &mut [u8], src: &str) -> (usize, bool, bool) {
        let mut dst_pos = 0;
        let mut lossy_conv = false;
        for c in src.chars() {
            if dst_pos == dst.len() {
                // result buffer is full
                return (dst_pos, false, lossy_conv);
            }
            // Make sure character is allowed in 8.3 name
            #[rustfmt::skip]
            let fixed_c = match c {
                // strip spaces and dots
                ' ' | '.' => {
                    lossy_conv = true;
                    continue;
                },
                // copy allowed characters
                'A'..='Z' | 'a'..='z' | '0'..='9'
                | '!' | '#' | '$' | '%' | '&' | '\'' | '(' | ')' | '-' | '@' | '^' | '_' | '`' | '{' | '}' | '~' => c,
                // replace disallowed characters by underscore
                _ => '_',
            };
            // Update 'lossy conversion' flag
            lossy_conv = lossy_conv || (fixed_c != c);
            // short name is always uppercase
            let upper = fixed_c.to_ascii_uppercase();
            dst[dst_pos] = upper as u8; // SAFE: upper is in range 0x20-0x7F
            dst_pos += 1;
        }
        (dst_pos, true, lossy_conv)
    }

    fn add_existing(&mut self, short_name: &[u8; SFN_SIZE]) {
        // check for exact match collision
        if short_name == &self.short_name {
            self.exact_match = true;
        }
        // check for long prefix form collision (TEXTFI~1.TXT)
        self.check_for_long_prefix_collision(short_name);

        // check for short prefix + checksum form collision (TE021F~1.TXT)
        self.check_for_short_prefix_collision(short_name);
    }

    fn check_for_long_prefix_collision(&mut self, short_name: &[u8; SFN_SIZE]) {
        // check for long prefix form collision (TEXTFI~1.TXT)
        let long_prefix_len = cmp::min(self.basename_len, 6);
        if short_name[long_prefix_len] != b'~' {
            return;
        }
        if let Some(num_suffix) = char::from(short_name[long_prefix_len + 1]).to_digit(10) {
            let long_prefix_matches =
                short_name[..long_prefix_len] == self.short_name[..long_prefix_len];
            let ext_matches = short_name[8..] == self.short_name[8..];
            if long_prefix_matches && ext_matches {
                self.long_prefix_bitmap |= 1 << num_suffix;
            }
        }
    }

    fn check_for_short_prefix_collision(&mut self, short_name: &[u8; SFN_SIZE]) {
        // check for short prefix + checksum form collision (TE021F~1.TXT)
        let short_prefix_len = cmp::min(self.basename_len, 2);
        if short_name[short_prefix_len + 4] != b'~' {
            return;
        }
        if let Some(num_suffix) = char::from(short_name[short_prefix_len + 4 + 1]).to_digit(10) {
            let short_prefix_matches =
                short_name[..short_prefix_len] == self.short_name[..short_prefix_len];
            let ext_matches = short_name[8..] == self.short_name[8..];
            if short_prefix_matches && ext_matches {
                let chksum_res =
                    str::from_utf8(&short_name[short_prefix_len..short_prefix_len + 4])
                        .map(|s| u16::from_str_radix(s, 16));

                if chksum_res == Ok(Ok(self.chksum)) {
                    self.prefix_chksum_bitmap |= 1 << num_suffix;
                }
            }
        }
    }

    fn checksum(name: &str) -> u16 {
        // BSD checksum algorithm
        let mut chksum = num::Wrapping(0_u16);
        for c in name.chars() {
            chksum = (chksum >> 1) + (chksum << 15) + num::Wrapping(c as u16);
        }
        chksum.0
    }

    fn generate(&self) -> Result<[u8; SFN_SIZE], Error> {
        if !self.lossy_conv && self.name_fits && !self.exact_match {
            // If there was no lossy conversion and name fits into
            // 8.3 convention and there is no collision return it as is
            return Ok(self.short_name);
        }
        // Try using long 6-characters prefix
        for i in 1..5 {
            if self.long_prefix_bitmap & (1 << i) == 0 {
                return Ok(self.build_prefixed_name(i, false));
            }
        }
        // Try prefix with checksum
        for i in 1..10 {
            if self.prefix_chksum_bitmap & (1 << i) == 0 {
                return Ok(self.build_prefixed_name(i, true));
            }
        }
        // Too many collisions - fail
        Err(EEXIST)
    }

    fn next_iteration(&mut self) {
        // Try different checksum in next iteration
        self.chksum = (num::Wrapping(self.chksum) + num::Wrapping(1)).0;
        // Zero bitmaps
        self.long_prefix_bitmap = 0;
        self.prefix_chksum_bitmap = 0;
    }

    fn build_prefixed_name(&self, num: u32, with_chksum: bool) -> [u8; SFN_SIZE] {
        let mut buf = [SFN_PADDING; SFN_SIZE];
        let prefix_len = if with_chksum {
            let prefix_len = cmp::min(self.basename_len, 2);
            buf[..prefix_len].copy_from_slice(&self.short_name[..prefix_len]);
            buf[prefix_len..prefix_len + 4].copy_from_slice(&Self::u16_to_hex(self.chksum));
            prefix_len + 4
        } else {
            let prefix_len = cmp::min(self.basename_len, 6);
            buf[..prefix_len].copy_from_slice(&self.short_name[..prefix_len]);
            prefix_len
        };
        buf[prefix_len] = b'~';
        buf[prefix_len + 1] = char::from_digit(num, 10).unwrap() as u8; // SAFE: num is in range [1, 9]
        buf[8..].copy_from_slice(&self.short_name[8..]);
        buf
    }

    fn u16_to_hex(x: u16) -> [u8; 4] {
        // Unwrapping below is safe because each line takes 4 bits of `x` and shifts
        // them to the right so they form a number in range [0, 15]
        let x_u32 = u32::from(x);
        let mut hex_bytes = [
            char::from_digit((x_u32 >> 12) & 0xF, 16).unwrap() as u8,
            char::from_digit((x_u32 >> 8) & 0xF, 16).unwrap() as u8,
            char::from_digit((x_u32 >> 4) & 0xF, 16).unwrap() as u8,
            char::from_digit(x_u32 & 0xF, 16).unwrap() as u8,
        ];
        hex_bytes.make_ascii_uppercase();
        hex_bytes
    }
}

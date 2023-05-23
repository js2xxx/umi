use alloc::{sync::Arc, vec, vec::Vec};
use core::{
    mem,
    ops::Range,
    pin::{pin, Pin},
};

use futures_util::{stream, stream::StreamExt};
use goblin::elf64::{header::*, program_header::*, section_header::*};
use kmem::{Phys, Virt, ZERO};
use ksc::Error::{ENOEXEC, ENOSYS};
use rv39_paging::{Attr, LAddr, PAGE_MASK, PAGE_SHIFT};
use umifs::traits::IoExt;

#[derive(Debug)]
pub enum Error {
    ElfParse(goblin::error::Error),
    NotSupported(&'static str),
    PhysAlloc(ksc::Error),
    PhysRead(ksc::Error),
    PhysWrite(ksc::Error),
    VirtAlloc(ksc::Error),
    VirtMap(ksc::Error),
}

impl From<Error> for ksc::Error {
    fn from(value: Error) -> Self {
        log::error!("parsing elf error: {value:?}");
        match value {
            Error::ElfParse(_) => ENOEXEC,
            Error::NotSupported(_) => ENOSYS,
            Error::PhysAlloc(err)
            | Error::PhysRead(err)
            | Error::PhysWrite(err)
            | Error::VirtAlloc(err)
            | Error::VirtMap(err) => err,
        }
    }
}

pub struct LoadedElf {
    pub is_dyn: bool,
    pub range: Range<LAddr>,
    pub header: Header,
    /// Note: The size of the stack can be zero and the caller should check it
    /// before allocating memory for the stack.
    pub stack: Option<(usize, Attr)>,
    pub entry: LAddr,
    pub dynamic: Option<ProgramHeader>,
    pub sym_len: usize,
}

fn parse_attr(flags: u32) -> Attr {
    Attr::builder()
        .user_access(true)
        .readable(flags & PF_R != 0)
        .writable(flags & PF_W != 0)
        .executable(flags & PF_X != 0)
        .build()
}

async fn parse_header(phys: &Phys, force_dyn: Option<bool>) -> Result<(Header, bool), Error> {
    let mut data = [0; mem::size_of::<Header>()];
    phys.read_exact_at(0, &mut data)
        .await
        .map_err(Error::PhysRead)?;

    let header = Header::parse(&data).map_err(Error::ElfParse)?;

    if header.e_ident[EI_CLASS] != ELFCLASS64 {
        return Err(Error::NotSupported("Only support 64-bit file"));
    }
    if header.e_ident[EI_DATA] != ELFDATA2LSB {
        return Err(Error::NotSupported("Only support little endian file"));
    }
    if (force_dyn == Some(true) || header.e_type != ET_EXEC) && header.e_type != ET_DYN {
        return Err(Error::NotSupported(
            "Only support dynamic (or executable if enabled) file",
        ));
    }
    if force_dyn == Some(false) && header.e_type != ET_EXEC {
        return Err(Error::NotSupported("Only support static file"));
    }

    Ok((header, header.e_type == ET_DYN))
}

async fn parse_segments(
    phys: &Phys,
    offset: usize,
    count: usize,
) -> Result<Vec<ProgramHeader>, Error> {
    let mut data = vec![0; count * mem::size_of::<ProgramHeader>()];
    phys.read_exact_at(offset, &mut data)
        .await
        .map_err(Error::PhysRead)?;

    Ok(ProgramHeader::from_bytes(&data, count))
}

async fn parse_sections(
    phys: &Phys,
    offset: usize,
    count: usize,
) -> Result<Vec<SectionHeader>, Error> {
    let mut data = vec![0; count * mem::size_of::<SectionHeader>()];
    phys.read_exact_at(offset, &mut data)
        .await
        .map_err(Error::PhysRead)?;

    Ok(SectionHeader::from_bytes(&data, count))
}

fn get_addr_range_info(segments: &[ProgramHeader]) -> (usize, usize) {
    segments
        .iter()
        .filter(|segment| segment.p_type == PT_LOAD)
        .fold((usize::MAX, 0), |(min, max), segment| {
            let base = segment.p_vaddr as usize;
            let size = segment.p_memsz as usize;
            (min.min(base), max.max(base + size))
        })
}

/// # Mapping details
///
///     page boundaries
///      +......................+......................+......................+
///
///         offset
///         +------------------------------+---------------------------+
///         |         file_size            |                           |
///         +------------------------------+             memory_size   |
///         +----------------------------------------------------------+
///
///     aligned_offset                data_end    file_end           memory_end
///      +----------------------+----------+-----------+----------------------+
///      |      aligned_data_size          | zero_size |                      |
///      +---------------------------------+-----------+                      |
///      |            aligned_file_size                |       map_size       |
///      +---------------------------------------------+                      |
///      +--------------------------------------------------------------------+
async fn map_segment(
    segment: &ProgramHeader,
    phys: &Phys,
    virt: Pin<&Virt>,
    base: LAddr,
) -> Result<(), Error> {
    let memory_size = segment.p_memsz as usize;
    let file_size = segment.p_filesz as usize;
    let offset = segment.p_offset as usize;
    let address = segment.p_vaddr as usize;

    if offset & PAGE_MASK != address & PAGE_MASK {
        return Err(Error::NotSupported(
            "Offset of segments must be page aligned",
        ));
    }
    let file_end = (offset + file_size + PAGE_MASK) & !PAGE_MASK;
    let data_end = offset + file_size;
    let memory_end = (offset + memory_size + PAGE_MASK) & !PAGE_MASK;
    let aligned_offset = offset & !PAGE_MASK;
    let aligned_address = address & !PAGE_MASK;
    let aligned_file_size = file_end - aligned_offset;
    let aligned_data_size = data_end - aligned_offset;
    let zero_size = file_end - data_end;
    let map_size = memory_end - aligned_offset;

    let attr = parse_attr(segment.p_flags);

    if map_size > 0 {
        log::trace!(
            "elf::load: Map {:#x}~{:#x} -> {:?}, zero {:#x} size extra data",
            aligned_offset,
            aligned_offset + map_size,
            base + aligned_address,
            zero_size,
        );

        let segment = phys.clone_as(
            true,
            aligned_offset >> PAGE_SHIFT,
            Some(aligned_file_size >> PAGE_SHIFT),
        );
        segment
            .write_all_at(aligned_data_size, &ZERO[..zero_size])
            .await
            .map_err(Error::PhysWrite)?;

        virt.map(
            Some(base + aligned_address),
            Arc::new(segment),
            0,
            map_size >> PAGE_SHIFT,
            attr,
        )
        .await
        .map_err(Error::VirtMap)?;
    }
    Ok(())
}

pub async fn get_interp(phys: &Phys) -> Result<Option<Vec<u8>>, Error> {
    let (header, _) = parse_header(phys, None).await?;
    let segments = parse_segments(phys, header.e_phoff as usize, header.e_phnum as usize).await?;

    let iter = stream::iter(segments.into_iter()).filter_map(|segment| async move {
        if segment.p_type == PT_INTERP {
            let offset = segment.p_offset as usize;
            let size = segment.p_filesz as usize;

            let mut ret = vec![0; size];

            let res = phys.read_exact_at(offset, &mut ret).await;
            Some(res.map_err(Error::PhysRead).map(|_| ret))
        } else {
            None
        }
    });
    pin!(iter).next().await.transpose()
}

pub async fn load(
    phys: &Phys,
    force_dyn: Option<bool>,
    virt: Pin<&Virt>,
) -> Result<LoadedElf, Error> {
    log::trace!("elf::load");
    if !phys.is_cow() {
        return Err(Error::NotSupported("the Phys should be COW"));
    }
    let (header, is_dyn) = parse_header(phys, force_dyn).await?;

    let segments = parse_segments(phys, header.e_phoff as usize, header.e_phnum as usize).await?;
    let sections = parse_sections(phys, header.e_shoff as usize, header.e_shnum as usize).await?;
    let (min, max) = get_addr_range_info(&segments);
    log::trace!("elf::load: address range: {min:#x}..{max:#x}");

    let base = {
        let count = (max - min + PAGE_MASK) >> PAGE_SHIFT;

        let start = if is_dyn { None } else { Some(min.into()) };
        let find_free = virt.find_free(start, count);
        find_free.await.map_err(Error::VirtAlloc)?.start
    };
    let offset = if is_dyn { base } else { LAddr::from(0usize) };
    log::trace!("elf::load: set base at {base:?}");

    let entry = offset + header.e_entry as usize;

    let mut stack = None;
    let mut dynamic = None;
    for segment in segments {
        match segment.p_type {
            PT_LOAD => map_segment(&segment, phys, virt, offset).await?,
            PT_GNU_STACK => stack = Some((segment.p_memsz as usize, parse_attr(segment.p_flags))),
            PT_DYNAMIC => dynamic = Some(segment),
            _ => {}
        }
    }

    let sym_len = sections
        .into_iter()
        .find_map(|section| {
            #[allow(clippy::unnecessary_lazy_evaluations)]
            (section.sh_type == SHT_DYNSYM && section.sh_entsize != 0)
                .then(|| (section.sh_size / section.sh_entsize) as usize)
        })
        .unwrap_or_default();

    log::debug!(
        "elf::load: entry = {entry:?}, {}",
        if is_dyn { "dynamic" } else { "static" }
    );
    Ok(LoadedElf {
        is_dyn,
        range: base..(base + (max - min)),
        header,
        stack,
        entry,
        dynamic,
        sym_len,
    })
}

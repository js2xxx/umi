use alloc::string::{String, ToString};
use core::{char, fmt, ops::Range, str};

use arsc_rs::Arsc;
use bitflags::bitflags;
use concat_arrays::concat_arrays;
use ksc_core::Error;
use nom::IResult;
use umifs::traits::{File, FileExt};

use crate::{
    dir::{FatDir, LfnBuffer},
    file::FatFile,
    fs::FatFileSystem,
    time::{Date, DateTime},
    TimeProvider,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct LossyOemCpConverter;

impl LossyOemCpConverter {
    fn decode(oem_char: u8) -> char {
        if oem_char <= 0x7F {
            char::from(oem_char)
        } else {
            '\u{FFFD}'
        }
    }

    #[allow(dead_code)]
    fn encode(uni_char: char) -> Option<u8> {
        if uni_char <= '\x7F' {
            Some(uni_char as u8) // safe cast: value is in range [0, 0x7F]
        } else {
            None
        }
    }
}

bitflags! {
    /// A FAT file attributes.
    #[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct FileAttributes: u8 {
        const READ_ONLY  = 0x01;
        const HIDDEN     = 0x02;
        const SYSTEM     = 0x04;
        const VOLUME_ID  = 0x08;
        const DIRECTORY  = 0x10;
        const ARCHIVE    = 0x20;
        const LFN        = Self::READ_ONLY.bits() | Self::HIDDEN.bits()
                         | Self::SYSTEM.bits() | Self::VOLUME_ID.bits();
    }
}

// Size of single directory entry in bytes
pub(crate) const DIR_ENTRY_SIZE: u32 = 32;

// Directory entry flags available in first byte of the short name
pub(crate) const DIR_ENTRY_DELETED_FLAG: u8 = 0xE5;
pub(crate) const DIR_ENTRY_REALLY_E5_FLAG: u8 = 0x05;

// Short file name field size in bytes (besically 8 + 3)
pub(crate) const SFN_SIZE: usize = 11;

// Byte used for short name padding
pub(crate) const SFN_PADDING: u8 = b' ';

// Length in characters of a LFN fragment packed in one directory entry
pub(crate) const LFN_PART_LEN: usize = 13;

// Bit used in order field to mark last LFN entry
pub(crate) const LFN_ENTRY_LAST_FLAG: u8 = 0x40;

// Character to upper case conversion which supports Unicode only if `unicode`
// feature is enabled
fn char_to_uppercase(c: char) -> char::ToUppercase {
    c.to_uppercase()
}

/// Decoded file short name
#[derive(Clone, Debug, Default)]
pub(crate) struct ShortName {
    name: [u8; 12],
    len: u8,
}

impl ToString for ShortName {
    fn to_string(&self) -> String {
        // Strip non-ascii characters from short name
        self.as_bytes()
            .iter()
            .copied()
            .map(LossyOemCpConverter::decode)
            .collect()
    }
}

impl ShortName {
    pub(crate) fn new(raw_name: &[u8; SFN_SIZE]) -> Self {
        // get name components length by looking for space character
        let name_len = raw_name[0..8]
            .iter()
            .rposition(|x| *x != SFN_PADDING)
            .map_or(0, |p| p + 1);
        let ext_len = raw_name[8..11]
            .iter()
            .rposition(|x| *x != SFN_PADDING)
            .map_or(0, |p| p + 1);
        let mut name = [SFN_PADDING; 12];
        name[..name_len].copy_from_slice(&raw_name[..name_len]);
        let total_len = if ext_len > 0 {
            name[name_len] = b'.';
            name[name_len + 1..name_len + 1 + ext_len].copy_from_slice(&raw_name[8..8 + ext_len]);
            // Return total name length
            name_len + 1 + ext_len
        } else {
            // No extension - return length of name part
            name_len
        };
        // FAT encodes character 0xE5 as 0x05 because 0xE5 marks deleted files
        if name[0] == DIR_ENTRY_REALLY_E5_FLAG {
            name[0] = 0xE5;
        }
        // Short names in FAT filesystem are encoded in OEM code-page
        Self {
            name,
            len: total_len as u8,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.name[..usize::from(self.len)]
    }

    fn eq_ignore_case(&self, name: &str) -> bool {
        // Convert name to UTF-8 character iterator
        let byte_iter = self.as_bytes().iter().copied();
        let char_iter = byte_iter.map(LossyOemCpConverter::decode);
        // Compare interators ignoring case
        let uppercase_char_iter = char_iter.flat_map(char_to_uppercase);
        uppercase_char_iter.eq(name.chars().flat_map(char_to_uppercase))
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DirFileEntryData {
    name: [u8; SFN_SIZE],
    attrs: FileAttributes,
    reserved_0: u8,
    create_time_0: u8,
    create_time_1: u16,
    create_date: u16,
    access_date: u16,
    first_cluster_hi: u16,
    modify_time: u16,
    modify_date: u16,
    first_cluster_lo: u16,
    size: u32,
}

impl DirFileEntryData {
    pub(crate) fn new(name: [u8; SFN_SIZE], attrs: FileAttributes) -> Self {
        Self {
            name,
            attrs,
            ..Self::default()
        }
    }

    pub(crate) fn renamed(&self, new_name: [u8; SFN_SIZE]) -> Self {
        let mut sfn_entry = self.clone();
        sfn_entry.name = new_name;
        sfn_entry
    }

    pub(crate) fn name(&self) -> &[u8; SFN_SIZE] {
        &self.name
    }

    fn lowercase_name(&self) -> ShortName {
        let mut name_copy: [u8; SFN_SIZE] = self.name;
        if self.lowercase_basename() {
            name_copy[..8].make_ascii_lowercase();
        }
        if self.lowercase_ext() {
            name_copy[8..].make_ascii_lowercase();
        }
        ShortName::new(&name_copy)
    }

    pub(crate) fn first_cluster(&self) -> Option<u32> {
        let cluster = (u32::from(self.first_cluster_hi) << 16) | u32::from(self.first_cluster_lo);
        (cluster != 0).then_some(cluster)
    }

    pub(crate) fn set_first_cluster(&mut self, cluster: Option<u32>) {
        let n = cluster.unwrap_or(0);
        self.first_cluster_hi = (n >> 16) as u16;
        self.first_cluster_lo = (n & 0xFFFF) as u16;
    }

    pub(crate) fn size(&self) -> Option<u32> {
        if self.is_file() {
            Some(self.size)
        } else {
            None
        }
    }

    fn set_size(&mut self, size: u32) {
        self.size = size;
    }

    pub(crate) fn is_dir(&self) -> bool {
        self.attrs.contains(FileAttributes::DIRECTORY)
    }

    fn is_file(&self) -> bool {
        !self.is_dir()
    }

    fn lowercase_basename(&self) -> bool {
        self.reserved_0 & (1 << 3) != 0
    }

    fn lowercase_ext(&self) -> bool {
        self.reserved_0 & (1 << 4) != 0
    }

    fn created(&self) -> DateTime {
        DateTime::decode(self.create_date, self.create_time_1, self.create_time_0)
    }

    fn accessed(&self) -> Date {
        Date::decode(self.access_date)
    }

    fn modified(&self) -> DateTime {
        DateTime::decode(self.modify_date, self.modify_time, 0)
    }

    pub(crate) fn set_created(&mut self, date_time: DateTime) {
        self.create_date = date_time.date.encode();
        let encoded_time = date_time.time.encode();
        self.create_time_1 = encoded_time.0;
        self.create_time_0 = encoded_time.1;
    }

    pub(crate) fn set_accessed(&mut self, date: Date) {
        self.access_date = date.encode();
    }

    pub(crate) fn set_modified(&mut self, date_time: DateTime) {
        self.modify_date = date_time.date.encode();
        self.modify_time = date_time.time.encode().0;
    }

    #[allow(clippy::drop_non_drop)]
    pub(crate) fn to_bytes(&self) -> [u8; 32] {
        concat_arrays!(
            self.name,
            [self.attrs.bits(), self.reserved_0, self.create_time_0],
            self.create_time_1.to_le_bytes(),
            self.create_date.to_le_bytes(),
            self.access_date.to_le_bytes(),
            self.first_cluster_hi.to_le_bytes(),
            self.modify_time.to_le_bytes(),
            self.modify_date.to_le_bytes(),
            self.first_cluster_lo.to_le_bytes(),
            self.size.to_le_bytes(),
        )
    }

    pub(crate) fn is_deleted(&self) -> bool {
        self.name[0] == DIR_ENTRY_DELETED_FLAG
    }

    pub(crate) fn set_deleted(&mut self) {
        self.name[0] = DIR_ENTRY_DELETED_FLAG;
    }

    pub(crate) fn is_end(&self) -> bool {
        self.name[0] == 0
    }

    pub(crate) fn is_volume(&self) -> bool {
        self.attrs.contains(FileAttributes::VOLUME_ID)
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub(crate) struct DirLfnEntryData {
    order: u8,
    name_0: [u16; 5],
    attrs: FileAttributes,
    entry_type: u8,
    checksum: u8,
    name_1: [u16; 6],
    reserved_0: u16,
    name_2: [u16; 2],
}

impl DirLfnEntryData {
    pub(crate) fn new(order: u8, checksum: u8) -> Self {
        Self {
            order,
            checksum,
            attrs: FileAttributes::LFN,
            ..Self::default()
        }
    }

    pub(crate) fn copy_name_from_slice(&mut self, lfn_part: &[u16; LFN_PART_LEN]) {
        self.name_0.copy_from_slice(&lfn_part[0..5]);
        self.name_1.copy_from_slice(&lfn_part[5..5 + 6]);
        self.name_2.copy_from_slice(&lfn_part[11..11 + 2]);
    }

    pub(crate) fn copy_name_to_slice(&self, lfn_part: &mut [u16]) {
        debug_assert!(lfn_part.len() == LFN_PART_LEN);
        lfn_part[0..5].copy_from_slice(&self.name_0);
        lfn_part[5..11].copy_from_slice(&self.name_1);
        lfn_part[11..13].copy_from_slice(&self.name_2);
    }

    #[allow(clippy::drop_non_drop)]
    pub(crate) fn to_bytes(&self) -> [u8; 32] {
        concat_arrays!(
            [self.order],
            self.name_0[0].to_le_bytes(),
            self.name_0[1].to_le_bytes(),
            self.name_0[2].to_le_bytes(),
            self.name_0[3].to_le_bytes(),
            self.name_0[4].to_le_bytes(),
            [self.attrs.bits(), self.entry_type, self.checksum],
            self.name_1[0].to_le_bytes(),
            self.name_1[1].to_le_bytes(),
            self.name_1[2].to_le_bytes(),
            self.name_1[3].to_le_bytes(),
            self.name_1[4].to_le_bytes(),
            self.name_1[5].to_le_bytes(),
            self.reserved_0.to_le_bytes(),
            self.name_2[0].to_le_bytes(),
            self.name_2[1].to_le_bytes(),
        )
    }

    pub(crate) fn order(&self) -> u8 {
        self.order
    }

    pub(crate) fn checksum(&self) -> u8 {
        self.checksum
    }

    pub(crate) fn is_deleted(&self) -> bool {
        self.order == DIR_ENTRY_DELETED_FLAG
    }

    pub(crate) fn set_deleted(&mut self) {
        self.order = DIR_ENTRY_DELETED_FLAG;
    }

    pub(crate) fn is_end(&self) -> bool {
        self.order == 0
    }
}

#[derive(Clone, Debug)]
pub(crate) enum DirEntryData {
    File(DirFileEntryData),
    Lfn(DirLfnEntryData),
}

impl DirEntryData {
    pub(crate) fn to_bytes(&self) -> [u8; 32] {
        match self {
            DirEntryData::File(file) => file.to_bytes(),
            DirEntryData::Lfn(lfn) => lfn.to_bytes(),
        }
    }

    pub(crate) fn parse(mut input: &[u8]) -> IResult<&[u8], Self> {
        let mut name = [0; SFN_SIZE];

        let n;
        (input, n) = nom::bytes::streaming::take(SFN_SIZE)(input)?;
        name.copy_from_slice(n);

        let attrs;
        (input, attrs) = nom::number::streaming::u8(input)?;
        let attrs = FileAttributes::from_bits_truncate(attrs);

        if attrs.contains(FileAttributes::LFN) {
            // read long name entry
            let mut data = DirLfnEntryData {
                attrs,
                ..DirLfnEntryData::default()
            };
            // divide the name into order and LFN name_0
            data.order = name[0];
            for (dst, src) in data.name_0.iter_mut().zip(name[1..].chunks_exact(2)) {
                // unwrap cannot panic because src has exactly 2 values
                *dst = u16::from_le_bytes(src.try_into().unwrap());
            }

            (input, data.entry_type) = nom::number::streaming::u8(input)?;
            (input, data.checksum) = nom::number::streaming::u8(input)?;
            for x in &mut data.name_1 {
                (input, *x) = nom::number::streaming::le_u16(input)?;
            }
            (input, data.reserved_0) = nom::number::streaming::le_u16(input)?;
            for x in &mut data.name_2 {
                (input, *x) = nom::number::streaming::le_u16(input)?;
            }
            Ok((input, DirEntryData::Lfn(data)))
        } else {
            // read short name entry
            let mut data = DirFileEntryData {
                name,
                attrs,
                ..Default::default()
            };
            (input, data.reserved_0) = nom::number::streaming::u8(input)?;
            (input, data.create_time_0) = nom::number::streaming::u8(input)?;
            (input, data.create_time_1) = nom::number::streaming::le_u16(input)?;
            (input, data.create_date) = nom::number::streaming::le_u16(input)?;
            (input, data.access_date) = nom::number::streaming::le_u16(input)?;
            (input, data.first_cluster_hi) = nom::number::streaming::le_u16(input)?;
            (input, data.modify_time) = nom::number::streaming::le_u16(input)?;
            (input, data.modify_date) = nom::number::streaming::le_u16(input)?;
            (input, data.first_cluster_lo) = nom::number::streaming::le_u16(input)?;
            (input, data.size) = nom::number::streaming::le_u32(input)?;

            Ok((input, DirEntryData::File(data)))
        }
    }

    pub(crate) fn is_deleted(&self) -> bool {
        match self {
            DirEntryData::File(file) => file.is_deleted(),
            DirEntryData::Lfn(lfn) => lfn.is_deleted(),
        }
    }

    pub(crate) fn set_deleted(&mut self) {
        match self {
            DirEntryData::File(file) => file.set_deleted(),
            DirEntryData::Lfn(lfn) => lfn.set_deleted(),
        }
    }

    pub(crate) fn is_end(&self) -> bool {
        match self {
            DirEntryData::File(file) => file.is_end(),
            DirEntryData::Lfn(lfn) => lfn.is_end(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DirEntryEditor {
    data: DirFileEntryData,
    pos: u64,
    dirty: bool,
}

impl DirEntryEditor {
    fn new(data: DirFileEntryData, pos: u64) -> Self {
        Self {
            data,
            pos,
            dirty: false,
        }
    }

    pub(crate) fn inner(&self) -> &DirFileEntryData {
        &self.data
    }

    pub(crate) fn set_first_cluster(&mut self, first_cluster: Option<u32>) {
        if first_cluster != self.data.first_cluster() {
            self.data.set_first_cluster(first_cluster);
            self.dirty = true;
        }
    }

    pub(crate) fn set_size(&mut self, size: u32) {
        match self.data.size() {
            Some(n) if size != n => {
                self.data.set_size(size);
                self.dirty = true;
            }
            _ => {}
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_created(&mut self, date_time: DateTime) {
        if date_time != self.data.created() {
            self.data.set_created(date_time);
            self.dirty = true;
        }
    }

    pub(crate) fn set_accessed(&mut self, date: Date) {
        if date != self.data.accessed() {
            self.data.set_accessed(date);
            self.dirty = true;
        }
    }

    pub(crate) fn set_modified(&mut self, date_time: DateTime) {
        if date_time != self.data.modified() {
            self.data.set_modified(date_time);
            self.dirty = true;
        }
    }

    pub(crate) async fn flush(&mut self, device: &dyn File) -> Result<(), Error> {
        if self.dirty {
            self.write(device).await?;
            self.dirty = false;
        }
        Ok(())
    }

    async fn write(&self, device: &dyn File) -> Result<(), Error> {
        let bytes = self.data.to_bytes();
        device
            .write_all_at(self.pos as usize, &mut [&bytes])
            .await?;
        Ok(())
    }
}

/// A FAT directory entry.
///
/// `DirEntry` is returned by `DirIter` when reading a directory.
#[derive(Clone)]
pub struct DirEntry<T: TimeProvider> {
    pub(crate) data: DirFileEntryData,
    pub(crate) short_name: ShortName,
    pub(crate) lfn_utf16: LfnBuffer,
    pub(crate) entry_pos: u64,
    pub(crate) offset_range: Range<u64>,
    pub(crate) fs: Arsc<FatFileSystem<T>>,
}

#[allow(clippy::len_without_is_empty)]
impl<T: TimeProvider> DirEntry<T> {
    /// Returns short file name.
    ///
    /// Non-ASCII characters are replaced by the replacement character (U+FFFD).
    #[must_use]
    pub fn short_file_name(&self) -> String {
        self.short_name.to_string()
    }

    /// Returns short file name as byte array slice.
    ///
    /// Characters are encoded in the OEM codepage.
    #[must_use]
    pub fn short_file_name_as_bytes(&self) -> &[u8] {
        self.short_name.as_bytes()
    }

    /// Returns long file name as u16 array slice.
    ///
    /// Characters are encoded in the UCS-2 encoding.
    #[must_use]
    pub fn long_file_name_as_ucs2_units(&self) -> Option<&[u16]> {
        if self.lfn_utf16.len() > 0 {
            Some(self.lfn_utf16.as_ucs2_units())
        } else {
            None
        }
    }

    /// Returns long file name or if it doesn't exist fallbacks to short file
    /// name.
    #[must_use]
    pub fn file_name(&self) -> String {
        let lfn_opt = self.long_file_name_as_ucs2_units();
        if let Some(lfn) = lfn_opt {
            return String::from_utf16_lossy(lfn);
        }
        self.data.lowercase_name().to_string()
    }

    /// Returns file attributes.
    #[must_use]
    pub fn attributes(&self) -> FileAttributes {
        self.data.attrs
    }

    /// Checks if entry belongs to directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.data.is_dir()
    }

    /// Checks if entry belongs to regular file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.data.is_file()
    }

    pub(crate) fn first_cluster(&self) -> Option<u32> {
        self.data.first_cluster()
    }

    fn editor(&self) -> DirEntryEditor {
        DirEntryEditor::new(self.data.clone(), self.entry_pos)
    }

    pub(crate) fn is_same_entry(&self, other: &DirEntry<T>) -> bool {
        self.entry_pos == other.entry_pos
    }

    /// Returns `File` struct for this entry.
    ///
    /// # Panics
    ///
    /// Will panic if this is not a file.
    pub async fn to_file(&self) -> Result<FatFile<T>, Error> {
        assert!(!self.is_dir(), "Not a file entry");
        FatFile::new(self.fs.clone(), self.first_cluster(), Some(self.editor())).await
    }

    /// Returns `Dir` struct for this entry.
    ///
    /// # Panics
    ///
    /// Will panic if this is not a directory.
    pub async fn to_dir(&self) -> Result<FatDir<T>, Error> {
        assert!(self.is_dir(), "Not a directory entry");
        match self.first_cluster() {
            Some(n) => {
                let file = FatFile::new(self.fs.clone(), Some(n), Some(self.editor())).await?;
                Ok(FatDir::new(file))
            }
            None => self.fs.clone().root_dir().await,
        }
    }

    /// Returns file size or 0 for directory.
    #[must_use]
    pub fn len(&self) -> u64 {
        u64::from(self.data.size)
    }

    /// Returns file creation date and time.
    ///
    /// Resolution of the time field is 1/100s.
    #[must_use]
    pub fn created(&self) -> DateTime {
        self.data.created()
    }

    /// Returns file last access date.
    #[must_use]
    pub fn accessed(&self) -> Date {
        self.data.accessed()
    }

    /// Returns file last modification date and time.
    ///
    /// Resolution of the time field is 2s.
    #[must_use]
    pub fn modified(&self) -> DateTime {
        self.data.modified()
    }

    pub(crate) fn raw_short_name(&self) -> &[u8; SFN_SIZE] {
        &self.data.name
    }

    fn eq_name_lfn(&self, name: &str) -> bool {
        if let Some(lfn) = self.long_file_name_as_ucs2_units() {
            let self_decode_iter = char::decode_utf16(lfn.iter().copied());
            let mut other_uppercase_iter = name.chars().flat_map(char_to_uppercase);
            for decode_result in self_decode_iter {
                if let Ok(self_char) = decode_result {
                    for self_uppercase_char in char_to_uppercase(self_char) {
                        // compare each character in uppercase
                        if Some(self_uppercase_char) != other_uppercase_iter.next() {
                            return false;
                        }
                    }
                } else {
                    // decoding failed
                    return false;
                }
            }
            // both iterators should be at the end here
            other_uppercase_iter.next().is_none()
        } else {
            // entry has no long name
            false
        }
    }

    pub(crate) fn eq_name(&self, name: &str) -> bool {
        if self.eq_name_lfn(name) {
            return true;
        }
        self.short_name.eq_ignore_case(name)
    }

    pub(crate) async fn free_all_entries(&self, inner: &FatFile<T>) -> Result<(), Error> {
        for offset in self.offset_range.clone().step_by(DIR_ENTRY_SIZE as usize) {
            let mut buf = [0; DIR_ENTRY_SIZE as usize];
            inner
                .read_exact_at(offset as usize, &mut [&mut buf])
                .await?;
            let (_, mut data) = DirEntryData::parse(&buf)?;
            data.set_deleted();
            inner
                .write_all_at(offset as usize, &mut [&data.to_bytes()])
                .await?;
        }
        Ok(())
    }
}

impl<T: TimeProvider> fmt::Debug for DirEntry<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        self.data.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_with_ext() {
        assert_eq!(ShortName::new(b"FOO     BAR").to_string(), "FOO.BAR");
        assert_eq!(ShortName::new(b"LOOK AT M E").to_string(), "LOOK AT.M E");
        assert_eq!(
            ShortName::new(b"\x99OOK AT M \x99").to_string(),
            "\u{FFFD}OOK AT.M \u{FFFD}"
        );
        assert!(ShortName::new(b"\x99OOK AT M \x99").eq_ignore_case("\u{FFFD}OOK AT.M \u{FFFD}",));
    }

    #[test]
    fn short_name_without_ext() {
        assert_eq!(ShortName::new(b"FOO        ").to_string(), "FOO");
        assert_eq!(ShortName::new(b"LOOK AT    ").to_string(), "LOOK AT");
    }

    #[test]
    fn short_name_eq_ignore_case() {
        let raw_short_name: &[u8; SFN_SIZE] = b"\x99OOK AT M \x99";
        assert!(ShortName::new(raw_short_name).eq_ignore_case("\u{FFFD}OOK AT.M \u{FFFD}",));
        assert!(ShortName::new(raw_short_name).eq_ignore_case("\u{FFFD}ook AT.m \u{FFFD}",));
    }

    #[test]
    fn short_name_05_changed_to_e5() {
        let raw_short_name = [0x05; SFN_SIZE];
        assert_eq!(
            ShortName::new(&raw_short_name).as_bytes(),
            [0xE5, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, b'.', 0x05, 0x05, 0x05]
        );
    }

    #[test]
    fn lowercase_short_name() {
        let raw_short_name: &[u8; SFN_SIZE] = b"FOO     RS ";
        let mut raw_entry = DirFileEntryData {
            name: *raw_short_name,
            reserved_0: (1 << 3) | (1 << 4),
            ..DirFileEntryData::default()
        };
        assert_eq!(raw_entry.lowercase_name().to_string(), "foo.rs");
        raw_entry.reserved_0 = 1 << 3;
        assert_eq!(raw_entry.lowercase_name().to_string(), "foo.RS");
        raw_entry.reserved_0 = 1 << 4;
        assert_eq!(raw_entry.lowercase_name().to_string(), "FOO.rs");
        raw_entry.reserved_0 = 0;
        assert_eq!(raw_entry.lowercase_name().to_string(), "FOO.RS");
    }
}

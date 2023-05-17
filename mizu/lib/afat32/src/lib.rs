//! Async FAT 32 file system. Mainly inspired from
//! [`rust-fatfs`](https://github.com/rafalh/rust-fatfs).
#![cfg_attr(not(test), no_std)]
#![feature(split_array)]
#![feature(maybe_uninit_as_bytes)]
#![feature(maybe_uninit_slice)]

mod dir;
mod dirent;
mod file;
mod fs;
mod raw;
mod table;
mod time;

extern crate alloc;

pub use self::{
    dir::FatDir,
    dirent::{DirEntry, FileAttributes},
    file::FatFile,
    fs::{FatFileSystem, FatStats, FsStatusFlags},
    time::{Date, DateTime, DefaultTimeProvider, NullTimeProvider, Time, TimeProvider},
};

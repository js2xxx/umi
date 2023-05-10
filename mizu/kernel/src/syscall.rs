use core::ops::ControlFlow;

use co_trap::TrapFrame;
use ksc::{
    AHandlers,
    Scn::{self, *},
};
use spin::Lazy;
use sygnal::SigInfo;

use crate::task::{fd, Task, TaskState};

pub type ScParams<'a> = (&'a mut TaskState, &'a mut TrapFrame);
pub type ScRet = ControlFlow<i32, Option<SigInfo>>;

// TODO: Add handlers to the static.
pub static SYSCALL: Lazy<AHandlers<Scn, ScParams, ScRet>> = Lazy::new(|| {
    AHandlers::new()
        // Memory management
        .map(BRK, crate::mem::brk)
        // Tasks
        .map(EXIT, Task::exit)
        // FS operations
        .map(READ, fd::read)
        .map(WRITE, fd::write)
        .map(CHDIR, fd::chdir)
        .map(GETCWD, fd::getcwd)
        .map(DUP, fd::dup)
        .map(DUP3, fd::dup3)
        .map(OPENAT, fd::openat)
        .map(MKDIRAT, fd::mkdirat)
        .map(FSTAT, fd::fstat)
        .map(GETDENTS64, fd::getdents64)
        .map(UNLINKAT, fd::unlinkat)
        .map(CLOSE, fd::close)
});

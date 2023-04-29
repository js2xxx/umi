use co_trap::TrapFrame;
use ksc::{AHandlers, Scn};
use spin::Lazy;
use sygnal::SigInfo;

use crate::task::TaskState;

type ScParams<'a> = (&'a mut TaskState, &'a mut TrapFrame);

// TODO: Add handlers to the static.
pub static SYSCALL: Lazy<AHandlers<Scn, ScParams, Result<(), SigInfo>>> = Lazy::new(AHandlers::new);

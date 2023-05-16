use core::mem;

use array_macro::array;
use rv39_paging::LAddr;
use spin::Mutex;

use crate::{Sig, SigSet, NR_SIGNALS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    Ignore,
    Kill,
    Suspend,
    Resume,
    User {
        entry: LAddr,
        exit: LAddr,
        use_extra_cx: bool,
        use_alt_stack: bool,
    },
}

impl ActionType {
    pub const fn default(sig: Sig) -> Self {
        use ActionType::*;
        match sig {
            Sig::SIGCHLD | Sig::SIGURG => Ignore,
            Sig::SIGSTOP => Suspend,
            Sig::SIGCONT => Resume,
            _ => ActionType::Kill,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub ty: ActionType,
    pub mask: SigSet,
}

impl Action {
    pub const fn default(sig: Sig) -> Self {
        Action {
            ty: ActionType::default(sig),
            mask: SigSet::EMPTY,
        }
    }
}

pub struct ActionSet {
    data: [Mutex<Action>; NR_SIGNALS],
}

impl ActionSet {
    pub const fn new() -> Self {
        ActionSet {
            data: array![
                index => match Sig::from_index(index) {
                    Some(sig) => Mutex::new(Action::default(sig)),
                    None => panic!("unsupported index for signal")
                };
                NR_SIGNALS
            ],
        }
    }

    pub fn get(&self, sig: Sig) -> Action {
        ksync::critical(|| *self.data[sig.index()].lock())
    }

    pub fn replace(&self, sig: Sig, new: Action) -> Action {
        ksync::critical(|| {
            let mut action = self.data[sig.index()].lock();
            let old = mem::replace(&mut *action, new);
            if sig.should_never_capture() {
                *action = Action::default(sig);
            }
            old
        })
    }

    pub fn deep_fork(&self) -> Self {
        ActionSet {
            data: array![
                index => Mutex::new(ksync::critical(|| *self.data[index].lock()));
                NR_SIGNALS
            ],
        }
    }
}

impl const Default for ActionSet {
    fn default() -> Self {
        Self::new()
    }
}

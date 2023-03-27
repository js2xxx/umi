use core::{
    fmt,
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Sig(i32);

pub const NR_SIGNALS: usize = 64;

impl TryFrom<usize> for Sig {
    type Error = FromIndexError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Self::from_index(value).ok_or(FromIndexError(value))
    }
}

#[derive(Debug)]
pub struct FromIndexError(usize);

impl fmt::Display for FromIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported index for signal: {}", self.0)
    }
}

impl Sig {
    /// Hangup detected on controlling terminal or death of controlling process
    pub const SIGHUP: Sig = Sig(1);
    /// Interrupt from keyboard
    pub const SIGINT: Sig = Sig(2);
    /// Quit from keyboard
    pub const SIGQUIT: Sig = Sig(3);
    /// Illegal Instruction
    pub const SIGILL: Sig = Sig(4);
    /// Trace/breakpoint trap
    pub const SIGTRAP: Sig = Sig(5);
    /// Abort signal
    pub const SIGABRT: Sig = Sig(6);
    /// Bus error (bad memory access)
    pub const SIGBUS: Sig = Sig(7);
    /// Floating-point exception
    pub const SIGFPE: Sig = Sig(8);
    /// Kill signal
    pub const SIGKILL: Sig = Sig(9);
    /// User-defined signal 1
    pub const SIGUSR1: Sig = Sig(10);
    /// Invalid memory reference
    pub const SIGSEGV: Sig = Sig(11);
    /// User-defined signal 2
    pub const SIGUSR2: Sig = Sig(12);
    /// Broken pipe: write to pipe with no readers
    pub const SIGPIPE: Sig = Sig(13);
    /// Timer signal
    pub const SIGALRM: Sig = Sig(14);
    /// Termination signal
    pub const SIGTERM: Sig = Sig(15);
    /// Child stopped or terminated
    pub const SIGCHLD: Sig = Sig(17);
    /// Continue if stopped
    pub const SIGCONT: Sig = Sig(18);
    /// Stop process (suspend)
    pub const SIGSTOP: Sig = Sig(19);
    /// Stop typed at terminal
    pub const SIGTSTP: Sig = Sig(20);
    /// Terminal input for background process
    pub const SIGTTIN: Sig = Sig(21);
    /// Terminal output for background process
    pub const SIGTTOU: Sig = Sig(22);
    /// Urgent condition on socket
    pub const SIGURG: Sig = Sig(23);
    /// CPU time limit exceeded
    pub const SIGXCPU: Sig = Sig(24);
    /// File size limit exceeded
    pub const SIGXFSZ: Sig = Sig(25);
    pub const SIGVTALRM: Sig = Sig(26);
    /// Profiling timer expired
    pub const SIGPROF: Sig = Sig(27);
    /// Virtual alarm clock
    pub const SIGWINCH: Sig = Sig(28);
    /// I/O now possible
    pub const SIGIO: Sig = Sig(29);
    /// Power failure (System V)
    pub const SIGPWR: Sig = Sig(30);
    /// Bad system call
    pub const SIGSYS: Sig = Sig(31);

    pub const SIG_LEGACY_MAX: Sig = Sig(32);

    pub const SIG_MAX: Sig = Sig(64);

    pub const fn from_index(index: usize) -> Option<Self> {
        if index < NR_SIGNALS {
            Some(Sig(index as i32 + 1))
        } else {
            None
        }
    }

    pub const fn is_legacy(&self) -> bool {
        self.0 <= Self::SIG_LEGACY_MAX.0
    }

    pub const fn new(sig: i32) -> Option<Self> {
        (sig <= Self::SIG_MAX.0).then_some(Self(sig))
    }

    pub const fn mask(&self) -> u64 {
        1 << (self.0 - 1)
    }

    pub const fn index(&self) -> usize {
        (self.0 - 1) as usize
    }

    pub const fn should_never_capture(self) -> bool {
        matches!(self, Sig::SIGKILL | Sig::SIGSTOP)
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct SigSet(u64);

impl const From<u64> for SigSet {
    fn from(value: u64) -> Self {
        SigSet(value)
    }
}

impl const From<SigSet> for u64 {
    fn from(value: SigSet) -> Self {
        value.0
    }
}

impl const From<Sig> for SigSet {
    fn from(value: Sig) -> Self {
        SigSet(value.mask())
    }
}

impl SigSet {
    pub const EMPTY: SigSet = SigSet(0);

    pub const fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub const fn contains<S: ~const Into<Self>>(self, other: S) -> bool {
        let other: Self = other.into();
        (!self & other).0 == 0
    }

    pub const fn contains_index(self, other: usize) -> bool {
        let other = match Sig::from_index(other) {
            Some(other) => other,
            None => return false,
        };
        self.contains(other)
    }
}

macro_rules! impl_binary {
    ($trait:ident, $rhs_ty:ident, $func:ident => | $l:ident, $r:ident | $expr:expr) => {
        impl const $trait<$rhs_ty> for SigSet {
            type Output = Self;

            fn $func(self, rhs: $rhs_ty) -> Self {
                let $l = self;
                let $r = rhs;
                $expr
            }
        }
    };
}

macro_rules! impl_binary_assign {
    ($trait:ident, $rhs_ty:ident, $func:ident => | $l:ident, $r:ident | $expr:expr) => {
        impl $trait<$rhs_ty> for SigSet {
            fn $func(&mut self, rhs: $rhs_ty) {
                let $l = self;
                let $r = rhs;
                $expr
            }
        }
    };
}

impl_binary!(BitOr, Sig, bitor => |x, y| SigSet(x.0 | y.mask()));
impl_binary!(BitAnd, Sig, bitand => |x, y| SigSet(x.0 & y.mask()));
impl_binary!(BitXor, Sig, bitxor => |x, y| SigSet(x.0 ^ y.mask()));
impl_binary!(BitOr, SigSet, bitor => |x, y| SigSet(x.0 | y.0));
impl_binary!(BitAnd, SigSet, bitand => |x, y| SigSet(x.0 & y.0));
impl_binary!(BitXor, SigSet, bitxor => |x, y| SigSet(x.0 ^ y.0));

impl_binary_assign!(BitOrAssign, Sig, bitor_assign => |x, y| x.0 |= y.mask());
impl_binary_assign!(BitAndAssign, Sig, bitand_assign => |x, y| x.0 &= y.mask());
impl_binary_assign!(BitXorAssign, Sig, bitxor_assign => |x, y| x.0 ^= y.mask());
impl_binary_assign!(BitOrAssign, SigSet, bitor_assign => |x, y| x.0 |= y.0);
impl_binary_assign!(BitAndAssign, SigSet, bitand_assign => |x, y| x.0 &= y.0);
impl_binary_assign!(BitXorAssign, SigSet, bitxor_assign => |x, y| x.0 ^= y.0);

impl const Not for SigSet {
    type Output = Self;

    fn not(self) -> Self::Output {
        SigSet(!self.0)
    }
}

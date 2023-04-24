use enum_primitive_derive::Primitive;
pub use Scn::*;

#[allow(non_camel_case_types)]
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Primitive)]
#[repr(u16)]
pub enum Scn {
    #[cfg(feature = "test")]
    __TEST0 = 0,
    #[cfg(feature = "test")]
    __TEST1 = 1,
    #[cfg(feature = "test")]
    __TEST2 = 2,

    GETCWD = 17,
    DUP = 23,
    DUP3 = 24,
    FCNTL = 25,
    IOCTL = 29,
    MKDIRAT = 34,
    UNLINKAT = 35,
    UMOUNT2 = 39,
    MOUNT = 40,
    STATFS = 43,
    FACCESSAT = 48,
    CHDIR = 49,
    FCHOWN = 55,
    OPENAT = 56,
    CLOSE = 57,
    PIPE2 = 59,
    GETDENTS64 = 61,
    LSEEK = 62,
    READ = 63,
    WRITE = 64,
    READV = 65,
    WRITEV = 66,
    PREAD64 = 67,
    SENDFILE = 71,
    PSELECT6 = 72,
    PPOLL = 73,
    READLINKAT = 78,
    NEWFSTATAT = 79,
    FSTAT = 80,
    FSYNC = 82,
    UTIMENSAT = 88,
    EXIT = 93,
    EXIT_GROUP = 94,
    SET_TID_ADDRESS = 96,
    FUTEX = 98,
    SET_ROBUST_LIST = 99,
    GET_ROBUST_LIST = 100,
    NANOSLEEP = 101,
    SETITIMER = 103,
    CLOCK_GETTIME = 113,
    SYSLOG = 116,
    SCHED_YIELD = 124,
    KILL = 129,
    TKILL = 130,
    TGKILL = 131,
    SIGALTSTACK = 132,
    RT_SIGSUSPEND = 133,
    RT_SIGACTION = 134,
    RT_SIGPROCMASK = 135,
    RT_SIGPENDING = 136,
    RT_SIGTIMEDWAIT = 137,
    RT_SIGQUEUEINFO = 138,
    RT_SIGRETURN = 139,
    TIMES = 153,
    SETPGID = 154,
    GETPGID = 155,
    UNAME = 160,
    GETRUSAGE = 165,
    UMASK = 166,
    GETTIMEOFDAY = 169,
    GETPID = 172,
    GETPPID = 173,
    GETUID = 174,
    GETEUID = 175,
    GETEGID = 177,
    GETTID = 178,
    SYSINFO = 179,
    SOCKET = 198,
    BIND = 200,
    LISTEN = 201,
    ACCEPT = 202,
    CONNECT = 203,
    GETSOCKNAME = 204,
    SENDTO = 206,
    RECVFROM = 207,
    SETSOCKOPT = 208,
    BRK = 214,
    MUNMAP = 215,
    CLONE = 220,
    EXECVE = 221,
    MMAP = 222,
    MPROTECT = 226,
    MSYNC = 227,
    WAIT4 = 260,
    PRLIMIT64 = 261,
    RENAMEAT2 = 276,
    GETRANDOM = 278,
    MEMBARRIER = 283,
}
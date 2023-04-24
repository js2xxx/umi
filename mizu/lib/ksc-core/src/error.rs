use core::fmt;

use enum_primitive_derive::Primitive;
use num_traits::FromPrimitive;
pub use Error::*;

use crate::RawReg;

pub type Result<T = ()> = core::result::Result<T, Error>;

#[allow(clippy::upper_case_acronyms)]
#[repr(isize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Primitive)]
pub enum Error {
    /// Success.
    EUNDEF = 0,
    /// Operation not permitted.
    EPERM = 1,
    /// No such file or directory.
    ENOENT = 2,
    /// No such process.
    ESRCH = 3,
    /// Interrupted system call.
    EINTR = 4,
    /// I/O error.
    EIO = 5,
    /// No such device or address.
    ENXIO = 6,
    /// Argument list too long.
    E2BIG = 7,
    /// Exec format error.
    ENOEXEC = 8,
    /// Bad file number.
    EBADF = 9,
    /// No child processes.
    ECHILD = 10,
    /// Try again.
    EAGAIN = 11,
    /// Out of memory.
    ENOMEM = 12,
    /// Permission denied.
    EACCES = 13,
    /// Bad address.
    EFAULT = 14,
    /// Block device required.
    ENOTBLK = 15,
    /// Device or resource busy.
    EBUSY = 16,
    /// File exists.
    EEXIST = 17,
    /// Cross-device link.
    EXDEV = 18,
    /// No such device.
    ENODEV = 19,
    /// Not a directory.
    ENOTDIR = 20,
    /// Is a directory.
    EISDIR = 21,
    /// Invalid argument.
    EINVAL = 22,
    /// File table overflow.
    ENFILE = 23,
    /// Too many open files.
    EMFILE = 24,
    /// Not a typewriter.
    ENOTTY = 25,
    /// Text file busy.
    ETXTBSY = 26,
    /// File too large.
    EFBIG = 27,
    /// No space left on device.
    ENOSPC = 28,
    /// Illegal seek.
    ESPIPE = 29,
    /// Read-only file system.
    EROFS = 30,
    /// Too many links.
    EMLINK = 31,
    /// Broken pipe.
    EPIPE = 32,
    /// Math argument out of domain of func.
    EDOM = 33,
    /// Math result not representable.
    ERANGE = 34,
    /// Resource deadlock would occur.
    EDEADLK = 35,
    /// File name too long.
    ENAMETOOLONG = 36,
    /// No record locks available.
    ENOLCK = 37,
    /// Function not implemented.
    ENOSYS = 38,
    /// Directory not empty.
    ENOTEMPTY = 39,
    /// Too many symbolic links encountered.
    ELOOP = 40,
    /// No message of desired type.
    ENOMSG = 42,
    /// Identifier removed.
    EIDRM = 43,
    /// Channel number out of range.
    ECHRNG = 44,
    /// Level 2 not synchronized.
    EL2NSYNC = 45,
    /// Level 3 halted.
    EL3HLT = 46,
    /// Level 3 reset.
    EL3RST = 47,
    /// Link number out of range.
    ELNRNG = 48,
    /// Protocol driver not attached.
    EUNATCH = 49,
    /// No CSI structure available.
    ENOCSI = 50,
    /// Level 2 halted.
    EL2HLT = 51,
    /// Invalid exchange.
    EBADE = 52,
    /// Invalid request descriptor.
    EBADR = 53,
    /// Exchange full.
    EXFULL = 54,
    /// No anode.
    ENOANO = 55,
    /// Invalid request code.
    EBADRQC = 56,
    /// Invalid slot.
    EBADSLT = 57,
    /// Bad font file format.
    EBFONT = 59,
    /// Device not a stream.
    ENOSTR = 60,
    /// No data available.
    ENODATA = 61,
    /// Timer expired.
    ETIME = 62,
    /// Out of streams resources.
    ENOSR = 63,
    /// Machine is not on the network.
    ENONET = 64,
    /// Package not installed.
    ENOPKG = 65,
    /// Object is remote.
    EREMOTE = 66,
    /// Link has been severed.
    ENOLINK = 67,
    /// Advertise error.
    EADV = 68,
    /// Srmount error.
    ESRMNT = 69,
    /// Communication error on send.
    ECOMM = 70,
    /// Protocol error.
    EPROTO = 71,
    /// Multihop attempted.
    EMULTIHOP = 72,
    /// RFS specific error.
    EDOTDOT = 73,
    /// Not a data message.
    EBADMSG = 74,
    /// Value too large for defined data type.
    EOVERFLOW = 75,
    /// Name not unique on network.
    ENOTUNIQ = 76,
    /// File descriptor in bad state.
    EBADFD = 77,
    /// Remote address changed.
    EREMCHG = 78,
    /// Can not access a needed shared library.
    ELIBACC = 79,
    /// Accessing a corrupted shared library.
    ELIBBAD = 80,
    /// .lib section in a.out corrupted.
    ELIBSCN = 81,
    /// Attempting to link in too many shared libraries.
    ELIBMAX = 82,
    /// Cannot exec a shared library directly.
    ELIBEXEC = 83,
    /// Illegal byte sequence.
    EILSEQ = 84,
    /// Interrupted system call should be restarted.
    ERESTART = 85,
    /// Streams pipe error.
    ESTRPIPE = 86,
    /// Too many users.
    EUSERS = 87,
    /// Socket operation on non-socket.
    ENOTSOCK = 88,
    /// Destination address required.
    EDESTADDRREQ = 89,
    /// Message too long.
    EMSGSIZE = 90,
    /// Protocol wrong type for socket.
    EPROTOTYPE = 91,
    /// Protocol not available.
    ENOPROTOOPT = 92,
    /// Protocol not supported.
    EPROTONOSUPPORT = 93,
    /// Socket type not supported.
    ESOCKTNOSUPPORT = 94,
    /// Operation not supported on transport endpoint.
    EOPNOTSUPP = 95,
    /// Protocol family not supported.
    EPFNOSUPPORT = 96,
    /// Address family not supported by protocol.
    EAFNOSUPPORT = 97,
    /// Address already in use.
    EADDRINUSE = 98,
    /// Cannot assign requested address.
    EADDRNOTAVAIL = 99,
    /// Network is down.
    ENETDOWN = 100,
    /// Network is unreachable.
    ENETUNREACH = 101,
    /// Network dropped connection because of reset.
    ENETRESET = 102,
    /// Software caused connection abort.
    ECONNABORTED = 103,
    /// Connection reset by peer.
    ECONNRESET = 104,
    /// No buffer space available.
    ENOBUFS = 105,
    /// Transport endpoint is already connected.
    EISCONN = 106,
    /// Transport endpoint is not connected.
    ENOTCONN = 107,
    /// Cannot send after transport endpoint shutdown.
    ESHUTDOWN = 108,
    /// Too many references: cannot splice.
    ETOOMANYREFS = 109,
    /// Time out.
    ETIMEDOUT = 110,
    /// Connection refused.
    ECONNREFUSED = 111,
    /// Host is down.
    EHOSTDOWN = 112,
    /// No route to host.
    EHOSTUNREACH = 113,
    /// Operation already in progress.
    EALREADY = 114,
    /// Operation now in progress.
    EINPROGRESS = 115,
    /// Stale file handle.
    ESTALE = 116,
    /// Structure needs cleaning.
    EUCLEAN = 117,
    /// Not a XENIX named type file.
    ENOTNAM = 118,
    /// No XENIX semaphores available.
    ENAVAIL = 119,
    /// Is a named type file.
    EISNAM = 120,
    /// Remote I/O error.
    EREMOTEIO = 121,
    /// Quota exceeded.
    EDQUOT = 122,
    /// No medium found.
    ENOMEDIUM = 123,
    /// Wrong medium type.
    EMEDIUMTYPE = 124,
    /// Operation Canceled.
    ECANCELED = 125,
    /// Required key not available.
    ENOKEY = 126,
    /// Key has expired.
    EKEYEXPIRED = 127,
    /// Key has been revoked.
    EKEYREVOKED = 128,
    /// Key was rejected by service.
    EKEYREJECTED = 129,
    /// Owner died.
    EOWNERDEAD = 130,
    /// State not recoverable.
    ENOTRECOVERABLE = 131,
    /// Operation not possible due to RF-kill.
    ERFKILL = 132,
    /// Memory page has hardware error.
    EHWPOISON = 133,
}

impl From<core::alloc::LayoutError> for Error {
    #[inline]
    fn from(_: core::alloc::LayoutError) -> Self {
        ENOMEM
    }
}

impl From<core::num::TryFromIntError> for Error {
    #[inline]
    fn from(_: core::num::TryFromIntError) -> Self {
        EINVAL
    }
}

impl From<core::str::Utf8Error> for Error {
    #[inline]
    fn from(_: core::str::Utf8Error) -> Self {
        EINVAL
    }
}

impl From<rv39_paging::Error> for Error {
    fn from(error: rv39_paging::Error) -> Self {
        match error {
            rv39_paging::Error::OutOfMemory => ENOMEM,
            rv39_paging::Error::AddrMisaligned { .. } => EINVAL,
            rv39_paging::Error::RangeEmpty => EINVAL,
            rv39_paging::Error::EntryExistent(true) => EEXIST,
            rv39_paging::Error::EntryExistent(false) => ENOENT,
            rv39_paging::Error::PermissionDenied => EPERM,
        }
    }
}

impl Error {
    fn try_from_raw(raw: usize) -> Option<Self> {
        FromPrimitive::from_isize(-(raw as isize))
    }

    pub fn into_raw(self) -> usize {
        -(self as isize) as _
    }
}

impl<T: RawReg> RawReg for Result<T> {
    fn from_raw(raw: usize) -> Self {
        Error::try_from_raw(raw).map_or(Ok(T::from_raw(raw)), Err)
    }

    fn into_raw(self) -> usize {
        match self {
            Ok(v) => v.into_raw(),
            Err(e) => e.into_raw(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            EUNDEF => "Success",
            EPERM => "Operation not permitted",
            ENOENT => "No such file or directory",
            ESRCH => "No such process",
            EINTR => "Interrupted system call",
            EIO => "I/O error",
            ENXIO => "No such device or address",
            E2BIG => "Argument list too long",
            ENOEXEC => "Exec format error",
            EBADF => "Bad file number",
            ECHILD => "No child processes",
            EAGAIN => "Try again",
            ENOMEM => "Out of memory",
            EACCES => "Permission denied",
            EFAULT => "Bad address",
            ENOTBLK => "Block device required",
            EBUSY => "Device or resource busy",
            EEXIST => "File exists",
            EXDEV => "Cross-device link",
            ENODEV => "No such device",
            ENOTDIR => "Not a directory",
            EISDIR => "Is a directory",
            EINVAL => "Invalid argument",
            ENFILE => "File table overflow",
            EMFILE => "Too many open files",
            ENOTTY => "Not a typewriter",
            ETXTBSY => "Text file busy",
            EFBIG => "File too large",
            ENOSPC => "No space left on device",
            ESPIPE => "Illegal seek",
            EROFS => "Read-only file system",
            EMLINK => "Too many links",
            EPIPE => "Broken pipe",
            EDOM => "Math argument out of domain of func",
            ERANGE => "Math result not representable",
            EDEADLK => "Resource deadlock would occur",
            ENAMETOOLONG => "File name too long",
            ENOLCK => "No record locks available",
            ENOSYS => "Function not implemented",
            ENOTEMPTY => "Directory not empty",
            ELOOP => "Too many symbolic links encountered",
            ENOMSG => "No message of desired type",
            EIDRM => "Identifier removed",
            ECHRNG => "Channel number out of range",
            EL2NSYNC => "Level 2 not synchronized",
            EL3HLT => "Level 3 halted",
            EL3RST => "Level 3 reset",
            ELNRNG => "Link number out of range",
            EUNATCH => "Protocol driver not attached",
            ENOCSI => "No CSI structure available",
            EL2HLT => "Level 2 halted",
            EBADE => "Invalid exchange",
            EBADR => "Invalid request descriptor",
            EXFULL => "Exchange full",
            ENOANO => "No anode",
            EBADRQC => "Invalid request code",
            EBADSLT => "Invalid slot",
            EBFONT => "Bad font file format",
            ENOSTR => "Device not a stream",
            ENODATA => "No data available",
            ETIME => "Timer expired",
            ENOSR => "Out of streams resources",
            ENONET => "Machine is not on the network",
            ENOPKG => "Package not installed",
            EREMOTE => "Object is remote",
            ENOLINK => "Link has been severed",
            EADV => "Advertise error",
            ESRMNT => "Srmount error",
            ECOMM => "Communication error on send",
            EPROTO => "Protocol error",
            EMULTIHOP => "Multihop attempted",
            EDOTDOT => "RFS specific error",
            EBADMSG => "Not a data message",
            EOVERFLOW => "Value too large for defined data type",
            ENOTUNIQ => "Name not unique on network",
            EBADFD => "File descriptor in bad state",
            EREMCHG => "Remote address changed",
            ELIBACC => "Can not access a needed shared library",
            ELIBBAD => "Accessing a corrupted shared library",
            ELIBSCN => ".lib section in a.out corrupted",
            ELIBMAX => "Attempting to link in too many shared libraries",
            ELIBEXEC => "Cannot exec a shared library directly",
            EILSEQ => "Illegal byte sequence",
            ERESTART => "Interrupted system call should be restarted",
            ESTRPIPE => "Streams pipe error",
            EUSERS => "Too many users",
            ENOTSOCK => "Socket operation on non-socket",
            EDESTADDRREQ => "Destination address required",
            EMSGSIZE => "Message too long",
            EPROTOTYPE => "Protocol wrong type for socket",
            ENOPROTOOPT => "Protocol not available",
            EPROTONOSUPPORT => "Protocol not supported",
            ESOCKTNOSUPPORT => "Socket type not supported",
            EOPNOTSUPP => "Operation not supported on transport endpoint",
            EPFNOSUPPORT => "Protocol family not supported",
            EAFNOSUPPORT => "Address family not supported by protocol",
            EADDRINUSE => "Address already in use",
            EADDRNOTAVAIL => "Cannot assign requested address",
            ENETDOWN => "Network is down",
            ENETUNREACH => "Network is unreachable",
            ENETRESET => "Network dropped connection because of reset",
            ECONNABORTED => "Software caused connection abort",
            ECONNRESET => "Connection reset by peer",
            ENOBUFS => "No buffer space available",
            EISCONN => "Transport endpoint is already connected",
            ENOTCONN => "Transport endpoint is not connected",
            ESHUTDOWN => "Cannot send after transport endpoint shutdown",
            ETOOMANYREFS => "Too many references: cannot splice",
            ETIMEDOUT => "Time out",
            ECONNREFUSED => "Connection refused",
            EHOSTDOWN => "Host is down",
            EHOSTUNREACH => "No route to host",
            EALREADY => "Operation already in progress",
            EINPROGRESS => "Operation now in progress",
            ESTALE => "Stale file handle",
            EUCLEAN => "Structure needs cleaning",
            ENOTNAM => "Not a XENIX named type file",
            ENAVAIL => "No XENIX semaphores available",
            EISNAM => "Is a named type file",
            EREMOTEIO => "Remote I/O error",
            EDQUOT => "Quota exceeded",
            ENOMEDIUM => "No medium found",
            EMEDIUMTYPE => "Wrong medium type",
            ECANCELED => "Operation Canceled",
            ENOKEY => "Required key not available",
            EKEYEXPIRED => "Key has expired",
            EKEYREVOKED => "Key has been revoked",
            EKEYREJECTED => "Key was rejected by service",
            EOWNERDEAD => "Owner died",
            ENOTRECOVERABLE => "State not recoverable",
            ERFKILL => "Operation not possible due to RF-kill",
            EHWPOISON => "Memory page has hardware error",
        };
        f.write_str(msg)
    }
}

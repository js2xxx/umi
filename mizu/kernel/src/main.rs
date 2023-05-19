#![cfg_attr(not(feature = "test"), no_std)]
#![cfg_attr(not(feature = "test"), no_main)]
#![feature(alloc_layout_extra)]
#![feature(asm_const)]
#![feature(const_mut_refs)]
#![feature(const_trait_impl)]
#![feature(inline_const)]
#![feature(maybe_uninit_as_bytes)]
#![feature(naked_functions)]
#![feature(pointer_is_aligned)]
#![feature(result_option_inspect)]
#![feature(thread_local)]

mod dev;
pub mod fs;
mod mem;
mod rxx;
mod syscall;
pub mod task;
mod trap;

#[macro_use]
extern crate klog;

extern crate alloc;

use alloc::{
    sync::{Arc, Weak},
    vec,
};

use umifs::types::{OpenOptions, Permissions};

pub use self::rxx::executor;
use crate::task::InitTask;

async fn main(fdt: usize) {
    println!("Hello from UMI ^_^");

    mem::test_phys().await;

    unsafe { dev::init(fdt as _).expect("failed to initialize devices") };
    fs::fs_init().await;

    let (fs, _) = fs::get("".as_ref()).unwrap();
    let rt = fs.root_dir().await.unwrap();

    let spec = [
        "argv",
        "basename",
        "clocale_mbfuncs",
        "clock_gettime",
        "crypt",
        "dirname",
        "env",
        "fdopen",
        "fnmatch",
        "fscanf",
        "fwscanf",
        "iconv_open",
        "inet_pton",
        "mbc",
        "memstream",
        "pthread_cancel_points",
        "pthread_cancel",
        "pthread_cond",
        "pthread_tsd",
        "qsort",
        "random",
        "search_hsearch",
        "search_insque",
        "search_lsearch",
        "search_tsearch",
        "setjmp",
        "snprintf",
        "socket",
        "sscanf",
        "sscanf_long",
        "stat",
        "strftime",
        "string",
        "string_memcpy",
        "string_memmem",
        "string_memset",
        "string_strchr",
        "string_strcspn",
        "string_strstr",
        "strptime",
        "strtod",
        "strtod_simple",
        "strtof",
        "strtol",
        "strtold",
        "swprintf",
        "tgmath",
        "time",
        "tls_align",
        "udiv",
        "ungetc",
        "utime",
        "wcsstr",
        "wcstol",
        "pleval",
        "daemon_failure",
        "dn_expand_empty",
        "dn_expand_ptr_0",
        "fflush_exit",
        "fgets_eof",
        "fgetwc_buffering",
        "flockfile_list",
        "fpclassify_invalid_ld80",
        "ftello_unflushed_append",
        "getpwnam_r_crash",
        "getpwnam_r_errno",
        "iconv_roundtrips",
        "inet_ntop_v4mapped",
        "inet_pton_empty_last_field",
        "iswspace_null",
        "lrand48_signextend",
        "lseek_large",
        "malloc_0",
        "mbsrtowcs_overflow",
        "memmem_oob_read",
        "memmem_oob",
        "mkdtemp_failure",
        "mkstemp_failure",
        "printf_1e9_oob",
        "printf_fmt_g_round",
        "printf_fmt_g_zeros",
        "printf_fmt_n",
        "pthread_robust_detach",
        "pthread_cancel_sem_wait",
        "pthread_cond_smasher",
        "pthread_condattr_setclock",
        "pthread_exit_cancel",
        "pthread_once_deadlock",
        "pthread_rwlock_ebusy",
        "putenv_doublefree",
        "regex_backref_0",
        "regex_bracket_icase",
        "regex_ere_backref",
        "regex_escaped_high_byte",
        "regex_negated_range",
        "regexec_nosub",
        "rewind_clear_error",
        "rlimit_open_files",
        "scanf_bytes_consumed",
        "scanf_match_literal_eof",
        "scanf_nullbyte_char",
        "setvbuf_unget",
        "sigprocmask_internal",
        "sscanf_eof",
        "statvfs",
        "strverscmp",
        "syscall_sign_extend",
        "uselocale_0",
        "wcsncpy_read_overflow",
        "wcsstr_false_negative",
    ];

    let oo = OpenOptions::RDONLY;
    let perm = Permissions::all_same(true, false, true);

    rt.clone()
        .open("entry-static".as_ref(), oo, perm)
        .await
        .unwrap();

    let (runner, _) = rt.open("runtest".as_ref(), oo, perm).await.unwrap();
    let runner = Arc::new(mem::new_phys(runner.to_io().unwrap(), true));

    log::warn!("Start testing");
    for case in spec {
        log::info!("Running test case {case:?}");

        let task = InitTask::from_elf(
            Weak::new(),
            &runner,
            crate::mem::new_virt(),
            Default::default(),
            vec![
                "runtest".into(),
                "-w".into(),
                "entry-static".into(),
                case.into(),
            ],
        )
        .await
        .unwrap();
        let task = task.spawn().unwrap();
        let code = task.wait().await;
        log::info!("test case {case:?} returned with {code:?}\n");
    }

    log::warn!("Goodbye!");
}

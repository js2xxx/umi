#![forbid(unsafe_code)]

use array_macro::array;
use std::cell::Cell;
use std::convert::TryFrom;
use std::fmt::Debug;
use std::future::{pending, Future};
use std::num::TryFromIntError;
use std::panic::catch_unwind;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::task::{Context, Poll};

#[test]
fn simple_array() {
    assert_eq!(array![3; 5], [3, 3, 3, 3, 3]);
}

#[test]
fn callback_array() {
    assert_eq!(array![x => x * 2; 3], [0, 2, 4]);
}

#[test]
fn outer_scope() {
    let x = 1;
    assert_eq!(array![x; 3], [1, 1, 1]);
}

#[test]
fn mutability() {
    let mut x = 1;
    assert_eq!(
        array![_ => {
            x += 1;
            x
        }; 3],
        [2, 3, 4]
    );
}

#[test]
fn big_array() {
    assert_eq!(&array!["x"; 333] as &[_], &["x"; 333] as &[_]);
}

#[test]
fn macro_within_macro() {
    assert_eq!(
        array![x => array![y => (x, y); 2]; 3],
        [[(0, 0), (0, 1)], [(1, 0), (1, 1)], [(2, 0), (2, 1)]]
    );
}

#[test]
fn const_expr() {
    const TWO: usize = 2;
    assert_eq!(array![i => i; 2 + TWO], [0, 1, 2, 3]);
}

#[test]
fn panic_safety() {
    static CALLED_DROP: AtomicBool = AtomicBool::new(false);

    struct DontDrop;
    impl Drop for DontDrop {
        fn drop(&mut self) {
            CALLED_DROP.store(true, Relaxed);
        }
    }
    fn panicky() -> DontDrop {
        panic!();
    }
    assert!(catch_unwind(|| array![_ => panicky(); 2]).is_err());
    assert_eq!(CALLED_DROP.load(Relaxed), false);
}

#[test]
fn panic_safety_part_two() {
    static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

    struct DropOnlyThrice;
    impl Drop for DropOnlyThrice {
        fn drop(&mut self) {
            DROP_COUNT.fetch_add(1, Relaxed);
        }
    }
    fn panicky(i: usize) -> DropOnlyThrice {
        if i == 3 {
            panic!();
        }
        DropOnlyThrice
    }
    assert!(catch_unwind(|| array![i => panicky(i); 555]).is_err());
    assert_eq!(DROP_COUNT.load(Relaxed), 3);
}

#[test]
fn array_of_void() {
    fn internal<T: Debug + Eq>(f: fn() -> T) {
        let a: [T; 0] = array![_ => f(); 0];
        assert_eq!(a, []);
    }
    internal(|| -> ! { panic!("This function shouldn't be called") });
}

#[should_panic]
#[test]
fn array_of_void_panic_safety() {
    fn internal<T: Debug + Eq>(f: fn() -> T) {
        let _a: [T; 1] = array![_ => f(); 1];
    }
    internal(|| -> ! { panic!() });
}

#[test]
fn malicious_length() {
    trait Evil {
        fn length(&self) -> *mut usize;
    }
    impl<T> Evil for T {
        fn length(&self) -> *mut usize {
            42 as *mut usize
        }
    }
    assert_eq!(array![1; 3], [1, 1, 1]);
}

#[test]
fn return_in_array() {
    assert_eq!(
        (|| {
            array![x => if x == 1 { return 42 } else { String::from("Allocation") }; 4];
            unreachable!();
        })(),
        42,
    );
}

#[test]
fn question_mark() {
    assert!((|| -> Result<[String; 129], TryFromIntError> {
        Ok(array![x => i8::try_from(x)?.to_string(); 129])
    })()
    .is_err())
}

#[test]
fn const_array() {
    const fn const_fn() -> u32 {
        0
    }
    const ARRAY: [u32; 4] = array![_ => const_fn(); 4];
    assert_eq!(ARRAY, [0; 4]);
}

#[tokio::test]
async fn await_array() {
    let array = array![async { 42 }.await; 3];
    assert_eq!(array, [42, 42, 42]);
}

#[tokio::test]
async fn cancel_in_middle() {
    struct ImmediatePoll<F>(F);
    impl<F> Future for ImmediatePoll<F>
    where
        F: Future + Unpin,
    {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            assert!(Pin::new(&mut self.0).poll(cx).is_pending());
            Poll::Ready(())
        }
    }

    let allocated = Cell::new(false);
    let fut = async {
        array![x => if x == 3 {
            pending().await
        } else {
            allocated.set(true);
            String::from("Allocation")
        }; 4]
    };
    tokio::pin!(fut);
    ImmediatePoll(fut).await;
    assert!(allocated.get());
}

#[tokio::test]
async fn async_send_sync() {
    fn ret_fut() -> impl Future<Output = [(); 4]> + Send + Sync {
        async { array![_ => async { }.await; 4] }
    }
    assert_eq!(ret_fut().await, [(); 4]);
}

#[test]
fn const_generics() {
    fn array<const N: usize>() -> [u8; N] {
        array![0; N]
    }
    fn array_pos<const N: usize>() -> [usize; N] {
        array![x => x; N]
    }
    assert_eq!(array(), [0, 0, 0]);
    assert_eq!(array(), [0, 0, 0, 0, 0]);
    assert_eq!(array_pos(), [0, 1, 2, 3, 4, 5, 6]);
}

#[test]
fn generic_const_array() {
    const fn get_array<T>() -> [Option<T>; 3] {
        array![_ => None; 3]
    }
    const ARRAY: [Option<String>; 3] = get_array();
    assert_eq!(ARRAY, [None, None, None]);
}

#[test]
fn impure_count() {
    array![String::from("Hello, world!"); impure_proc_macro::count!()];
}

#[test]
fn impure_count_backwards() {
    array![String::from("Hello, world!"); impure_proc_macro::count_backwards!()];
}

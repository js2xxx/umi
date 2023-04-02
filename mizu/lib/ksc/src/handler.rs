use alloc::boxed::Box;
use core::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

use ahash::RandomState;
use co_trap::TrapFrame;
use futures_util::FutureExt;
use hashbrown::HashMap;

use crate::RawReg;

pub type Boxed<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub trait Handler: 'static {
    fn handle(&self, tf: &mut TrapFrame);
}

pub trait AsyncHandler: 'static {
    fn handle<'a>(&self, tf: &'a mut TrapFrame) -> Boxed<'a>;
}

pub trait IntoHandler<Param> {
    type Handler: Handler;

    fn into_handler(self) -> Self::Handler;
}

pub trait IntoAsyncHandler<Param> {
    type Handler: AsyncHandler;

    fn into_handler(self) -> Self::Handler;
}

pub struct FunctionHandler<F, Args> {
    func: F,
    _param: PhantomData<Args>,
}

macro_rules! impl_fn {
    ($($arg:ident),*) => {
        impl<Z, $($arg: RawReg + 'static,)* R: RawReg> Handler for FunctionHandler<Z, ($($arg,)*)>
        where
            Z: Fn($($arg),*) -> R + 'static,
        {
            fn handle(&self, tf: &mut TrapFrame) {
                $(
                    #[allow(non_snake_case)]
                    let $arg = tf.syscall_arg::<${index()}>();
                )*
                let r = (self.func)($(<$arg as RawReg>::from_raw($arg)),*);
                tf.set_syscall_ret(R::into_raw(r))
            }
        }

        impl<Z, $($arg: RawReg + 'static,)* R: RawReg> IntoHandler<($($arg,)*)> for Z
        where
            Z: Fn($($arg),*) -> R + 'static,
        {
            type Handler = FunctionHandler<Z, ($($arg,)*)>;

            fn into_handler(self) -> Self::Handler {
                FunctionHandler {
                    func: self,
                    _param: PhantomData,
                }
            }
        }

        impl<Z, $($arg: RawReg + 'static,)* R> AsyncHandler for FunctionHandler<Z, ($($arg,)*)>
        where
            R: Future + Send + 'static,
            R::Output: RawReg,
            Z: Fn($($arg),*) -> R + 'static,
        {
            fn handle<'a>(&self, tf: &'a mut TrapFrame) -> Boxed<'a> {
                $(
                    #[allow(non_snake_case)]
                    let $arg = tf.syscall_arg::<${index()}>();
                )*
                let r = (self.func)($(<$arg as RawReg>::from_raw($arg)),*);
                Box::pin(r.map(move |ret| tf.set_syscall_ret(R::Output::into_raw(ret))))
            }
        }

        impl<Z, $($arg: RawReg + 'static,)* R> IntoAsyncHandler<($($arg,)*)> for Z
        where
            R: Future + Send + 'static,
            R::Output: RawReg,
            Z: Fn($($arg),*) -> R + 'static,
        {
            type Handler = FunctionHandler<Z, ($($arg,)*)>;

            fn into_handler(self) -> Self::Handler {
                FunctionHandler {
                    func: self,
                    _param: PhantomData,
                }
            }
        }
    };
}

impl_fn!(A);
impl_fn!(A, B);
impl_fn!(A, B, C);
impl_fn!(A, B, C, D);
impl_fn!(A, B, C, D, E);
impl_fn!(A, B, C, D, E, F);
impl_fn!(A, B, C, D, E, F, G);

pub struct Handlers {
    map: HashMap<u8, Box<dyn Handler>, RandomState>,
}

impl Handlers {
    pub fn new(seed: usize) -> Self {
        Handlers {
            map: HashMap::with_hasher(RandomState::with_seed(seed)),
        }
    }

    pub fn with<P>(mut self, hn: u8, handler: impl IntoHandler<P>) -> Self {
        self.map.insert(hn, Box::new(handler.into_handler()));
        self
    }

    pub fn handle(&self, tf: &mut TrapFrame) {
        let hn = u8::try_from(tf.syscall_arg::<7>());
        let handler = hn.ok().and_then(|hn| self.map.get(&hn));
        if let Some(handler) = handler {
            handler.handle(tf);
        }
    }
}

pub struct AsyncHandlers {
    map: HashMap<u8, Box<dyn AsyncHandler>, RandomState>,
}

impl AsyncHandlers {
    pub fn new(seed: usize) -> Self {
        AsyncHandlers {
            map: HashMap::with_hasher(RandomState::with_seed(seed)),
        }
    }

    pub fn with<P>(mut self, hn: u8, handler: impl IntoAsyncHandler<P>) -> Self {
        self.map.insert(hn, Box::new(handler.into_handler()));
        self
    }

    pub fn handle<'a>(&self, tf: &'a mut TrapFrame) -> Handle<'a> {
        let hn = u8::try_from(tf.syscall_arg::<7>());
        let handler = hn.ok().and_then(|hn| self.map.get(&hn));
        Handle(handler.map(|handler| handler.handle(tf)))
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct Handle<'a>(Option<Boxed<'a>>);

impl Unpin for Handle<'_> {}

impl Future for Handle<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.0.as_mut() {
            Some(fut) => fut.poll_unpin(cx),
            None => Poll::Ready(()),
        }
    }
}

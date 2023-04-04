use alloc::boxed::Box;
use core::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    pin::Pin,
};

use ahash::RandomState;
use co_trap::TrapFrame;
use futures_util::Future;
use hashbrown::HashMap;

use crate::RawReg;

pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A wrapper around `TrapFrame` to make it easier to access the arguments and
/// return values from user's syscalls.
///
/// Pass a function prototype to the generic parameter to utilize its max
/// functionality:
///
/// ```
/// use ksc::UserCx;
///
/// let mut tf = Default::default();
///
/// let user: UserCx<'_, fn(u32, *const u8) -> usize> =
///     UserCx::from(&mut tf);
///
/// let (a, b): (u32, *const u8) = user.args();
/// user.ret(a as usize + b as usize);
/// ```
pub struct UserCx<'a, A> {
    tf: &'a mut TrapFrame,
    _marker: PhantomData<A>,
}

impl<'a, A> From<&'a mut TrapFrame> for UserCx<'a, A> {
    fn from(tf: &'a mut TrapFrame) -> Self {
        UserCx {
            tf,
            _marker: PhantomData,
        }
    }
}

impl<'a, A> Deref for UserCx<'a, A> {
    type Target = TrapFrame;

    fn deref(&self) -> &Self::Target {
        self.tf
    }
}

impl<'a, A> DerefMut for UserCx<'a, A> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.tf
    }
}

impl<'a, A> UserCx<'a, A> {
    /// Get the underlying `TrapFrame`, consuming `self`.
    pub fn into_inner(self) -> &'a mut TrapFrame {
        self.tf
    }
}

macro_rules! impl_arg {
    ($($arg:ident),*) => {
        impl<'a, $($arg: RawReg,)* T: RawReg> UserCx<'a, fn($($arg,)*) -> T> {
            #[allow(clippy::unused_unit)]
            #[allow(non_snake_case)]
            #[allow(unused_parens)]
            /// Get the arguments with the same prototype as the parameters in the function prototype.
            pub fn args(&self) -> ($($arg),*) {
                $(
                    let $arg = self.tf.syscall_arg::<${index()}>();
                )*
                ($(RawReg::from_raw($arg)),*)
            }

            /// Gives the return value to the user context, consuming `self`.
            pub fn ret(self, value: T) {
                self.tf.set_syscall_ret(RawReg::into_raw(value))
            }
        }
    };
}

impl_arg!();
impl_arg!(A);
impl_arg!(A, B);
impl_arg!(A, B, C);
impl_arg!(A, B, C, D);
impl_arg!(A, B, C, D, E);
impl_arg!(A, B, C, D, E, F);
impl_arg!(A, B, C, D, E, F, G);

pub trait Handler<'a>: Send + Sync {
    type State;
    type Output: Send + 'a;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Self::Output;
}

pub trait HandlerFunc<'a, Marker>: Send + Sync {
    type State;
    type Output: Send + 'a;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Self::Output;
}

pub trait HandlerFut<'a, Marker>: Send + Sync {
    type State;
    type Output: Send + 'a;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Boxed<'a, Self::Output>;
}

impl<'a, F, S, O, A> HandlerFunc<'a, for<'any> fn(&'any mut S, UserCx<'any, A>)> for F
where
    F: Fn(&'a mut S, UserCx<'a, A>) -> O + Send + Sync,
    S: 'a,
    O: Send + 'a,
{
    type State = S;
    type Output = O;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Self::Output {
        (self)(state, UserCx::from(tf))
    }
}

impl<'a, F, S, O, A> HandlerFut<'a, for<'any> fn(&'any mut S, UserCx<'any, A>)> for F
where
    F: Fn(&'a mut S, UserCx<'a, A>) -> O + Send + Sync,
    S: 'a,
    O: Future + Send + 'a,
    O::Output: Send + 'a,
{
    type State = S;
    type Output = O::Output;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Boxed<'a, Self::Output> {
        let arg = UserCx {
            tf,
            _marker: PhantomData,
        };
        Box::pin((self)(state, arg))
    }
}

#[derive(Copy, Clone)]
pub struct FunctionHandler<F, Marker> {
    func: F,
    _marker: PhantomData<fn(Marker)>,
}

impl<'a, Z, Marker> Handler<'a> for FunctionHandler<Z, Marker>
where
    Z: HandlerFunc<'a, Marker>,
{
    type State = Z::State;
    type Output = Z::Output;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Self::Output {
        self.func.handle(state, tf)
    }
}

#[derive(Copy, Clone)]
pub struct FutureHandler<F, Marker> {
    func: F,
    _marker: PhantomData<fn(Marker)>,
}

impl<'a, Z, Marker> Handler<'a> for FutureHandler<Z, Marker>
where
    Z: HandlerFut<'a, Marker>,
{
    type State = Z::State;
    type Output = Boxed<'a, Z::Output>;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Self::Output {
        self.func.handle(state, tf)
    }
}

pub trait IntoHandler<Marker> {
    type Handler: for<'any> Handler<'any, State = Self::State<'any>, Output = Self::Output<'any>>;
    type State<'a>;
    type Output<'a>;

    fn handler(self) -> Self::Handler;
}

impl<H: for<'any> Handler<'any>> IntoHandler<()> for H {
    type Handler = H;
    type State<'a> = <H as Handler<'a>>::State;
    type Output<'a> = <H as Handler<'a>>::Output;

    fn handler(self) -> Self::Handler {
        self
    }
}

pub enum AsFunc {}
impl<F, Marker> IntoHandler<(AsFunc, Marker)> for F
where
    F: for<'any> HandlerFunc<'any, Marker>,
{
    type Handler = FunctionHandler<F, Marker>;
    type State<'a> = <F as HandlerFunc<'a, Marker>>::State;
    type Output<'a> = <F as HandlerFunc<'a, Marker>>::Output;

    fn handler(self) -> Self::Handler {
        FunctionHandler {
            func: self,
            _marker: PhantomData,
        }
    }
}

pub enum AsFut {}
impl<Z, Marker> IntoHandler<(AsFut, Marker)> for Async<Z>
where
    Z: for<'any> HandlerFut<'any, Marker>,
{
    type Handler = FutureHandler<Z, Marker>;
    type State<'a> = <Z as HandlerFut<'a, Marker>>::State;
    type Output<'a> = Boxed<'a, <Z as HandlerFut<'a, Marker>>::Output>;

    fn handler(self) -> Self::Handler {
        FutureHandler {
            func: self.0,
            _marker: PhantomData,
        }
    }
}

pub struct Async<F>(pub F);

type AnyHandler<S, O> = Box<dyn for<'a> Handler<'a, State = S, Output = O>>;
/// A collection of handlers.
pub struct Handlers<S, O> {
    map: HashMap<u8, AnyHandler<S, O>, RandomState>,
}

impl<S, O: Send> Handlers<S, O> {
    pub fn new(seed: usize) -> Self {
        Handlers {
            map: HashMap::with_hasher(RandomState::with_seed(seed)),
        }
    }

    /// Insert a handler to the collection, replacing the old value in the slot
    /// indexed by `scn` if any. Commonly used in chains.
    ///
    /// # Example
    /// ```
    /// use ksc::{Handlers, UserCx};
    ///
    /// fn h0(_: &mut (), _: UserCx<fn()>) {}
    /// fn h1(_: &mut (), _: UserCx<fn(usize) -> usize>) {}
    /// fn h2(_: &mut (), _: UserCx<fn(i32, *const u8) -> u64>) {}
    ///
    /// let handlers = Handlers::new(0).map(0, h0).map(1, h1).map(2, h2);
    /// handlers.handle(&mut (), &mut Default::default());
    /// ```
    pub fn map<H, Marker: 'static>(mut self, scn: u8, handler: H) -> Self
    where
        H: for<'any> IntoHandler<Marker, State<'any> = S, Output<'any> = O> + 'static,
    {
        self.insert(scn, handler);
        self
    }

    /// Insert a handler to the collection, replacing the old value in the slot
    /// indexed by `scn` if any.
    ///
    /// # Example
    /// ```
    /// use ksc::{Handlers, UserCx};
    ///
    /// fn h0(_: &mut (), _: UserCx<fn()>) {}
    /// fn h1(_: &mut (), _: UserCx<fn(usize) -> usize>) {}
    /// fn h2(_: &mut (), _: UserCx<fn(i32, *const u8) -> u64>) {}
    ///
    /// let mut handlers = Handlers::new(0);
    /// handlers.insert(0, h0);
    /// handlers.insert(1, h1);
    /// handlers.insert(2, h2);
    /// handlers.handle(&mut (), &mut Default::default());
    /// ```
    pub fn insert<H, Marker: 'static>(&mut self, scn: u8, handler: H)
    where
        H: for<'any> IntoHandler<Marker, State<'any> = S, Output<'any> = O> + 'static,
    {
        self.map.insert(scn, Box::new(handler.handler()));
    }

    /// Execute the handler in the slot indexed by `scn`, which is acquired from
    /// the given `TrapFrame`.
    pub fn handle(&self, state: &mut S, tf: &mut TrapFrame) -> Option<O> {
        let scn = u8::try_from(tf.syscall_arg::<7>());
        let handler = scn.ok().and_then(|scn| self.map.get(&scn));
        handler.map(|handler| handler.handle(state, tf))
    }
}

type AnyAsyncHandler<S, O> = Box<dyn for<'a> Handler<'a, State = S, Output = Boxed<'a, O>>>;
/// A collection of async handlers.
pub struct AHandlers<S, O> {
    map: HashMap<u8, AnyAsyncHandler<S, O>, RandomState>,
}

impl<S, O> AHandlers<S, O> {
    pub fn new(seed: usize) -> Self {
        AHandlers {
            map: HashMap::with_hasher(RandomState::with_seed(seed)),
        }
    }

    /// Insert an async handler to the collection, replacing the old value in
    /// the slot indexed by `scn` if any. Commonly used in chains.
    ///
    /// # Example
    /// ```
    /// use ksc::{AHandlers, UserCx};
    ///
    /// async fn h0(_: &mut (), _: UserCx<'_, fn()>) {}
    /// async fn h1(_: &mut (), _: UserCx<'_, fn(usize) -> usize>) {}
    /// async fn h2(_: &mut (), _: UserCx<'_, fn(i32, u8) -> u64>) {}
    ///
    /// let handlers = AHandlers::new(0).map(0, h0).map(1, h1).map(2, h2);
    /// smol::block_on(handlers.handle(&mut (), &mut Default::default()));
    /// ```
    pub fn map<H, Marker: 'static>(mut self, scn: u8, handler: H) -> Self
    where
        Async<H>:
            for<'any> IntoHandler<Marker, State<'any> = S, Output<'any> = Boxed<'any, O>> + 'static,
    {
        self.insert(scn, handler);
        self
    }

    /// Insert an async handler to the collection, replacing the old value in
    /// the slot indexed by `scn` if any.
    ///
    /// # Example
    /// ```
    /// use ksc::{AHandlers, UserCx};
    ///
    /// async fn h0(_: &mut (), _: UserCx<'_, fn()>) {}
    /// async fn h1(_: &mut (), _: UserCx<'_, fn(usize) -> usize>) {}
    /// async fn h2(_: &mut (), _: UserCx<'_, fn(i32, u8) -> u64>) {}
    ///
    /// let mut handlers = AHandlers::new(0);
    /// handlers.insert(0, h0);
    /// handlers.insert(1, h1);
    /// handlers.insert(2, h2);
    /// smol::block_on(handlers.handle(&mut (), &mut Default::default()));
    /// ```
    pub fn insert<H, Marker: 'static>(&mut self, scn: u8, handler: H)
    where
        Async<H>:
            for<'any> IntoHandler<Marker, State<'any> = S, Output<'any> = Boxed<'any, O>> + 'static,
    {
        self.map.insert(scn, Box::new(Async(handler).handler()));
    }

    /// Execute the async handler in the slot indexed by `scn`, which is
    /// acquired from the given `TrapFrame`.
    pub async fn handle(&self, state: &mut S, tf: &mut TrapFrame) -> Option<O> {
        let scn = u8::try_from(tf.syscall_arg::<7>());
        let handler = scn.ok().and_then(|scn| self.map.get(&scn));
        match handler {
            Some(handler) => Some(handler.handle(state, tf).await),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handlers() {
        fn handler0(s: &mut u8, user: UserCx<fn(u32, *const u8) -> u64>) -> usize {
            let (_a, _b) = user.args();
            *s -= 1;
            user.ret(*s as u64 + 10);
            *s as usize
        }

        let h = Handlers::new(0).map(0, handler0);

        {
            let mut state = 234;
            let ret = h.handle(&mut state, &mut TrapFrame::default());
            assert_eq!(ret, Some(233));

            let ret = h.handle(&mut state, &mut TrapFrame::default());
            assert_eq!(ret, Some(232));
        }

        {
            let mut state = 1;
            let ret = h.handle(&mut state, &mut TrapFrame::default());
            assert_eq!(ret, Some(0));
        }
    }

    #[test]
    fn test_fut() {
        async fn handler0(s: &mut u8, user: UserCx<'_, fn(u32, isize) -> u64>) -> usize {
            let (_a, _b) = user.args();
            *s -= 1;
            user.ret(*s as u64 + 10);
            *s as usize
        }

        let h = FutureHandler {
            func: handler0,
            _marker: PhantomData,
        };
        Handler::handle(&h, &mut 234, &mut TrapFrame::default());

        let a = async move {
            let h = AHandlers::new(0).map(0, handler0);
            {
                let mut state = 234;
                let ret = h.handle(&mut state, &mut TrapFrame::default()).await;
                assert_eq!(ret, Some(233));

                let ret = h.handle(&mut state, &mut TrapFrame::default()).await;
                assert_eq!(ret, Some(232));
            }

            {
                let mut state = 1;
                let ret = h.handle(&mut state, &mut TrapFrame::default()).await;
                assert_eq!(ret, Some(0));
            }
        };
        smol::block_on(a);
    }
}

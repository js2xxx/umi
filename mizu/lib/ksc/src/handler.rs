use alloc::boxed::Box;
use core::{marker::PhantomData, pin::Pin};

use ahash::RandomState;
use co_trap::TrapFrame;
use futures_util::Future;
use hashbrown::HashMap;

use crate::RawReg;

pub type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct UserArg<'a, A, T> {
    tf: &'a mut TrapFrame,
    _marker: PhantomData<(A, T)>,
}

impl<'a, A, T: RawReg> UserArg<'a, A, T> {
    pub fn trap_frame(&mut self) -> &mut TrapFrame {
        self.tf
    }

    pub fn ret(self, value: T) {
        self.tf.set_syscall_ret(RawReg::into_raw(value))
    }
}

macro_rules! impl_arg {
    ($($arg:ident),*) => {
        impl<'a, $($arg: RawReg,)* T> UserArg<'a, ($($arg,)*), T> {
            #[allow(clippy::unused_unit)]
            #[allow(non_snake_case)]
            #[allow(unused_parens)]
            pub fn get(&self) -> ($($arg),*) {
                $(
                    let $arg = self.tf.syscall_arg::<${index()}>();
                )*
                ($(RawReg::from_raw($arg)),*)
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

impl<'a, F, S, O, A, T> HandlerFunc<'a, for<'any> fn(&'any mut S, UserArg<'any, A, T>)> for F
where
    F: Fn(&'a mut S, UserArg<'a, A, T>) -> O + Send + Sync,
    S: 'a,
    O: Send + 'a,
{
    type State = S;
    type Output = O;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Self::Output {
        let arg = UserArg {
            tf,
            _marker: PhantomData,
        };
        (self)(state, arg)
    }
}

impl<'a, F, S, O, A, T> HandlerFut<'a, for<'any> fn(&'any mut S, UserArg<'any, A, T>)> for F
where
    F: Fn(&'a mut S, UserArg<'a, A, T>) -> O + Send + Sync,
    S: 'a,
    O: Future + Send + 'a,
    O::Output: Send + 'a,
{
    type State = S;
    type Output = O::Output;

    fn handle(&self, state: &'a mut Self::State, tf: &'a mut TrapFrame) -> Boxed<'a, Self::Output> {
        let arg = UserArg {
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

pub trait IntoHandler<'a, S, O, Marker> {
    type Handler: Handler<'a, State = S, Output = O>;

    fn handler(self) -> Self::Handler;
}

impl<'a, S, O, H: Handler<'a, State = S, Output = O>> IntoHandler<'a, S, O, ()> for H {
    type Handler = H;

    fn handler(self) -> Self::Handler {
        self
    }
}

pub enum AsFunc {}
impl<'a, F, Marker> IntoHandler<'a, F::State, F::Output, (AsFunc, Marker)> for F
where
    F: HandlerFunc<'a, Marker>,
{
    type Handler = FunctionHandler<F, Marker>;

    fn handler(self) -> Self::Handler {
        FunctionHandler {
            func: self,
            _marker: PhantomData,
        }
    }
}

pub enum AsFut {}
impl<'a, Z, Marker> IntoHandler<'a, Z::State, Boxed<'a, Z::Output>, (AsFut, Marker)> for Async<Z>
where
    Z: HandlerFut<'a, Marker>,
{
    type Handler = FutureHandler<Z, Marker>;

    fn handler(self) -> Self::Handler {
        FutureHandler {
            func: self.0,
            _marker: PhantomData,
        }
    }
}

pub struct Async<F>(pub F);

type AnyHandler<S, O> = Box<dyn for<'a> Handler<'a, State = S, Output = O>>;
pub struct Handlers<S, O> {
    map: HashMap<u8, AnyHandler<S, O>, RandomState>,
}

impl<S, O: Send> Handlers<S, O> {
    pub fn new(seed: usize) -> Self {
        Handlers {
            map: HashMap::with_hasher(RandomState::with_seed(seed)),
        }
    }

    pub fn map<H, Marker: 'static>(mut self, scn: u8, handler: H) -> Self
    where
        H: for<'any> HandlerFunc<'any, Marker, State = S, Output = O> + 'static,
    {
        self.map.insert(scn, Box::new(handler.handler()));
        self
    }

    pub fn map_handler<H>(mut self, scn: u8, handler: H) -> Self
    where
        H: for<'any> Handler<'any, State = S, Output = O> + 'static,
    {
        self.map.insert(scn, Box::new(handler));
        self
    }

    pub fn handle(&self, state: &mut S, tf: &mut TrapFrame) -> Option<O> {
        let scn = u8::try_from(tf.syscall_arg::<7>());
        let handler = scn.ok().and_then(|scn| self.map.get(&scn));
        handler.map(|handler| handler.handle(state, tf))
    }
}

type AnyAsyncHandler<S, O> = Box<dyn for<'a> Handler<'a, State = S, Output = Boxed<'a, O>>>;
pub struct AHandlers<S, O> {
    map: HashMap<u8, AnyAsyncHandler<S, O>, RandomState>,
}

impl<S, O> AHandlers<S, O> {
    pub fn new(seed: usize) -> Self {
        AHandlers {
            map: HashMap::with_hasher(RandomState::with_seed(seed)),
        }
    }

    pub fn map<H, Marker: 'static>(mut self, scn: u8, handler: H) -> Self
    where
        H: for<'any> HandlerFut<'any, Marker, State = S, Output = O> + 'static,
    {
        self.map.insert(scn, Box::new(Async(handler).handler()));
        self
    }

    pub fn map_handler<H>(mut self, scn: u8, handler: H) -> Self
    where
        H: for<'any> Handler<'any, State = S, Output = Boxed<'any, O>> + 'static,
    {
        self.map.insert(scn, Box::new(handler));
        self
    }

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
        fn handler0(s: &mut u8, user: UserArg<'_, (u32, *const u8), u64>) -> usize {
            let (_a, _b) = user.get();
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
        async fn handler0<'a>(s: &'a mut u8, user: UserArg<'a, (u32, isize), u64>) -> usize {
            let (_a, _b) = user.get();
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

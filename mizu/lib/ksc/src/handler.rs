use alloc::boxed::Box;
use core::{borrow::Borrow, hash::Hash, marker::PhantomData};

use bevy_utils_proc_macros::all_tuples;
use hashbrown::HashMap;
pub use ksc_core::handler::*;
use rand_riscv::RandomState;

pub trait Handler<'a>: Send + Sync {
    type Param: Param;
    type Output: Param;

    fn handle(&self, param: <Self::Param as Param>::Item<'a>) -> <Self::Output as Param>::Item<'a>;
}

pub trait HandlerFunc<'a, Marker>: Send + Sync {
    type Param: Param;
    type Output: Param;

    fn handle(&self, param: <Self::Param as Param>::Item<'a>) -> <Self::Output as Param>::Item<'a>;
}

macro_rules! impl_func {
    ($($param:ident),*) => {
        #[allow(clippy::unused_unit)]
        #[allow(non_snake_case)]
        #[allow(unused_parens)]
        impl<'a, G, T, $($param: Param),*> HandlerFunc<'a, fn($($param),*) -> T> for G
        where
            G:
                (Fn($(<$param as Param>::Item<'a>),*) -> <T as Param>::Item<'a>)
                + (Fn($($param),*) -> T)
                + Send + Sync,
            T: Param,
        {
            type Param = ($($param),*);
            type Output = T;

            fn handle(&self, param: <Self::Param as Param>::Item<'a>) -> <Self::Output as Param>::Item<'a> {
                let ($($param),*) = param;
                (self)($($param),*)
            }
        }
    };
}
all_tuples!(impl_func, 1, 12, P);

pub struct FunctionHandler<F, Marker> {
    func: F,
    marker: PhantomData<fn(Marker)>,
}

impl<F: Copy, Marker> Copy for FunctionHandler<F, Marker> {}

impl<F: Clone, Marker> Clone for FunctionHandler<F, Marker> {
    fn clone(&self) -> Self {
        Self {
            func: self.func.clone(),
            marker: self.marker,
        }
    }
}

impl<'a, G, Marker, P, O> Handler<'a> for FunctionHandler<G, (P, Marker)>
where
    G: HandlerFunc<'a, Marker, Output = O>,
    P: Param,
    O: Param,
    G::Param: FromParam<P>,
{
    type Param = P;
    type Output = O;

    fn handle(&self, param: <Self::Param as Param>::Item<'a>) -> O::Item<'a> {
        self.func
            .handle(<G::Param as FromParam<P>>::from_param(param))
    }
}

pub trait IntoHandler<Marker> {
    type Handler: for<'any> Handler<'any, Param = Self::Param<'any>, Output = Self::Output<'any>>;
    type Param<'a>;
    type Output<'a>;

    fn handler(self) -> Self::Handler;
}

impl<H: for<'any> Handler<'any>> IntoHandler<()> for H {
    type Handler = Self;
    type Param<'a> = <Self as Handler<'a>>::Param;
    type Output<'a> = <Self as Handler<'a>>::Output;

    fn handler(self) -> Self::Handler {
        self
    }
}

pub enum AsFunc {}
impl<G, Marker, P: Param> IntoHandler<(AsFunc, P, Marker)> for G
where
    G: for<'any> HandlerFunc<'any, Marker>,
    for<'any> <G as HandlerFunc<'any, Marker>>::Param: FromParam<P>,
{
    type Handler = FunctionHandler<G, (P, Marker)>;
    type Param<'a> = P;
    type Output<'a> = <G as HandlerFunc<'a, Marker>>::Output;

    fn handler(self) -> Self::Handler {
        FunctionHandler {
            func: self,
            marker: PhantomData,
        }
    }
}

type AnyHandler<P, O> = Box<dyn for<'a> Handler<'a, Param = P, Output = O>>;
/// A collection of handlers.
pub struct Handlers<K, P, O> {
    map: HashMap<K, AnyHandler<P, O>, RandomState>,
}

impl<K, P, O> Handlers<K, P, O> {
    pub fn new() -> Self {
        Handlers {
            map: HashMap::with_hasher(RandomState::new()),
        }
    }
}

impl<K: Eq + Hash, P: Param, O: Param> Handlers<K, P, O> {
    /// Insert a handler to the collection, replacing the old value in the slot
    /// indexed by `scn` if any. Commonly used in chains.
    ///
    /// # Example
    /// ```
    /// use ksc::{Handlers, __TEST0, __TEST1, __TEST2};
    /// use co_trap::UserCx;
    ///
    /// fn h0(_: &mut (), _: UserCx<fn()>) {}
    /// fn h1(_: &mut (), _: UserCx<fn(usize) -> usize>) {}
    /// fn h2(_: &mut (), _: UserCx<fn(i32, *const u16) -> u64>) {}
    ///
    /// let handlers = Handlers::new()
    ///     .map(__TEST0, h0)
    ///     .map(__TEST1, h1)
    ///     .map(__TEST2, h2);
    /// handlers.handle(__TEST0, (&mut (), &mut Default::default()));
    /// ```
    pub fn map<H, Marker: 'static>(mut self, key: K, handler: H) -> Self
    where
        H: for<'any> IntoHandler<Marker, Param<'any> = P, Output<'any> = O> + 'static,
    {
        self.insert(key, handler);
        self
    }

    /// Insert a handler to the collection, replacing the old value in the slot
    /// indexed by `scn` if any.
    ///
    /// # Example
    /// ```
    /// use ksc::{Handlers, __TEST0, __TEST1, __TEST2};
    /// use co_trap::UserCx;
    ///
    /// fn h0(_: &mut (), _: UserCx<fn()>) {}
    /// fn h1(_: &mut (), _: UserCx<fn(usize) -> usize>) {}
    /// fn h2(_: &mut (), _: UserCx<fn(i32, *const u16) -> u64>) {}
    ///
    /// let mut handlers = Handlers::new();
    /// handlers.insert(__TEST0, h0);
    /// handlers.insert(__TEST1, h1);
    /// handlers.insert(__TEST2, h2);
    /// handlers.handle(__TEST0, (&mut (), &mut Default::default()));
    /// ```
    pub fn insert<H, Marker: 'static>(&mut self, key: K, handler: H)
    where
        H: for<'any> IntoHandler<Marker, Param<'any> = P, Output<'any> = O> + 'static,
    {
        self.map.insert(key, Box::new(handler.handler()));
    }

    /// Execute the handler in the slot indexed by `scn`, which is acquired from
    /// the given `TrapFrame`.
    pub fn handle<'a>(
        &self,
        key: impl Borrow<K>,
        param: <P as Param>::Item<'a>,
    ) -> Option<O::Item<'a>> {
        let handler = self.map.get(key.borrow());
        handler.map(|handler| handler.handle(param))
    }
}

impl<K, P, O> Default for Handlers<K, P, O> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AHandlers<K, P, O>(Handlers<K, P, Boxed<'static, O>>);

impl<K, P, O> AHandlers<K, P, O> {
    pub fn new() -> Self {
        AHandlers(Handlers::new())
    }
}

impl<K: Eq + Hash, P: Param, O: Param> AHandlers<K, P, O> {
    /// Insert an async handler to the collection, replacing the old value in
    /// the slot indexed by `scn` if any. Commonly used in chains.
    ///
    /// # Example
    /// ```
    /// use ksc::{AHandlers, async_handler, __TEST0, __TEST1, __TEST2};
    /// use co_trap::UserCx;
    ///
    /// #[async_handler]
    /// async fn h0(_: &mut (), _: UserCx<'_, fn()>) {}
    /// #[async_handler]
    /// async fn h1(_: &mut (), _: UserCx<'_, fn(usize) -> usize>) {}
    /// #[async_handler]
    /// async fn h2(_: &mut (), _: UserCx<'_, fn(i32, u16) -> u64>) {}
    ///
    /// let handlers = AHandlers::new()
    ///     .map(__TEST0, h0)
    ///     .map(__TEST1, h1)
    ///     .map(__TEST2, h2);
    /// spin_on::spin_on(handlers.handle(__TEST0, (&mut (), &mut Default::default())));
    /// ```
    pub fn map<'a, H, Marker: 'static>(mut self, key: K, handler: H) -> Self
    where
        H: for<'any> IntoHandler<Marker, Param<'any> = P, Output<'any> = Boxed<'static, O>>
            + 'static,
    {
        self.insert(key, handler);
        self
    }

    /// Insert an async handler to the collection, replacing the old value in
    /// the slot indexed by `scn` if any.
    ///
    /// # Example
    /// ```
    /// use ksc::{AHandlers, async_handler, __TEST0, __TEST1, __TEST2};
    /// use co_trap::UserCx;
    ///
    /// #[async_handler]
    /// async fn h0(_: &mut (), _: UserCx<'_, fn()>) {}
    /// #[async_handler]
    /// async fn h1(_: &mut (), _: UserCx<'_, fn(usize) -> usize>) {}
    /// #[async_handler]
    /// async fn h2(_: &mut (), _: UserCx<'_, fn(i32, u16) -> u64>) {}
    ///
    /// let mut handlers = AHandlers::new();
    /// handlers.insert(__TEST0, h0);
    /// handlers.insert(__TEST1, h1);
    /// handlers.insert(__TEST2, h2);
    /// spin_on::spin_on(handlers.handle(__TEST0, (&mut (), &mut Default::default())));
    /// ```
    pub fn insert<H, Marker: 'static>(&mut self, key: K, handler: H)
    where
        H: for<'any> IntoHandler<Marker, Param<'any> = P, Output<'any> = Boxed<'static, O>>
            + 'static,
    {
        self.0.insert(key, handler)
    }

    /// Execute the async handler in the slot indexed by `scn`, which is
    /// acquired from the given `TrapFrame`.
    pub async fn handle<'a>(
        &self,
        key: impl Borrow<K>,
        param: <P as Param>::Item<'a>,
    ) -> Option<O::Item<'a>> {
        match self.0.handle(key, param) {
            Some(fut) => Some(fut.await),
            None => None,
        }
    }
}

impl<K, P, O> Default for AHandlers<K, P, O> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use co_trap::{TrapFrame, UserCx};

    use super::*;
    use crate::async_handler;

    #[test]
    fn test_handlers() {
        fn handler0<'a>(
            s: &'a mut u16,
            user: UserCx<'a, fn(u32, *const u16) -> u64>,
        ) -> &'a mut u16 {
            let (_a, _b) = user.args();
            *s -= 1;
            user.ret(*s as u64 + 10);
            s
        }

        static H: LazyLock<Handlers<u8, (&mut u16, &mut TrapFrame), &mut u16>> =
            LazyLock::new(|| Handlers::new().map(0, handler0));

        let mut tf = TrapFrame::default();
        {
            let mut state = 234;
            let ret = H.handle(&0, (&mut state, &mut tf));
            assert_eq!(ret, Some(&mut 233));

            let ret = H.handle(&0, (&mut state, &mut tf));
            assert_eq!(ret, Some(&mut 232));
        }

        {
            let mut state = 1;
            let ret = H.handle(&0, (&mut state, &mut tf));
            assert_eq!(ret, Some(&mut 0));
        }
    }

    #[test]
    fn test_fut() {
        fn handler0<'a>(s: &'a mut u16) -> Boxed<'a, usize> {
            Box::pin(async move {
                // let (_a, _b) = user.args();
                *s -= 1;
                // user.ret(*s as u64 + 10);
                *s as usize
            })
        }

        #[async_handler]
        async fn handler1(s: &mut u16, user: UserCx<'_, fn(u32, *const u16) -> u64>) -> usize {
            let (_a, _b) = user.args();
            *s -= 1;
            user.ret(*s as u64 + 10);
            *s as usize
        }

        let h = FunctionHandler {
            func: handler0,
            marker: PhantomData,
        };
        Handler::handle(&h, &mut 234);

        fn assert_handler_fut<'a, Marker, F: HandlerFunc<'a, Marker>>(_: F) {}
        fn assert_any_handler_fut<Marker, F: for<'any> HandlerFunc<'any, Marker>>(_: F) {}

        assert_handler_fut(handler0);
        assert_any_handler_fut(handler0);
        assert_handler_fut(handler1);
        assert_any_handler_fut(handler1);

        static H: LazyLock<AHandlers<u8, (&mut u16, &mut TrapFrame), usize>> =
            LazyLock::new(|| AHandlers::new().map(0, handler1));

        spin_on::spin_on(async move {
            {
                let mut state = 234;
                let ret = H.handle(&0, (&mut state, &mut TrapFrame::default())).await;
                assert_eq!(ret, Some(233));

                let ret = H.handle(&0, (&mut state, &mut TrapFrame::default())).await;
                assert_eq!(ret, Some(232));
            }

            {
                let mut state = 1;
                let ret = H.handle(&0, (&mut state, &mut TrapFrame::default())).await;
                assert_eq!(ret, Some(0));
            }
        });
    }
}

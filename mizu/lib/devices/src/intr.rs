pub type Completion = dyn Fn() + Send + Sync + 'static;

pub trait IntrHandler: for<'any> Fn(&'any Completion) -> bool + Send + Sync + 'static {}
impl<T: for<'any> Fn(&'any Completion) -> bool + Send + Sync + 'static> IntrHandler for T {}

# `ktime(-core)`

这个模块主要处理时间相关的数据结构。

- 在`core`中模仿标准库定义了独立的`Instant`，并像标准库一样可以与`core::time::Duration`进行互操作；
- 模仿[`async-io`](https://doc.rs/async-io/latest/async_io/struct.Timer.html)实现了异步定时器及其队列。
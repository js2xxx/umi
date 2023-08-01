# `ksync(-core)`

这个模块提供了各种在异步上下文中同步源语，这些数据结构均取自[`async-lock`](https://github.com/smol-rs/async-lock) crate：

  - `mutex` 互斥锁
  - `rw_lock` 读写锁
  - `semaphore` 信号量

此外，还有如下数据结构：

  - `broadcast` 广播事件订阅
  - `mpmc` 多消费者多生产者的通道

在`core`中，实现了单个CPU核内的临界区访问机制：`fn critial`，以用来配合`spin` crate的自旋锁，防止可能的中断重入导致递归锁。

独立出`core`的原因是，由于模块间有相互依赖的关系，通过独立出一些共用的接口或服务就可以来消除模块依赖图中的环。
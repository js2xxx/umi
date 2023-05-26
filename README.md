# UMI: 初赛文档

## 开发人员

队伍：PLNTRY，西安交通大学
队员：徐启航，杨豪

## 项目结构

- `.github/workflows` - Github Actions配置；
- `.vscode` - VS Code工作区配置；
- `cargo-config` - 根目录cargo配置，会在make时复制到.cargo目录中；
- `debug` - 调试信息，包括内核文件的反汇编、ELF元数据还有QEMU的输出日志；
- `mizu/kernel` - 内核主程序代码；
- `mizu/lib` - 内核各个模块的代码；
- `scripts` - 包含第三方依赖的换源脚本；
- `target` - cargo的生成目录；
- `third-party/bin` - RustSBI的BIOS二进制文件；
- `third-party/vendor` - 第三方库的依赖；
- `third-party/img`- 初赛评测程序的磁盘映像文件。

由于分支原因，第三方库的依赖和换源脚本并不实际包含于以上目录中，将在本文档最后进行演示。

## 编译&运行&调试

依赖项：比赛要求的Rust工具链, make, cargo-binutils（通过`cargo install`安装）, riscv64-unknown-elf的GNU工具链。

为了减小仓库总体积，评测程序的磁盘映像文件并没有作为仓库的一部分，需要单独将其作为`sdcard-comp1.img`复制到`third-party/img`目录下，或者手动修改根目录的Makefile。

单独编译：
```bash
make all # MODE=release(默认)|debug
```

直接运行：
```bash
make run # MODE 同上
```

调试：
```bash
make debug MODE=debug # 一个终端
riscv64-unknown-elf-gdb debug/mizu.sym # 另一个终端
```

OS的输出文件被配置在了`debug/qemu.log`文件中，而终端中的QEMU作为一个监视器可以查看实时的运行信息（键入help可以查看所有命令）。

## 开发过程&结构设计

我们的OS是从接近RISC-V的硬件底层根SBI标准开始，先完成大部分模块会用到的公用底层模块的实现，然后将每一个模块逐个设计与实现，最后整合进内核，完成系统调用的编写，辅以简单的调试并通过初赛。接下来将按照开发时间顺序逐个讲解各个模块。

## 模块讲解

我们OS的内核态运行在异步的无栈协程上下文中。TODO

### `ksync(-core)`

这个模块提供了各种在异步上下文中同步源语，这些数据结构均取自(`async-lock`)[https://github.com/smol-rs/async-lock] crate：

  - `mutex` 互斥锁
  - `rw_lock` 读写锁
  - `semaphore` 信号量

此外，还有如下数据结构：

  - `broadcast` 广播事件订阅
  - `mpmc` 多消费者多生产者的通道
  - `RCU` 无锁 Read Copy-Update 机制
    - `epoch` 多线程垃圾回收算法

在`core`中，实现了单个CPU核内的临界区访问机制：`fn critial`，以用来配合`spin` crate的自旋锁，防止可能的中断重入导致递归锁。

独立出`core`的原因是，由于模块间有相互依赖的关系，通过独立出一些共用的接口或服务就可以来消除模块依赖图中的环。

### `ktime(-core)`

这个模块主要处理时间相关的数据结构。

- 在`core`中模仿标准库定义了独立的`Instant`，并像标准库一样可以与`core::time::Duration`进行互操作；
- 模仿(`async-io`)[https://doc.rs/async-io/latest/async_io/struct.Timer.html]实现了异步定时器及其队列。

### `co-trap`

这个模块处理用户态与内核态通过中断机制相互切换的流程控制和对用户上下文的系统调用参数抽象。

传统的中断处理程序通常会以函数调用的形式，伪代码如下：

1. 保存用户上下文；
2. 通过标准的函数调用形式跳转到中断处理程序；
3. 恢复用户上下文并退出。

而这里使用一种新的方式，思想与去年的一等奖作品FTL OS不谋而合，其伪代码分成两个部分，进入用户和进入内核。

进入内核时：

1. 保存用户上下文；
2. 通过sscratch切换并加载内核上下文；
3. 退出函数调用（通过`ret`指令）。

进入用户时：

1. 保存内核上下文；
2. 通过a0切换并加载用户上下文；
3. 退出中断函数（通过`sret`指令）。

可以看出，这两段代码是完全对偶的，并不跟传统方式一样是一个单独的函数调用过程。

实际上这就是一种有栈协程的上下文切换方式。在传统不依靠无栈协程的内核上下文中，内核线程的相互切换便是采用的这种方式。这里将这种方式转移到这里，可以使得用户代码和内核代码变成两个独立的控制流。而从内核态的视角来看，进入用户态相当于调用函数，而退出用户态便是调用函数返回。

```assembly
// a0 <- trap_frame: *mut TrapFrame
// a1 <- scratch register

.global _return_to_user
.type _return_to_user, @function
_return_to_user:
    xchg_sx // 交换s系列寄存器
    load_ux // 加载中断寄存器（sepc）等
    load_tx // 加载a、t系列和sp、tp等寄存器
    load_scratch // 加载a1

    csrw sscratch, a0 // a0即为该函数的参数，保存用户上下文的地址；将其存入sscratch中
    ld a0, 16*8(a0) // 加载a0
    sret

.global _user_entry
.type _user_entry, @function
.align 4
_user_entry:
    csrrw a0, sscratch, a0 // 取出用户上下文地址
    save_scratch
    save_tx // 保存a、t系列和sp、tp等寄存器
    save_ux // 保存中断寄存器（sepc）等
    csrr a1, sscratch // 保存a1
    xchg_sx // 交换s系列寄存器
    ret
```

而对于用户上下文的系统调用参数抽象，我们定义了一个泛型包装结构，可以很容易地从泛型中的函数签名看出系统调用的函数原型签名。在实现的时候使用了宏展开来保证每个函数签名的有效性。

```rust
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

macro_rules! impl_arg {
    ($($arg:ident),*) => {
        impl<'a, $($arg: RawReg,)* T: RawReg> UserCx<'a, fn($($arg),*) -> T> {
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

all_tuples!(impl_arg, 0, 7, P);
```

使用示例：

```rust
let mut tf = Default::default();

let user: UserCx<'_, fn(u32, *const u8) -> usize> =
    UserCx::from(&mut tf);

let (a, b): (u32, *const u8) = user.args();
user.ret(a as usize + b as usize);
```

### `art`

该模块的名称是Async RunTime的缩写，即该模块实现了一个多核的异步执行器，参考了[Tokio](https://tokio.rs/)的实现。

实现的功能有：

- 任务窃取，即在一个CPU核没有任务时，会去其他CPU核的工作队列偷一部分任务回来运行。
- 软抢占式调度：对于 IO 唤醒等特殊情况下被唤醒的线程，将其放在单独的`preempt_slot`中，在CPU调度时优先选择。

该异步执行器是与[`async-task`](https://docs.rs/async-task/latest/async_task/)合作实现的。由于软抢占式调度需要获取任务本身的内部信息，而之前版本的`async-task`并未提供访问这一信息的接口。因此徐启航同学向其提交了一个新的PR，目前已经被合并发布在4.4.0版本中。

### `ksc(-core, -macros)`

这个模块专注于系统调用（Kernel System Calls）以及FFI相关的数据结构：

- `Error` - Linux系统调用的错误码；
- `Scn` - Linux的系统调用号；
- `RawReg` - 系统调用参数和返回值与裸usize之间的转换trait。

还有一个较为重量级的数据结构：系统调用表（`(A)Handlers`）。

由于该OS的内核态运行在异步上下文中，因此系统调用必然也必须写成异步函数的形式，以支持可能的线程间上下文切换。然而传统的系统调用表是以系统调用函数的函数指针作为表项，而异步函数由于返回的Future的唯一性，其签名却是不统一的。因此我们通过一些高阶操作，实现了支持非异步函数和异步函数的函数调用表，分别是`Handlers`和`AHandlers`，参考的是[Bevy游戏引擎的ECS子系统](https://docs.rs/bevy_ecs/latest/bevy_ecs/)中的System实现。效果如下：

```rust
pub type ScParams<'a> = (&'a mut TaskState, &'a mut TrapFrame);
pub type ScRet = ControlFlow<(i32, Option<Sig>), Option<SigInfo>>;

pub static SYSCALL: Lazy<AHandlers<Scn, ScParams, ScRet>> = Lazy::new(|| {
    AHandlers::new()
        // Memory management
        .map(BRK, mem::brk)
        .map(MMAP, fd::mmap)
        .map(MUNMAP, fd::munmap)
        ..
        // Time
        .map(GETTIMEOFDAY, gettimeofday)
});

#[async_handler]
async fn gettimeofday(
    ts: &mut TaskState,
    cx: UserCx<'_, fn(UserPtr<Tv, Out>, i32) -> Result<(), Error>>,
) -> ScRet {
    let (out, _) = cx.args();
    let ret = todo!("gettimeofday");
    cx.ret(ret);
    ScRet::Continue(None)
}
```

其中每个系统调用的`UserCx`的签名都可以是不一样的，因为该结构本身实现了特殊的trait：`Param`和`FromParam`。

```rust
impl<A: 'static> Param for UserCx<'_, A> {
    type Item<'a> = UserCx<'a, A>;
}

impl<A: 'static> FromParam<&'_ mut TrapFrame> for UserCx<'_, A> {
    fn from_param<'a>(item: <&'_ mut TrapFrame as Param>::Item<'a>) -> Self::Item<'a> {
        UserCx::from(item)
    }
}
```

另外，其中定义在`ksc-macros`中的`#[async_handler]`宏可以自动包装异步函数返回的Future，并抹去其具体类型，变成一个`Pin<Box<dyn Future>>`。参考的是[`async-trait`](https://github.com/dtolnay/async-trait)的实现。

### `umio`、`umifs`和`afat32`

- `umio`抽象出一个读写相关的抽象trait `Io`，表示一切可读写的数据结构，以及从标准库扒来的`SeekFrom`、`IoSlice`等基础类型；
- `umifs`实现了umi的虚拟文件系统，实现了高性能的路径解析，并兼容了Linux的文件类型
  - VFS虚拟文件系统在各类文件系统之上构建了一个抽象层，从而使操作系统可以挂载各类w文件系统；
- `afat32` Async FAT32，参考[`rust-fatfs`](https://github.com/rafalh/rust-fatfs)实现的异步且并发的FAT32文件系统。

### `kmem`、`range-map`和`kalloc`

这几个模块专注于内存管理。

#### 物理内存

此处我们解耦了**内核和Rust语言自用的内核堆分配器**和**全局的物理页帧管理**的两个部分，从而减小了复杂度并一定程度上避免了一些安全的问题。

- `kalloc` (内核堆)：初始化一个全局分配器。Rust语言的后端通过 `#[global_allocator]` 确定一个全局的分配器，并在每次需要时从该函数分配内存
  
  ```Rust
  #[global_allocator]
  static GLOBAL_ALLOC: imp::Allocator = imp::Allocator::new();

  #[cfg(not(feature = "test"))]
  pub unsafe fn init<T: Copy>(start: &mut T, end: &mut T) {
    let start_ptr = (start as *mut T).cast();
    let end_ptr = (end as *mut T).cast::<u8>();
    let len = end_ptr.offset_from(start_ptr);
    GLOBAL_ALLOC.init(start_ptr as usize, len as usize)
  }
  ```

- 页帧分配：使用经典的链表模式分配页帧，但是全部操作都是无锁的，比`kalloc`引用的分配器效率更高；
- 页帧管理：提供了RAII的内存管理模型，每个 `Phys` 管理一个包含若干个页帧的LRU缓存（从[`lru-rs`](https://github.com/jeromefroe/lru-rs)导入），和一个可选的IO后端，通过按页帧索引提交、回写等操作来同步后端的内容，并在此基础上也实现了IO trait。

  ```Rust
  pub struct FrameInfo {
    frame: Arc<Frame>,
    len: usize,
    dirty: bool,
    pin_count: usize,
  }

  pub struct Phys {
    frames: Mutex<LruCache<usize, FrameInfo>>,
    position: AtomicUsize,
    cow: bool,
    backend: Arc<dyn Io>,
  }
  ```

#### 虚拟内存&地址空间

我们采用了当前主流操作系统类似的策略：将内核映射到地址空间的高位，从而实现了内核和用户地址的隔离。

虚拟地址空间管理的结构体持有一个根页表和一个`RangeMap`，即以一个范围为键的键值映射表，对每一个分配的地值范围建立一个映射结构体：

```rust
struct Mapping {
    phys: Arc<Phys>,
    start_index: usize,
    attr: Attr,
}

pub struct Virt {
    root: Mutex<Table>,
    map: Mutex<RangeMap<LAddr, Mapping>>,
    cpu_mask: AtomicUsize,

    _marker: PhantomPinned,
}
```

该结构体包含创建映射、提交映射、重设映射权限、释放映射等操作，实现了映射懒分配的功能。其中`RangeMap`结构支持地址空间随机化（ASLR）的分配策略，基于标准库的`BTreeMap`建立了一个键值表，并附带了ASLR的基本实现，从而提升了内核安全性，防止被缓存溢出侵略。

而地址空间的加载卸载和页表缓存的刷新则需要与硬件平台和SBI直接交互。其中加载方式便是为每个CPU分配一个TLS变量储存当前的地址空间结构，在更新`satp`的时候顺带更新该变量以转移所有权。

刷新页表缓存的代码如下：

```rust
/// # Arguments
/// 
/// - `cpu_mask` - 当前正在加载该地址空间的CPU核心；
/// - `addr`和`count` - 刷新页表的基地址和页的个数。
pub fn flush(cpu_mask: usize, addr: LAddr, count: usize) {
    if count == 0 {
        return;
    }
    let others = cpu_mask & !(1 << hart_id::hart_id());
    if others != 0 {
        let _ = sbi_rt::remote_sfence_vma(others, 0, addr.val(), count << PAGE_SHIFT);
    }
    if cpu_mask != others {
        unsafe {
            if count == 1 {
                sfence_vma(0, addr.val())
            } else {
                sfence_vma_all()
            }
        }
    }
}
```

其中有一个坑，使用RustSBI作为BIOS的小伙伴需要注意了：目前RustSBI的官方QEMU实现代码没有实现`remote_sfence_vma`，调用这个接口返回的是不支持错误。虽然在该OS目前的测试中没有遇到因此造成的bug，但如果遇到了页表不一致的问题应该往此方向考虑。

### `devices`

该模块专注于各种通过异步实现的驱动程序，包括PLIC中断控制器、块设备等等。

其中的VirtIO块设备的驱动程序调用的是[`virtio-drivers`](https://docs.rs/virtio-drivers/latest/virtio_drivers/)中的`*_nb`（非阻塞）函数，然后通过中断通知来实现异步操作。VirtIO块设备中包含一个Virt队列，分为提交队列跟完成队列，类似Linux的io-uring实现。

驱动过程
  1. 提交：线程 A 提交任务给提交队列, 然后睡眠等待通知；
  2. 等待中断；
  3. 完成：
     1. 设备发出中断信号；
     2. 驱动设备维护线程 B 接受信号，将对应的请求操作结构体从提交队列移入结束队列，唤醒 A；
     3. A 检查结束队列中的最新任务是否和自己符合，若符合则将其删除，完成操作并返回。

看上去不错？确实已经通过了测试，但这样的实现其实有 bug！

线程 A 有可能被取消，此时缓冲区就会被提早释放，如果设备此时还在使用该地址，那么实际上就造成了use-after-free的问题。这个问题可以留给同学们思考如何解决，而我们已经在后续的代码中解决了。

而PLIC中断管理，实际上就是对[文档](https://github.com/riscv/riscv-plic-spec)的一层包装，在此不再赘述。

### 其他的一些模块

- `kalloc` : 内核和Rust语言自用的内核堆分配器；
- `klog` : 一些日志和输出的宏和函数；
- `rv39-paging` : RISC-V 的 Sv39 页表机制；
- `config` : 一些参数；
- `hart-id` : 存储 hart-id；
- `rand-riscv` : 随机数生成函数；
- `sygnal` : 尚不完备的信号处理机制。

### `kernel`

这是内核程序的crate，也是各个模块整合的终点。在这个crate中，逐个介绍一些比较重要的设计点。

#### 内核态陷阱

由于我们的内核运行在无栈协程的异步上下文中，所以不会有显式的抢占来通知定时器和中断。这个时候我们就必须依赖内核态中断，也就是中断重入的方式来进行操作。通过`ksync::critical`函数我们可以控制中断重入的时机，只要在该函数的最外层之外，中断重入在内核态的任意时刻都可以发生。

而对于内核态中断的处理，我们也仅需要更新定时器队列和中断通知即可。注意的一点是，该处理程序仍然是传统的函数调用形式，也即运行在同步上下文中，因此线程的切换在该环境中也然是不可行的。

#### 内存管理

除了直接引入`kmem`crate，内核中还针对系统调用实现了用户指针，分为`UserPtr`和`UserBuffer`，通过这两者我们可以安全地读取用户空间中的内容而不怕被恶意攻击。

- `UserPtr`依靠手动设置`UA_FAULT`TLS变量来控制访问出错时程序控制流的跳转。在对用户指针的访问开始时，`UA_FAULT`被设置为一个特定的地址；如果访问出错，则内核态陷阱将会发生，此时内核态陷阱处理程序会访问`UA_FAULT`，将出错地址设置在a0中并跳转到指定的地址。而在`trap.S`中，该函数地址根特殊的用户访问函数（`_checked_copy`和`_checked_zero`）被定义在一起，并且二者均不对sp有任何访问，因此程序控制流可以无损地返回。

```asm
// trap.S

.global _checked_copy
.type _checked_copy, @function
_checked_copy:
.Lcopy_loop:
    beqz a2, .Lcopy_ret
    lb t0, 0(a0)
    sb t0, 0(a1)
    addi a0, a0, 1
    addi a1, a1, 1
    addi a2, a2, -1
    j .Lcopy_loop
.Lcopy_ret:
    li a0, 0
    ret

.global _checked_zero
.type _checked_zero, @function
_checked_zero:
    mv t0, a0
.Lzero_loop:
    beqz a2, .Lzero_ret
    sb t0, 0(a1)
    addi a0, a0, 1
    addi a1, a1, 1
    addi a2, a2, -1
    j .Lzero_loop
.Lzero_ret:
    li a0, 0
    ret

.global _checked_ua_fault
.type _checked_ua_fault, @function
_checked_ua_fault:
    ret
```

- `UserBuffer`则更为简单直接。因为用户态的缓冲区可能特别大，也可能是向量化的缓冲区（`iovec`），因此简单地复制缓冲区是非常耗费资源的。所以这里直接提取出缓冲区对应的物理地址页面列表，并通过恒等映射换到对应内核空间中的地址，当成普通的缓冲区直接进行操作。

#### 任务（线程）的结构

在我们的视角中，在OS的实现里线程的控制块包含**信息部分**和**状态部分**。其中信息部分对外共享，而状态部分则是仅对自身可见。在以往用Rust语言实现的OS中，往往将信息部分和状态部分都放置在同一个结构体中，而对其中可变的部分（状态部分跟一部分信息部分）则加上一把大锁，简单粗暴，但是很没必要：状态部分仅对自身可见，完全可以取得独占的可变访问，为什么还要特地加一把大锁呢？然而，如果直取`unsafe`或者`RefCell`来强行访问，又让Rust独特的生命周期特性显得可有可无。

在`art`和`co-trap`的讲解中，我们已经知道，这里所有的任务跑在一个异步的事件循环中，退出循环意味着任务退出。因此，我们没必要将**状态部分**（包括用户态保存的寄存器众）单独分配在堆上进行手动切换，而是直接当作局部变量传入主循环函数，这将让任务状态和寄存器作为整个无栈协程的状态机的一部分被一起保存，一起分配，一起加载，不仅省去了单独切换的麻烦，减少了内存分配的频率，还可以作为含生命周期的独占可变引用传进需要线程状态的任意函数（包括），最大化利用Rust的借用检查，既高效又安全。

```rust
//! task/future.rs

pub async fn user_loop(mut ts: TaskState, mut tf: TrapFrame) {
    'life: loop {
        ..

        let (scause, ..) = co_trap::yield_to_user(&mut tf);

        ..

        match handle_scause(scause, &mut ts, &mut tf).await {
            Continue(Some(sig)) => ts.task.sig.push(sig),
            Continue(None) => {}
            Break((code, sig)) => {
                let _ = ts.task.event.send(&TaskEvent::Exited(code, sig)).await;
                log::trace!("Sent exited event {code} {sig:?}");
                break 'life;
            }
        }

        ..
    }
}
```

而对于其中的信息部分，由于需要对外共享，因此还是需要保存在引用计数指针中，并为其中可变的部分加锁。当然，尽量用细粒度锁甚至原子、无锁结构自然是最好的。

```rust
//! task.rs

/// 任务状态，本地变量，在Rust的借用检查下可变。
pub struct TaskState {
    pub(crate) task: Arc<Task>,
    sig_mask: SigSet,
    pub(crate) brk: usize,

    system_times: u64,
    user_times: u64,

    pub(crate) virt: Pin<Arsc<Virt>>,
    sig_actions: Arsc<ActionSet>,
    files: Files,
    tid_clear: Option<UserPtr<usize, Out>>,
    exit_signal: Option<Sig>,
}

/// 任务之间共享的信息，需要对其中可变的部分加锁，或是利用原子、无锁结构。
pub struct Task {
    main: Weak<Task>,
    parent: Weak<Task>,
    children: spin::Mutex<Vec<Child>>,
    tid: usize,

    sig: Signals,
    event: Broadcast<SegQueue<TaskEvent>>,
}

```

其中一个比较特殊的是`virt`，也即地址空间切换。在`kmem`的讲解中我们已经知道了`Virt`的加载方式，而这种加载方式并不适用于上述情况。因为无栈协程的每一次唤醒，每一次poll都需要加载地址空间，因为可能会访问到用户空间的地址。因此`Virt::load`函数并不能直接写在主循环中，而应当对循环进行一个包装，在poll函数的前面手动加载地址空间。

```rust
//! task/future.rs

#[pin_project]
pub struct TaskFut<F> {
    virt: Pin<Arsc<Virt>>,
    #[pin]
    fut: F,
}

impl<F> TaskFut<F> {
    pub fn new(virt: Pin<Arsc<Virt>>, fut: F) -> Self {
        TaskFut { virt, fut }
    }
}

impl<F: Future> Future for TaskFut<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let old = unsafe { self.virt.clone().load() };
        if let Some(old) = old {
            ..
        }
        self.project().fut.poll(cx)
    }
}

```

#### 设备和文件系统

SBI会在内核的入口地址传入设备树信息的物理地址，我们使用该地址来初始化各种设备。

学习系统调用表的方式，我们可以重用`ksc`中的`Handler`来直接注册初始化过程（使用`compatible`字段作为键），并在实际的初始化函数中直接调用。

```rust
//! dev.rs

static DEV_INIT: Lazy<Handlers<&str, &FdtNode, bool>> = Lazy::new(|| {
    Handlers::new()
        .map("riscv,plic0", intr::init_plic)
        .map("virtio,mmio", virtio::virtio_mmio_init)
});

pub unsafe fn init(fdt_base: *const ()) -> Result<(), FdtError> {
    static FDT: Once<Fdt> = Once::new();
    let fdt = FDT.try_call_once(|| unsafe { fdt::Fdt::from_ptr(fdt_base.cast()) })?;

    // Some devices may depend on other devices (like interrupts), so we should keep
    // trying until no device get initialized in a turn.

    let mut nodes = fdt.all_nodes().collect::<Vec<_>>();
    let mut count = nodes.len();
    loop {
        if nodes.is_empty() {
            break;
        }

        nodes.retain(|node| {
            if let Some(compat) = node.compatible() {
                let init = compat.all().any(|key| {
                    let ret = DEV_INIT.handle(key, node);
                    matches!(ret, Some(true))
                });
                if init {
                    log::debug!("{} initialized", node.name);
                }
                return !init;
            }
            false
        });

        if count == nodes.len() {
            break;
        }
        count = nodes.len();
    }

    Ok(())
}

```

第三方crate以及设备树参考：
[fdt](https://docs.rs/fdt/0.1.5/fdt/)
[设备树的文档](https://devicetree-specification.readthedocs.io/en/latest/index.html)

而对于文件系统，我们也直接建立了串口、管道、DevFS等对应的文件系统结构。其中串口文件作为`klog`模块的简单包装，管道也仅是对`kmem::Phys`的简单包装，而DevFS则是囊括了`umifs::misc`和块设备中的所有文件节点。


## 常见问题与细节

### 启动时的地址转换

由于我们的内核是放在高位的地址空间，而启动时SBI会将我们放到低位地址空间中，这不仅需要从低到高进行一个长跳转，而且会导致编译出来的符号表地址的不统一。

1. 为了保证长跳转不出错，我们使用一个启动页表，同时映射低地址和高地址，并在入口先加载该页表，跳转之后再换成其他页表或者抹去低地址的页表；
2. 为了保证符号的统一性，我们将内核编译成静态的PIE（Position-Independent Executable），即不依赖绝对地址的可执行文件。这样可以将所有的函数调用和跳转转换成相对PC的寻址模式，具体表现为汇编出包含`auipc`指令的代码。并且如果不能控制GOT（全局偏移表）的生成，我们还需要在链接选项中添加诸如`--apply-dynamic-relocs`和`-Ztls-model=local-exec`等选项，并且在链接脚本中制定最终的加载地址，在静态连接时就确定GOT中表项的值，从而避免我们程序运行时再麻烦地动态设置。

### 每个模块内部的单元测试

为了不依赖于内核来测试各个模块，我们在一些模块内部实现了单元测试，直接运行在宿主机（工作机器）平台下。

### 第三方依赖的自动下载

在此直接奉上自动下载第三方依赖项的脚本，帮助同学们避坑。使用时注意替换其中可能与自己项目不一致的内容。

```bash
# scripts/revendor.sh

#! /bin/bash

RUST_DIR="$(rustc --print=sysroot)"

# Rust标准库（包括core和alloc）自身也会包括一些依赖项，这些依赖项的版本被固定在这个Cargo.lock中，会被
# cargo硬编码监测。需要将其复制到这些core、alloc或者test等的根目录下来保证和Cargo.toml的一致性，不然
# 在执行cargo vendor的时候，会更新一些不被该Cargo.lock认可的依赖项，造成vendor换源之后编译失败。
cp -f "$RUST_DIR"/lib/rustlib/src/rust/Cargo.lock \
    "$RUST_DIR"/lib/rustlib/src/rust/library/test/

mkdir -p .cargo
cp -rf cargo-config/* .cargo

# 这里的`scripts/config.patch.toml`是项目中非crates.io中的依赖项的源信息。具体示例在下文。
cp -f scripts/config.patch.toml .cargo/config.toml

rm -rf third-party/vendor

cargo update

# 实际的下载操作，cargo会自动检测你工作区中所有项目的依赖然后一起下载到指定目录中，在这里是
# third-party/vendor。
cargo vendor third-party/vendor \
    --respect-source-config --versioned-dirs \
    -s $RUST_DIR/lib/rustlib/src/rust/library/test/Cargo.toml \
    >> .cargo/config2.toml

mv -f .cargo/config2.toml .cargo/config.toml

# 这里的`scripts/config.toml`里包含公用的编译选项。比如LTO、编译参数设置等等。
cat scripts/config.toml >> .cargo/config.toml

cp -rf .cargo/* cargo-config
```
运行这个脚本之后，当前所有的第三方库源将会全部被替换成本地源。如果想要改回网络下载，将你的`scripts/config.toml`替换掉`cargo-config`中对应文件即可。

```toml
# scripts/cargo.patch.toml

# 比如说我的项目中依赖了我自己在Github上event-listener的fork，但是我可能又会引用一些依赖crates.io上的
# 该项目官方源的第三方库。这个时候如果直接进行cargo vendor就会造成多个源同时存在使得vendor失败。因此我们
# 需要将指向crate.io的该项目的源也替换成我自己的fork，从而解决冲突。
[patch.crates-io]
event-listener = { git = "https://github.com/js2xxx/event-listener"}
```

## 参考项目

- 往届的项目：[Maturin](https://gitlab.eduxiji.net/scPointer/maturin)，[FTL OS](https://gitlab.eduxiji.net/DarkAngelEX/oskernel2022-ftlos)；
- 商业和开源项目：Linux，Fuchsia；
- 自己之前写的OS：[oceanic](https://github.com/js2xxx/oceanic)。
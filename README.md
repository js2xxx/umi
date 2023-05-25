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
- `target` - cargo的生成目录；
- `third-party/bin` - RustSBI的BIOS二进制文件；
- `third-party/img`- 初赛评测程序的磁盘映像文件。

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

## 开发过程

我们的OS是从接近RISC-V的硬件底层根SBI标准开始，先完成大部分模块会用到的公用底层模块的实现，然后将每一个模块逐个设计与实现，最后整合进内核，完成系统调用的编写，辅以简单的调试并通过初赛。接下来将按照开发时间顺序逐个讲解各个模块。

## 模块讲解

### `mizu/kernel`

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
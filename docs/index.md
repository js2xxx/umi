# UMI: 全国赛第一阶段文档

## 开发人员

队伍：PLNTRY，西安交通大学
队员：徐启航，杨豪（中途退赛）

## 项目结构

- `.github/workflows` - Github Actions配置；
- `.vscode` - VS Code工作区配置；
- `cargo-config` - 根目录cargo配置，会在make时复制到.cargo目录中；
- `debug` - 调试信息，包括内核文件的反汇编、ELF元数据还有QEMU的输出日志；
- `docs` - 文档；
- `mizu/dev` - 各个设备驱动程序代码；
- `mizu/kernel` - 内核主程序代码；
- `mizu/lib` - 内核各个模块的代码；
- `scripts` - 包含第三方依赖的换源脚本；
- `target` - cargo的生成目录；
- `third-party/bin` - RustSBI的BIOS二进制文件；
- `third-party/vendor` - 第三方库的依赖；
- `third-party/img`- 初赛评测程序的磁盘映像文件。

由于分支原因，第三方库的依赖和换源脚本并不实际包含于以上目录中。

## 编译&运行&调试

依赖项：比赛要求的Rust工具链, make, cargo-binutils（通过`cargo install`安装）, riscv64-unknown-elf的GNU工具链。

为了减小仓库总体积，评测程序的磁盘映像文件并没有作为仓库的一部分，需要单独将其作为`sdcard-comp2.img`复制到`third-party/img`目录下，或者手动修改根目录的Makefile。

单独编译：
```bash
make all # MODE=release(默认)|debug BOARD=qemu-virt(默认)|cv1811h
```

直接运行：
```bash
# MODE 同上
make run BOARD=qemu-virt # 在qemu虚拟机内运行
make run BOARD=cv1811h # 将os.bin复制到/srv/tftp目录中，以便开发板的uboot取用
```

qemu上的调试：
```bash
make debug MODE=debug # 一个终端
riscv64-unknown-elf-gdb debug/mizu.sym # 另一个终端
```

在编译目标开发板是`qemu-virt`的情况下，OS的输出文件被配置在了`debug/qemu.log`文件中，而终端中的QEMU作为一个监视器可以查看实时的运行信息（键入help可以查看所有命令）。

## 开发过程&结构设计

我们的OS是从接近RISC-V的硬件底层根SBI标准开始，先完成大部分模块会用到的公用底层模块的实现，然后将每一个模块逐个设计与实现，最后整合进内核，完成系统调用的编写，辅以简单的调试并通过初赛。之后通过增加对应的系统调用辅以调试通过全国赛第一阶段的qemu赛道，再拆分出设备驱动程序、为每个模块适配功能以通过cv1811h赛道。

## 模块讲解

### 前言：无栈协程

我们OS的内核态运行在异步的无栈协程上下文中。接下来的文档，如没有提到OS相关概念，一律认为跟代码运行所处的环境无关（即不管是实现OS还是编写用户程序都是通用的）。

无栈协程是一个巨大的状态机，它不需要单独的栈来保存执行的上下文，而是将局部变量跟状态机一起保存。在Rust中，无栈协程以`core::future::Future` trait来表示：

```rust
pub trait Future {
    type Output;
    
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>;
}
```

其中`fn poll`是一次对无栈协程任务的执行，返回一个`Poll`表示`Pending`（有操作需要等待，需要切换上下文）和`Ready`（任务完成，返回结果）。

当一个任务被切换走后，如何重新唤醒呢？`cx`这个参数包含一个`Waker`（唤醒句柄），如果一个Future需要等待某个事件（返回`Pending`），则会将该`waker`注册到这个事件的等待队列中。在事件通知的代码中，调用该`waker.wake()`方法便可将该`Future`代表的异步任务重新放回等待执行的任务队列中。

在Rust中，一个异步函数`async fn`最终都会编译成一个独立的状态机。而在异步函数中调用异步函数，则需要使用`.await`关键字。因此，每一次`.await`的出现就代表一个可能的任务间上下文切换。

### 目录

我们的OS秉承模块化的设计，在`mizu`目录下分出三块：

- `kernel`是最终二进制文件的可执行程序，依赖所有模块并有一些自身的代码逻辑；
- `lib`是各个模块的所在，一些模块完全独立可被复用，而另一些依赖着其他模块，作为单独的功能代码便于解耦调试；
  - [`ksync`](lib/ksync.md)
  - [`ktime`](lib/ktime.md)
  - [`co-trap`](lib/co-trap.md)
  - [`art`](lib/art.md)
  - [`ksc`](lib/ksc.md)
  - [`devices`](lib/devices.md)
  - [`kmem`](lib/kmem.md)
  - [其他模块](lib/misc.md)
- `dev`是各个设备驱动程序的所在，依赖着`lib`中的模块。
  - [`virtio`](dev/virtio.md)，包括块设备和网络设备
  - `plic`，是对[文档](https://github.com/riscv/riscv-plic-spec)的一层包装。
  - [`SD卡`](dev/sdmmc.md)
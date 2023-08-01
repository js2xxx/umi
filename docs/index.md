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

### 模块目录

我们的OS秉承模块化的设计，在`mizu`目录下分出三块：

- `lib`是各个模块的所在，一些模块完全独立可被复用，而另一些依赖着其他模块，作为单独的功能代码便于解耦调试；
  - [`ksync`](lib/ksync.md)：同步源语
  - [`ktime`](lib/ktime.md)：时间相关
  - [`co-trap`](lib/co-trap.md)：上下文切换
  - [`art`](lib/art.md)：异步执行器
  - [`ksc`](lib/ksc.md)：系统调用相关
  - [`devices`](lib/devices.md)：设备抽象和网络协议栈
  - [`kmem`](lib/kmem.md)：内存管理
  - [`afat32`](lib/afat32.md)：异步并发的FAT32文件系统。
  - [其他模块](lib/misc.md)
- `dev`是各个设备驱动程序的所在，依赖着`lib`中的模块；
  - [`virtio`](dev/virtio.md)，包括块设备和网络设备
  - `plic`，是对[文档](https://github.com/riscv/riscv-plic-spec)的一层包装。
  - [`SD卡`](dev/sdmmc.md)
- [`kernel`](kernel/index.md)是最终二进制文件的可执行程序，依赖所有模块并有一些自身的代码逻辑。

## 常见问题与细节

### 启动时的地址转换

由于我们的内核是放在高位的地址空间，而启动时SBI会将我们放到低位地址空间中，这不仅需要从低到高进行一个长跳转，而且会导致编译出来的符号表地址的不统一。

1. 为了保证长跳转不出错，我们使用一个启动页表，同时映射低地址和高地址，并在入口先加载该页表，跳转之后再换成其他页表或者抹去低地址的页表；
2. 为了保证符号的统一性，我们将内核编译成静态的PIE（Position-Independent Executable），即不依赖绝对地址的可执行文件。这样可以将所有的函数调用和跳转转换成相对PC的寻址模式，具体表现为汇编出包含`auipc`指令的代码。并且如果不能控制GOT（全局偏移表）的生成，我们还需要在链接选项中添加诸如`--apply-dynamic-relocs`和`-Ztls-model=local-exec`等选项，并且在链接脚本中制定最终的加载地址，在静态连接时就确定GOT中表项的值，从而避免我们程序运行时再麻烦地动态设置。

### 每个模块内部的单元测试

为了不依赖于内核来测试各个模块，我们在一些模块内部实现了单元测试，直接运行在宿主机（工作机器）平台下。

### 第三方依赖的自动下载

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
- 徐启航同学之前写的OS：[oceanic](https://github.com/js2xxx/oceanic)。
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
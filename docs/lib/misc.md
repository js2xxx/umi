# 其他的一些模块（杂项）

- `umio`抽象出一个读写相关的抽象trait `Io`，表示一切可读写的数据结构，以及从标准库扒来的`SeekFrom`、`IoSlice`等基础类型；
- `umifs`实现了umi的虚拟文件系统，包括`FileSystem`、`Entry`、`Directory`、`DirectoryMut`等trait，实现了方便的路径解析，并兼容了Linux的文件类型
  - VFS虚拟文件系统在各类文件系统之上构建了一个抽象层，从而使操作系统可以挂载各类w文件系统；
- `kalloc` : 内核和Rust语言自用的内核堆分配器；
- `rv39-paging` : RISC-V 的 Sv39 页表机制；
- `config` : 设备相关的参数常量；
- `hart-id` : 存储 hart-id；
- `rand-riscv` : 随机数生成函数；
- `sygnal` : 信号处理机制。

# SD卡驱动

华山派cv1811h的SD卡设备是标准的MMIO SD Host Controller再加一点自定义的PINMUX控制寄存器。我们通过阅读华山派[官方镜像](https://github.com/sophgo/sophpi-huashan)中的uboot和Linux代码了解了具体的实现细节，SD卡的[官方文档](https://www.sdcard.org/downloads/pls/)了解了各个标准MMIO寄存器和SD卡命令的定义。最后编写了以ADMA2方式实现的中断驱动的SD卡块设备驱动。由于该驱动的实现仅是依靠标准，并没有过多的设计空间，其他部分和实现细节在此不过多赘述，在此只介绍ADMA2相关的部分。

## ADMA

在不依靠DMA的数据传输流程中，在提交了读写命令后，用户需要不断重复以下两个步骤直到传输结束：

1. 等待`READ(WRITE)_BUFFER_READY`中断；
2. 将数据读取或写入`BUF_DATA_PORT`寄存器。

通过该方式耗费的效率是不可想象的，既拖慢了传输，又大量占用了CPU时间。而通过DMA的方式可以不占用CPU时间，直接等待传输结束即可。ADMA2是囊括在SD卡标准中的内嵌DMA方式。在此简要介绍一下ADMA2的实现。

ADMA的实现方式中，设备通过在内存中的描述符表跟CPU互动。当用户在填充好描述符表内容、配置好ADMA2参数后，通过写入`COMMAND`寄存器提交读写命令，设备就会会读取这一描述符表。其中，每个描述符`addr`字段必须按4字节对齐，描述符表所有`len`字段的总长度必须为块大小的整数倍。

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(C, packed)]
pub struct Descriptor {
    attr: Attr,
    len: u16,
    addr: u64,
    _reserved: u32,
}

const_assert_eq!(mem::size_of::<Descriptor>(), mem::size_of::<u128>());

bitflags! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Attr: u16 {
        const VALID = 1 << 0;           // 该描述符有效
        const END = 1 << 1;             // 指示描述符表的结束
        const GEN_INTR = 1 << 2;        // 执行该描述符的时候强制生成ADMA中断

        const ACTION_NONE = 0;          // 该描述符不操作
        const ACTION_RSVD = 0b010_000;  // 保留操作
        const ACTION_XFER = 0b100_000;  // 代表一个传输操作，结构体其他字段有效
        const ACTION_LINK = 0b110_000;  // 代表不按顺序读取，而是通过接口体中
                                        // addr字段读取真正描述符的地址（链表模式）

        const ACTION_MASK = 0b111_000;
    }
}
```

一个描述符代表一个设备的操作。设备将会按给定的顺序读取描述符并完成相应的操作。所有操作完成后设备将会产生`TRANSFER_COMPLETE`中断。CPU接收这一中断后通知用户完成操作。

## 其他实现细节

PINMUX和SD卡电源、时钟还有速度的配置尤为重要。团队在实现该驱动的时候由于没有详细掌握配置参数，导致很长一段时间SD卡配置启动不成功，花费了大量精力。
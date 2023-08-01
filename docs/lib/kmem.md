# kmem

kmem是umi的内存管理模块，包含三大部分：页帧分配、物理页帧管理和地址空间管理。

## 页帧分配

使用经典的无锁链表分配页帧，比`kalloc`引用的分配器效率更高。

## 页帧管理

参考Fuchsia实现了RAII的二叉树形结构内存管理模型。每一个`Phys`结构体，包含一个页帧哈希表，一个父节点（可能为另一个`Phys`或者一个I/O后端），一个刷新器（背景任务），还有一些状态信息。

每一个`Phys`逻辑上是其父`Phys`的一个切片。页帧通过引用计数和缓存状态的更新在树形结构中复制和流动。

基本操作有提交、回写页面，以及在此之上包装的I/O操作。

```rust
#[derive(Clone)]
enum Parent {
    Phys {
        phys: Arsc<Phys>,
        start: usize,
        end: Option<usize>,
    },
    Backend(Arc<dyn Io>),
}

#[derive(Debug)]
struct FrameList {
    parent: Option<Parent>,
    frames: HashMap<usize, FrameInfo, RandomState>,
}

#[derive(Debug, Clone)]
struct Flusher {
    sender: Sender<SegQueue<FlushData>>,
    flushed: Arsc<Event>,
    offset: usize,
}

#[derive(Debug)]
pub struct Phys {
    branch: bool,
    list: Mutex<FrameList>,
    position: AtomicUsize,
    cow: bool,
    flusher: Option<Flusher>,
}
```

### 提交（获取）页面

按顺序依次：

- 从自身的页帧哈希表获取，若获取到了则更新页帧缓存信息；
- 使用递归方式从父节点获取，若父节点是个读写后端则从后端读出页面；
- 创建空白页面。

### 刷新（回写）页面

按顺序依次从自身往父节点往上向回写背景任务发送回写数据，并等待回写完成。

### 克隆（切片）结构体

并不直接为该`Phys`创建子结点，而是创建一个新的非公开树枝节点，将当前`Phys`和新创建的节点当成新树枝节点的两个子节点。这种方式的好处是销毁一个公开的`Phys`不会顺便销毁子树，坏处是会使得树形结构不断增长，因此必须在提交和刷新操作前先合并链型的树枝节点。

### 回写背景任务

由于Rust不支持异步`Drop`，因此必须有一个回写背景任务，这样在销毁Phys时发送完回写数据就可以直接返回。坏处是大量占用内核堆资源，并且可能会回写不及时。

## 地址空间管理

我们采用了当前主流操作系统类似的策略：将内核映射到地址空间的高位，从而实现了内核和用户地址的隔离。

虚拟地址空间管理的结构体持有一个根页表和一个`RangeMap`，即以一个范围为键的键值映射表，对每一个分配的地值范围建立一个映射结构体。

```rust
struct Mapping {
    phys: Arsc<Phys>,
    start_index: usize,
    attr: Attr,
}

pub struct Virt {
    root: Mutex<Frame>,
    map: RwLock<RangeMap<LAddr, Mapping>>,
    cpu_mask: AtomicUsize,

    _marker: PhantomPinned,
}
```

该结构体包含创建映射、提交映射、重设映射权限、释放映射等操作，实现了映射懒分配的功能。其中`RangeMap`结构支持地址空间随机化（ASLR）的分配策略，基于标准库的`BTreeMap`建立了一个键值表，并附带了ASLR的基本实现，从而提升了内核安全性，防止被缓存溢出侵略。另外，提交映射的方式也有一些区别，在此详细讲述。

### 提交映射

之前的提交映射方式是直接按照`RangeMap`中的映射更新页表项完事，然而在使用用户缓冲区时会发生问题：实际操作用户缓冲区的时机可能并不与提交映射的时机相同，有可能在使用时该缓冲区已经被另一个线程取消映射了。造成的页错误发生在内核空间，没有挽回的余地（二阶中断处理不能发生在异步上下文，因此异步的映射函数无法调用，错误的I/O操作无法中止）。因此除了传统的直接映射页表方式，还有另一种，在使用用户缓冲区的时候保持持有地址空间的读锁，使得其他线程无法改变地址空间，虽然在一定程度上拖慢了效率，但是增加了缓冲区安全性。

```rust
pub struct VirtCommitGuard<'a> {
    map: Option<RwLockUpgradableReadGuard<'a, RangeMap<LAddr, Mapping>>>,
    virt: &'a Virt,
    attr: Attr,
    range: Vec<SliceRepr>,
}
unsafe impl Send for VirtCommitGuard<'_> {}

impl<'a> VirtCommitGuard<'a> {
    pub async fn push(&mut self, range: Range<LAddr>) -> Result<(), Error> {
        // 添加缓冲区地址范围并更新页表。
        ...
    }

    pub fn as_slice(&mut self) -> &mut [&'a [u8]] {
        assert!(self.attr.contains(Attr::READABLE));
        unsafe { mem::transmute(self.range.as_mut_slice()) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [&'a mut [u8]] {
        assert!(self.attr.contains(Attr::WRITABLE));
        unsafe { mem::transmute(self.range.as_mut_slice()) }
    }
}
// Drop时会自动释放读锁。
```

### 页表缓存

地址空间的加载卸载和页表缓存的刷新需要与硬件平台和SBI直接交互。其中加载方式便是为每个CPU分配一个TLS变量储存当前的地址空间结构，在更新`satp`的时候顺带更新该变量以转移所有权。

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

## DMA 平台操作

华山派对内存的高速缓存更新不是那么智能，因此在DMA（比如SD卡的ADMA2）场景下需要手动更新高速缓存。相关函数如下：

```rust
pub fn thead_clean(base: LAddr) {
    unsafe { asm!(".insn r 0b1011, 0, 1, zero, {}, x4", in(reg) base.val()) }
}

pub fn thead_flush(base: LAddr) {
    unsafe { asm!(".insn r 0b1011, 0, 1, zero, {}, x7", in(reg) base.val()) }
}

pub fn thead_sync_s() {
    unsafe { asm!(".long 0x0190000b") }
}
```
# afat32

Async FAT32, 参考[`rust-fatfs`](https://github.com/rafalh/rust-fatfs)实现的异步并发的FAT32文件系统。通过将原本代码中的`RefCell`等结构换成了互斥等同步源语和引用计数指针，实现了同时读写不同文件的功能。在此简要介绍一下扩展的内部功能：批量操作FAT。

# 批量操作FAT

FAT32文件系统的文件分配表（FAT）是其核心结构，控制着簇（块）的分配。原本的实现中，一次只能从设备读取一条FAT条目。虽然有`Phys`对块设备页面缓存的加持，基于链表读写操作还是不够快。因此，我们引入了批量的功能，一次可以读取、更新至多1024条（1024*4=4096刚好是一个页面）的条目，极大地加快了簇的分配以及依赖该功能的文件创建和写入功能。

```rust
impl Fat {
    /// # Safety
    ///
    /// The buf must be written zeros.
    async unsafe fn get_range_raw(
        &self,
        start: u32,
        buf: &mut [MaybeUninit<u32>],
    ) -> Result<usize, Error> {
        let end = (start + u32::try_from(buf.len())?).min(self.allocable_range().end);
        if start > end {
            return Err(EINVAL);
        }
        if start == end {
            return Ok(0);
        }
        let read_len = (end - start) as usize;
        let bytes = MaybeUninit::slice_as_bytes_mut(&mut buf[0..read_len]);

        self.device
            .read_exact_at(self.offset(0, start), unsafe {
                MaybeUninit::slice_assume_init_mut(bytes)
            })
            .await?;

        Ok(read_len)
    }

    pub async fn get_range<'a>(
        &self,
        start: u32,
        buf: &'a mut [u32],
    ) -> Result<impl Iterator<Item = (u32, FatEntry)> + Send + Clone + 'a, Error> {
        buf.fill(0);
        // SAFETY: init to uninit is safe.
        let len = unsafe { self.get_range_raw(start, mem::transmute(&mut *buf)) }.await?;

        let zip = buf[..len].iter().zip(start..);
        Ok(zip.map(|(&raw, cluster)| (cluster, FatEntry::from_raw(raw, cluster))))
    }

    pub async fn set_range(
        &self,
        start: u32,
        buf: &mut [u32],
        entry: impl IntoIterator<Item = FatEntry>,
    ) -> Result<(), Error> {
        buf.fill(0);

        let _set = self.set_lock.lock().await;

        let len = unsafe { self.get_range_raw(start, mem::transmute(&mut *buf)) }.await?;

        for ((raw, cluster), entry) in buf[..len].iter_mut().zip(start..).zip(entry) {
            let old = *raw & 0xf000_0000;
            *raw = entry.into_raw(cluster, old)
        }

        // SAFETY: init to uninit is safe.
        let buf: &[MaybeUninit<u32>] = unsafe { mem::transmute(&buf[..len]) };
        // SAFETY: All bytes are valid.
        let bytes: &[u8] =
            unsafe { MaybeUninit::slice_assume_init_ref(MaybeUninit::slice_as_bytes(buf)) };

        try_join_all((0..self.mirrors).map(|mirror| async move {
            let offset = self.offset(mirror, start);
            self.device.write_all_at(offset, bytes).await
        }))
        .await?;

        Ok(())
    }
}
```
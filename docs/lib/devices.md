# `devices`

该模块提供各个驱动程序必要的抽象，包括块设备和网络设备的抽象；和一个以修改过的[`smoltcp`](https://github.com/js2xxx/smoltcp/tree/reuse_endpoints)为基底的支持多设备的网络协议栈。其中，由于时间和精力原因，块设备的抽象仅是一个最基本的包括读写的`async-trait`来保证功能，在效率上有所欠缺，因此将重点放在网络设备和协议栈上。

## 网络设备驱动接口

网络设备驱动接口学习了`smoltcp`和[`embassy-net`](https://github.com/embassy-rs/embassy/tree/main/embassy-net)中的接口定义方式，将网络设备抽象为`Net`、`NetTx`和`NetRx`三个包含`core::task::Context`的非异步trait，使其既可以直接作一层对`smoltcp`设备的包装，又可以克服了`smoltcp`的设备接口的缺点——支持动态派发。

```rust
pub trait Net: Send + Sync {
    fn features(&self) -> Features;

    fn address(&self) -> [u8; 6];

    fn ack_interrupt(&self);

    fn is_link_up(&self) -> bool;

    fn queues(&mut self) -> (&mut dyn NetTx, &mut dyn NetRx);
}

pub trait NetTx: Send {
    fn tx_peek(&mut self, cx: &mut Context<'_>) -> Option<Token>;
    fn tx_buffer(&mut self, token: &Token) -> &mut [u8];
    fn transmit(&mut self, token: Token, len: usize);
}

pub trait NetRx: Send {
    fn rx_peek(&mut self, cx: &mut Context<'_>) -> Option<Token>;
    fn rx_buffer(&mut self, token: &Token) -> &mut [u8];
    fn receive(&mut self, token: Token);
}
```

其中通过非`Copy`的`Token`的方式解决了分配设备缓冲区、填充内容和实际收发操作之间的配合关系。这一点在`virtio`的实现中也有体现。

```rust
#[derive(Debug)]
pub struct Token(pub usize);
```

之后便可直接将此接口包装为`smoltcp`的物理设备：

```rust
pub(in crate::net) trait NetExt: Net {
    fn with_cx<'device, 'cx>(
        &'device mut self,
        cx: Option<&'device mut Context<'cx>>,
    ) -> WithCx<'device, 'cx, Self> {
        WithCx { cx, device: self }
    }
}
impl<T: Net + ?Sized> NetExt for T {}

pub(in crate::net) struct WithCx<'device, 'cx, T: Net + ?Sized> {
    cx: Option<&'device mut Context<'cx>>,
    device: &'device mut T,
}

pub(in crate::net) struct TxToken<'driver> {
    device: &'driver mut dyn NetTx,
    token: Token,
}

pub(in crate::net) struct RxToken<'driver> {
    device: &'driver mut dyn NetRx,
    token: Token,
}

impl<'device, 'cx, T: Net + ?Sized> phy::Device for WithCx<'device, 'cx, T> {
    type RxToken<'a> = RxToken<'a> where Self: 'a;

    type TxToken<'a> = TxToken<'a> where Self: 'a;

    fn receive(&mut self, _: Instant) -> Option<(RxToken<'_>, TxToken<'_>)> { ... }

    fn transmit(&mut self, _: Instant) -> Option<TxToken<'_>> { ... }
    ...
}

impl phy::TxToken for TxToken<'_> { ... }

impl phy::RxToken for RxToken<'_> { ... }


// 接口的实现不再赘述，详情参考 mizu/lib/devices/src/net/driver.rs
```

## 网络协议栈

`smoltcp`原生支持对`core::task::Waker`的使用和唤醒，但是并没有直接的异步接口。于是我们参照`embassy-net`开发出了具有多设备统一的网络协议栈，在单独的一个背景任务里进行协议栈的异步轮询，目前支持DNS、UDP、TCP等插座类型、IPv4（静态和DHCP配置）和v6（静态配置）两个IP协议。目前进一步的优化思路是为每一个设备提供单独的背景轮询任务，因为时间原因尚未开展。

而本次测试用到的回环设备也作为在该网络协议栈中的一个特殊的独立设备，并不是简单类似`pipe`的双通道实现。在每个插座进行绑定IP地址和端口时，也会自动根据可用性选择对应的设备接口（比如`127.0.0.1`匹配到回环设备）。

```rust
pub struct Stack {
    devices: Vec<Arc<RwLock<dyn Net>>>,
    socket: RwLock<SocketStack>,
    states: RwLock<Vec<State>>,
}

#[derive(Debug)]
pub(in crate::net) struct State {
    link_up: bool,

    dhcpv4: Option<SocketHandle>,

    dns_socket: SocketHandle,
    dns_servers_ipv4: heapless::Vec<Ipv4Address, 3>,
    dns_servers_ipv6: heapless::Vec<Ipv6Address, 3>,
    dns_waker: AtomicWaker,
}

pub(in crate::net) struct SocketStack {
    pub sockets: SocketSet<'static>,
    next_local_port: u16,

    pub loopback: Tracer<Loopback>,
    pub ifaces: Vec<Interface>,

    waker: AtomicWaker,
}

// 实现省略，具体可以参考 mizu/lib/devices/src/net/stack.rs

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct StackBackground {
    stack: Arsc<Stack>,
    timer: Option<Timer>,
}

impl Future for StackBackground {
    type Output = Infallible;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            if let Some(timer) = self.timer.as_mut() {
                ready!(timer.poll_unpin(cx));
                self.timer = None;
            }
            match self.stack.poll(cx) {
                Some(ddl) if ddl.to_su() == (0, 0) => {
                    cx.waker().wake_by_ref(); // Wake up immediately.
                    break Poll::Pending;      // Yield for not occupying too much CPU resource.
                }
                Some(ddl) => self.timer = Some(ddl.into()),
                None => break Poll::Pending,
            }
        }
    }
}

```

## 插座

插座的实现方式有一些设计，但不多：UDP基本是对`smoltcp`的一层包装，TCP的系统调用接口则跟`smoltcp`大相径庭。为了支持TCP插座的ACCEPT操作，创建了一个ACCEPT队列，放在一个单独的背景任务里，潜在的优化点是可能可以跟网络协议栈的大背景任务合并。

```rust
#[derive(Debug)]
pub struct Socket {
    inner: Arsc<Inner>,
    accept: Mutex<Option<Receiver<SegQueue<Socket>>>>,
}

#[derive(Debug)]
struct Inner {
    stack: Arsc<Stack>,
    handle: RwLock<SocketHandle>,
    iface_id: AtomicUsize,

    listen: Mutex<Option<IpListenEndpoint>>,
    backlog: AtomicUsize,
    accept_event: Event,
}

impl Inner {
    async fn accept_task(
        self: Arsc<Self>,
        endpoint: IpListenEndpoint,
        tx: Sender<SegQueue<Socket>>,
    ) {
        while !tx.is_closed() {
            let mut listener = None;
            // Listen to `self.accept_event`.

            let establishment = poll_fn(|cx| self.poll_for_establishment(cx));
            let closed = pin!(self.close_event(&tx));

            let Either::Left((res, _)) = select(establishment, closed).await else { break };

            if let Ok(handle) = res {
                let conn = ...; // Create new smoltcp socket and add it to the stack.
                // 由于smoltcp的插座是单连接的，因此需要将已经建立连接的自身插座实例返
                // 回给发送给accept函数，将自身的插座实例替换为一个新的监听状态的插座。
                if let Some(conn) = conn {
                    let data = ...; // Create new connection socket interface.
                    if tx.send(data).await.is_err() {
                        break;
                    }
                }
            }
        }
        self.close().await
    }
}

impl Socket {
    pub fn listen(&self, backlog: usize) -> Result<impl Future<Output = ()> + 'static, Error> {
        let endpoint = ...; // Create the listen endpoint.

        self.inner
            .with_mut(|_, socket| socket.listen(endpoint))
            .map_err(|_| EINVAL)?;

        self.inner.backlog.store(backlog, SeqCst);
        let inner = self.inner.clone();
        let (tx, rx) = unbounded();
        ksync::critical(|| *self.accept.lock() = Some(rx));

        Ok(inner.accept_task(endpoint, tx)) // Return the accept background task to be spawned
                                            // in the kernel.
    }

    pub async fn accept(&self) -> Result<Self, Error> {
        let Some(rx) = ksync::critical(|| self.accept.lock().clone()) else {
            return Err(EINVAL);
        };
        let socket = rx.recv().await.unwrap();
        self.inner.accept_event.notify(1);
        Ok(socket)
    }
}

```

## 经过修改的`smoltcp`

在使用这个crate的时候，`iperf`测试的UDP的丢包率一直居高不下（20%左右）。经过排查发现是`smoltcp`中的UDP收包逻辑较为简单，没有伪remote的参与，而`iperf`使用多个插座连接同一个服务器端口，于是经常出现收发不均衡的现象，造成某些插座缓冲区满而丢包。于是在`smoltcp`中加入了伪远程端口字段，使得在UDP收包时优先匹配远程端口。经过测试将丢包率降到了个位数。

还在`smoltcp`中增加了检测收发缓冲区空闲大小的函数，以支持其poll事件功能。
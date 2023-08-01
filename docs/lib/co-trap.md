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

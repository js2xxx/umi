OUTPUT_ARCH(riscv)

SECTIONS
{
    . = KERNEL_START;
    .text : 
    { 
        KEEP(*(.init*));
        *(.text*);
        *(.plt*);
    } :text

    .dynamic : { *(.dynamic*) } :text :dynamic
    .dynsym : { *(.dynsym) }
    .gnu.hash : { *(.gnu.hash) }
    .hash : { *(.hash) }
    .dynstr : { *(.dynstr) }

    .rodata : ALIGN(8)
    {
        *(.srodata*);
        *(.rodata*);
    } :text

    .data : ALIGN(8)
    {
        _sidata = LOADADDR(.data);
        _sdata = .;
        PROVIDE(__global_pointer$ = . + 2K);
        *(.sdata*);
        *(.data*)
        _edata = ALIGN(8);
    } :data

    .tdata : ALIGN(8)
    {
        _stdata = .;
        *(.tdata*)
        *(.tbss*);
        _etdata = ALIGN(8);
        _tdata_size = _etdata - _stdata;
    } :data :tls

    .bss (NOLOAD) : ALIGN(8)
    {
        _sbss = .;
        *(.sbss*);
        *(.bss*);
        _ebss = ALIGN(8);

        _sstack = ALIGN(4K);
        . += _stack_size * MAX_HARTS;
        _estack = ALIGN(8);

        _sheap = ALIGN(4K);
        . += _heap_size;
        _eheap = ALIGN(8);
    }

    .tbss (NOLOAD) : ALIGN(8)
    {
        _stp = .;
        . += _tdata_size * MAX_HARTS;
        _etp = ALIGN(8);
    }

    .eh_frame (INFO) : { KEEP(*(.eh_frame)) }
    .eh_frame_hdr (INFO) : { *(.eh_frame_hdr) }
}

PHDRS
{
    phdr PT_PHDR PHDRS FLAGS(4);
    text PT_LOAD FLAGS(5);
    data PT_LOAD FLAGS(6);
    dynamic PT_DYNAMIC FLAGS(6);
    note PT_NOTE FLAGS(4);
    tls PT_TLS FLAGS(4);
}

PROVIDE(_heap_size = 24M);
PROVIDE(_stack_size = 4K);

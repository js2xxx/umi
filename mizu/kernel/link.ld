OUTPUT_ARCH(riscv)

SECTIONS
{
    .text : 
    { 
        KEEP(*(.init*));
        *(.text*);
        *(.plt*);
    } >VIRT AT>RAM :text
    .dynamic : { *(.dynamic*) } >VIRT AT>RAM :text :dynamic
    .dynsym : { *(.dynsym) } >VIRT AT>RAM
    .gnu.hash : { *(.gnu.hash) } >VIRT AT>RAM
    .hash : { *(.hash) } >VIRT AT>RAM
    .dynstr : { *(.dynstr) } >VIRT AT>RAM

    .rodata : ALIGN(0x10)
    {
        *(.srodata*);
        *(.rodata*);
    } >VIRT AT>RAM :text

    .data : ALIGN(0x10)
    {
        _sidata = LOADADDR(.data);
        _sdata = .;
        PROVIDE(__global_pointer$ = . + 2K);
        *(.sdata*);
        *(.data*)
        _edata = ALIGN(0x10);
    } >VIRT AT>RAM :data

    .tdata : ALIGN(0x10)
    {
        _stdata = .;
        *(.tdata*)
        *(.tbss*);
        _etdata = ALIGN(0x10);
        _tdata_size = _etdata - _stdata;
    } >VIRT AT>RAM :data :tls

    .bss (NOLOAD) : ALIGN(0x10)
    {
        _sbss = .;
        *(.sbss*);
        *(.bss*);
        _ebss = ALIGN(0x10);

        _sstack = ALIGN(4K);
        . += _stack_size * %MAX_HARTS%;
        _estack = .;

        _sheap = ALIGN(4K);
        . += _heap_size;
        _eheap = .;
    } >VIRT AT>RAM

    .tbss (NOLOAD) : ALIGN(0x10)
    {
        _stp = .;
        . += _tdata_size * %MAX_HARTS%;
        _etp = .;
    } >VIRT AT>RAM

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

MEMORY
{
    VIRT : ORIGIN = %KERNEL_START%, LENGTH = 128M
    RAM : ORIGIN = %KERNEL_START_PHYS%, LENGTH = 128M
}

PROVIDE(_heap_size = 24M);
PROVIDE(_stack_size = 16K);

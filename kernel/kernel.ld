OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(kernel_main)

KERNEL_PHYS_BASE = 0x02000000;
KERNEL_VIRT_BASE = 0xFFFF800000000000;

SECTIONS {
    . = KERNEL_PHYS_BASE;

    _kernel_start = .;

    .boot ALIGN(4096) : {
        KEEP(*(.multiboot))
        KEEP(*(.boot))
    }

    . += KERNEL_VIRT_BASE;

    .text ALIGN(4096) : AT(ADDR(.text) - KERNEL_VIRT_BASE) {
        _text_start = .;
        *(.text .text.*)
        _text_end = .;
    }

    .rodata ALIGN(4096) : AT(ADDR(.rodata) - KERNEL_VIRT_BASE) {
        _rodata_start = .;
        *(.rodata .rodata.*)
        _rodata_end = .;
    }

    .data ALIGN(4096) : AT(ADDR(.data) - KERNEL_VIRT_BASE) {
        _data_start = .;
        *(.data .data.*)
        _data_end = .;
    }

    .bss ALIGN(4096) : AT(ADDR(.bss) - KERNEL_VIRT_BASE) {
        _bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        _bss_end = .;
    }

    _kernel_end = . - KERNEL_VIRT_BASE;

    /DISCARD/ : {
        *(.eh_frame)
        *(.note.*)
        *(.comment)
    }
}

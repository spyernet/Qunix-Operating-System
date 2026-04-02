OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(kernel_main)

/* Kernel lives in the top 2 GB so R_X86_64_32S relocations fit */
KERNEL_PHYS_BASE = 0x02000000;
KERNEL_VIRT_BASE = 0xFFFFFFFF80000000;

SECTIONS {
    . = KERNEL_PHYS_BASE;
    _kernel_start = .;

    /* Relocate to higher half */
    . += KERNEL_VIRT_BASE;

    .text ALIGN(4096) : AT(ADDR(.text) - KERNEL_VIRT_BASE) {
        _text_start = .;
        KEEP(*(.text .text.*))
        _text_end = .;
    }

    .rodata ALIGN(4096) : AT(ADDR(.rodata) - KERNEL_VIRT_BASE) {
        _rodata_start = .;
        KEEP(*(.rodata .rodata.*))
        _rodata_end = .;
    }

    .data ALIGN(4096) : AT(ADDR(.data) - KERNEL_VIRT_BASE) {
        _data_start = .;
        KEEP(*(.data .data.*))
        _data_end = .;
    }

    .bss ALIGN(4096) : AT(ADDR(.bss) - KERNEL_VIRT_BASE) {
        _bss_start = .;
        KEEP(*(.bss .bss.*))
        *(COMMON)
        _bss_end = .;
    }

    _kernel_end = . - KERNEL_VIRT_BASE;

    /DISCARD/ : {
        *(.eh_frame)
        *(.note.*)
        *(.comment)
        *(.debug*)
    }
}

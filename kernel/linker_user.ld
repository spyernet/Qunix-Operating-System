OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(kernel_main)

SECTIONS {
    . = 0x100000;
    PROVIDE(_kernel_start = .);

    .note.Xen.PVH : { KEEP(*(.note.Xen.PVH)) }

    . = 0x100400;
    .pvh_entry : { KEEP(*(.pvh_entry)) }

    . = 0x100600;
    .pvh_stub64 : { KEEP(*(.pvh_stub64)) }

    . = ALIGN(0x200000);
    .text ALIGN(4096) : { KEEP(*(.text .text.*)) }
    .rodata ALIGN(4096) : { KEEP(*(.rodata .rodata.*)) }
    .data ALIGN(4096) : {
        KEEP(*(.data .data.*))
        *(.got) *(.got.plt)
    }
    .bss ALIGN(4096) : { KEEP(*(.bss .bss.*)) *(COMMON) }
    PROVIDE(_kernel_end = .);

    /DISCARD/ : {
        *(.eh_frame) *(.note.*) *(.comment) *(.debug*)
    }
}

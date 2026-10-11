ENTRY(_start)
PHDRS {
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    note PT_NOTE FLAGS(4);
    data PT_LOAD FLAGS(6);
}
SECTIONS {
    . = 0x4000000000;
    .text : ALIGN(4096) { KEEP(*(.text.entry)) *(.text .text.* .ltext .ltext.*) } :text
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.* .lrodata .lrodata.*) } :rodata
    .note.next : ALIGN(4) { KEEP(*(.note.next)) } :rodata :note
    . = ALIGN(4096);
    .data : { *(.data .data.* .ldata .ldata.*) *(.got .got.*) } :data
    .bss (NOLOAD) : { *(.bss .bss.* .lbss .lbss.*) *(COMMON) } :data
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note.GNU-stack) }
}
ASSERT(. <= 0x4000040000, "Native executable exceeds development image window")

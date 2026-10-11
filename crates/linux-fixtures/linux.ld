ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); rodata PT_LOAD FLAGS(4); data PT_LOAD FLAGS(6); }
SECTIONS {
 . = 0x4000000000;
 .text : { *(.text*) } :text
 . = ALIGN(4096);
 .rodata : { *(.rodata*) } :rodata
 . = ALIGN(4096);
 .data : { *(.data*) } :data
 .bss (NOLOAD) : { *(.bss*) } :data
 /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}

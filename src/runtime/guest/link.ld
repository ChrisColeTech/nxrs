/* One Horizon module (NSO): text, rodata, data+bss as three page-aligned PT_LOADs from address 0, MOD0 in rodata. */
PHDRS {
  text PT_LOAD FLAGS(5);
  rodata PT_LOAD FLAGS(4);
  data PT_LOAD FLAGS(6);
  tls PT_TLS;
  dyn PT_DYNAMIC;
  eh PT_GNU_EH_FRAME;
}
SECTIONS {
  . = 0;
  .text : { KEEP(*(.text.crt0)) *(.text .text.*) } :text
  .plt : { *(.plt .iplt) } :text
  . = ALIGN(0x1000);
  .rodata : { KEEP(*(.rodata.mod0)) *(.rodata .rodata.*) } :rodata
  .eh_frame_hdr : { __eh_frame_hdr_start = .; *(.eh_frame_hdr) __eh_frame_hdr_end = .; } :rodata :eh
  .eh_frame : { KEEP(*(.eh_frame)) } :rodata
  .gcc_except_table : { *(.gcc_except_table .gcc_except_table.*) } :rodata
  .dynsym : { *(.dynsym) } :rodata
  .dynstr : { *(.dynstr) } :rodata
  .hash : { *(.hash) } :rodata
  .gnu.hash : { *(.gnu.hash) } :rodata
  .rela.dyn : { *(.rela.dyn) } :rodata
  .rela.plt : { *(.rela.plt) } :rodata
  . = ALIGN(0x1000);
  .data : { *(.data .data.*) } :data
  .tdata : { __tls_start = .; *(.tdata .tdata.*) __tdata_end = .; } :data :tls
  .tbss : { *(.tbss .tbss.*) . = ALIGN(16); __tls_end = .; } :data :tls
  .init_array : { __init_array_start = .; KEEP(*(.init_array .init_array.*)) __init_array_end = .; } :data
  .dynamic : { *(.dynamic) } :data :dyn
  .got : { *(.got) } :data
  .got.plt : { *(.got.plt) } :data
  .bss : { __bss_start__ = .; *(.bss .bss.*) *(COMMON) . = ALIGN(8); __bss_end__ = .; } :data
  /DISCARD/ : { *(.comment) *(.note.*) }
}

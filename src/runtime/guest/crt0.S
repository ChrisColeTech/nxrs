// Module start: the first word is the entry (main) or a dead branch (libraries), the second the offset of MOD0.
    .hidden __module_start, __nx_mod0, __bss_start__, __bss_end__, __eh_frame_hdr_start, __eh_frame_hdr_end, _DYNAMIC
    .section .text.crt0, "ax"
    .global __module_start
    .type __module_start, %function
__module_start:
    .cfi_startproc          // an unwind entry, so the translator's discovery sees the entry as a function
#ifdef MAIN
    b __crt_start
#else
    b .
#endif
    .cfi_endproc
    .word __nx_mod0 - __module_start

    .section .rodata.mod0, "a"
    .align 2
    .global __nx_mod0
__nx_mod0:
    .ascii "MOD0"
    .word _DYNAMIC - __nx_mod0
    .word __bss_start__ - __nx_mod0
    .word __bss_end__ - __nx_mod0
    .word __eh_frame_hdr_start - __nx_mod0
    .word __eh_frame_hdr_end - __nx_mod0
    .word 0

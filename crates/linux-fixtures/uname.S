/* Original Linux x86-64 static fixture for the six-field utsname ABI. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    cmp qword ptr [rsp], 1
    jne failed
    cmp qword ptr [rsp + 16], 0
    jne failed

    /* The compatibility ABI promises the entire fixed-width record. */
    cld
    lea rdi, [rip + result]
    mov eax, 0xaaaaaaaa
    mov ecx, 390
    rep stosb
    mov eax, 63 /* uname */
    lea rdi, [rip + result]
    syscall
    test rax, rax
    jne failed

    cld
    lea rsi, [rip + expected]
    lea rdi, [rip + result]
    mov ecx, 390
    repe cmpsb
    jne failed

    /* A nonmapped destination must fail without reporting success. */
    mov eax, 63
    mov rdi, 0x4000040000
    syscall
    cmp rax, -14 /* EFAULT */
    jne failed

    mov eax, 1 /* write */
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 52
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2
.section .rodata
expected:
field_sysname: .asciz "Linux"
.zero 65-(.-field_sysname)
field_nodename: .asciz "next"
.zero 65-(.-field_nodename)
field_release: .asciz "1.0-next-linux-abi"
.zero 65-(.-field_release)
field_version: .asciz "#1 Helix Linux compatibility personality"
.zero 65-(.-field_version)
field_machine: .asciz "x86_64"
.zero 65-(.-field_machine)
field_domainname: .zero 65
message:
    .ascii "LINUX UNAME ELF: six fixed fields, zero padding, EFAULT PASS\n"
message_end:
.section .bss
.align 8
result: .skip 390
.section .note.GNU-stack,"",@progbits

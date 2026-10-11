/* Original Win64 PE fixture. No Microsoft headers, object code, or DLLs. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    /* Entry RSP is 8 mod 16. Reserve 32-byte home space and argument five. */
    sub rsp, 40
    mov rbx, 0x1111222233334444
    mov rbp, 0x2222333344445555
    mov rsi, 0x3333444455556666
    mov rdi, 0x4444555566667777
    mov r12, 0x5555666677778888
    mov r13, 0x6666777788889999
    mov r14, 0x777788889999aaaa
    mov r15, 0x123456789abcdef0
    movq xmm15, r15
    .ifdef NEXT_CROSS
    mov ecx, -99
    call qword ptr [rip + get_std_handle_iat]
    cmp rax, -1
    jne failed
    mov rcx, -1
    lea rdx, [rip + message]
    mov r8d, message_end-message
    lea r9, [rip + bytes_written]
    mov qword ptr [rsp + 32], 0
    call qword ptr [rip + write_file_iat]
    test eax, eax
    jne failed
    cmp dword ptr [rip + bytes_written], 0
    jne failed
    mov ecx, -11
    call qword ptr [rip + get_std_handle_iat]
    mov rcx, rax
    mov rdx, 0x4000040000
    mov r8d, 1
    lea r9, [rip + bytes_written]
    mov qword ptr [rsp + 32], 0
    call qword ptr [rip + write_file_iat]
    test eax, eax
    jne failed
    cmp dword ptr [rip + bytes_written], 0
    jne failed
    mov eax, 15
    int 0x80
    cmp rax, -6
    jne failed
    mov eax, 39
    syscall
    cmp rax, -6
    jne failed
    .endif
    mov ecx, -11
    call qword ptr [rip + get_std_handle_iat]
    test rax, rax
    je failed
    cmp rax, -1
    je failed
    mov rcx, rax
    lea rdx, [rip + message]
    mov r8d, message_end-message
    lea r9, [rip + bytes_written]
    mov qword ptr [rsp + 32], 0
    call qword ptr [rip + write_file_iat]
    test eax, eax
    je failed
    cmp dword ptr [rip + bytes_written], message_end-message
    jne failed
    mov rax, 0x1111222233334444
    cmp rbx, rax
    jne failed
    mov rax, 0x2222333344445555
    cmp rbp, rax
    jne failed
    mov rax, 0x3333444455556666
    cmp rsi, rax
    jne failed
    mov rax, 0x4444555566667777
    cmp rdi, rax
    jne failed
    mov rax, 0x5555666677778888
    cmp r12, rax
    jne failed
    mov rax, 0x6666777788889999
    cmp r13, rax
    jne failed
    mov rax, 0x777788889999aaaa
    cmp r14, rax
    jne failed
    mov rax, 0x123456789abcdef0
    cmp r15, rax
    jne failed
    movq rax, xmm15
    cmp r15, rax
    jne failed
    mov ecx, 45
    call qword ptr [rip + exit_process_iat]
    ud2
failed:
    mov ecx, 99
    call qword ptr [rip + exit_process_iat]
    ud2
.section .rdata,"a",@progbits
message: .ascii "NT PE32+ ELF-FREE: original Win64 imports/GPR/XMM compatibility PASS\n"
message_end:
.section .data
bytes_written: .long 0xffffffff
.section .note.GNU-stack,"",@progbits

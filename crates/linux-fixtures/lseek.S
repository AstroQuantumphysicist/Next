/* Original Linux x86-64 static fixture for signed lseek and file offsets. */
.intel_syntax noprefix
.global _start
.equ PAYLOAD_LENGTH, payload_end-payload
.section .text
_start:
    lea r15, [rip + default_path]
    cmp qword ptr [rsp], 2
    jb path_ready
    mov r15, qword ptr [rsp + 16]
path_ready:
    mov eax, 257 /* openat(AT_FDCWD, path, O_RDWR|O_CREAT|O_TRUNC|O_CLOEXEC, 0600) */
    mov rdi, -100
    mov rsi, r15
    mov edx, 0x80242
    mov r10d, 0600
    syscall
    test rax, rax
    js failed
    mov r12, rax

    mov eax, 1 /* write payload; cursor becomes 6 */
    mov rdi, r12
    lea rsi, [rip + payload]
    mov edx, PAYLOAD_LENGTH
    syscall
    cmp rax, PAYLOAD_LENGTH
    jne failed

    mov eax, 8 /* SEEK_SET to byte 2 */
    mov rdi, r12
    mov esi, 2
    xor edx, edx
    syscall
    cmp rax, 2
    jne failed
    mov eax, 0 /* read "cd"; file offset advances to 4 */
    mov rdi, r12
    lea rsi, [rip + buffer]
    mov edx, 2
    syscall
    cmp rax, 2
    jne failed
    cmp word ptr [rip + buffer], 0x6463
    jne failed

    mov eax, 8 /* SEEK_END - 1, then read final byte */
    mov rdi, r12
    mov rsi, -1
    mov edx, 2
    syscall
    cmp rax, 5
    jne failed
    mov eax, 0
    mov rdi, r12
    lea rsi, [rip + buffer]
    mov edx, 1
    syscall
    cmp rax, 1
    jne failed
    cmp byte ptr [rip + buffer], 'f'
    jne failed

    /* Failed seeks must preserve this file offset at 6. */
    mov eax, 8
    mov rdi, r12
    mov rsi, -7
    mov edx, 1 /* SEEK_CUR: resulting position would be -1 */
    syscall
    cmp rax, -22 /* EINVAL */
    jne failed
    mov eax, 8
    mov rdi, r12
    xor esi, esi
    mov edx, 1
    syscall
    cmp rax, 6
    jne failed

    mov eax, 8
    mov rdi, r12
    xor esi, esi
    mov edx, 99 /* invalid whence */
    syscall
    cmp rax, -22
    jne failed
    mov eax, 8
    mov rdi, r12
    xor esi, esi
    mov edx, 1
    syscall
    cmp rax, 6
    jne failed

    mov eax, 8
    mov edi, 99
    xor esi, esi
    xor edx, edx
    syscall
    cmp rax, -9 /* EBADF */
    jne failed

    mov eax, 8
    mov rdi, r12
    xor esi, esi
    mov edx, 1
    syscall
    cmp rax, 6
    jne failed
    mov eax, 3 /* close */
    mov rdi, r12
    syscall
    test rax, rax
    jne failed

    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 53
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2
.section .rodata
payload: .ascii "abcdef"
payload_end:
default_path: .asciz "/native/linux-lseek"
message:
    .ascii "LINUX LSEEK ELF: position, signed errors, and cursor preservation PASS\n"
message_end:
.section .bss
.align 8
buffer: .skip 16
.section .note.GNU-stack,"",@progbits

/* Original Linux x86-64 fstat fixture for the bounded 144-byte stat ABI. */
.intel_syntax noprefix
.global _start
.equ STAT_BYTES, 144
.equ PAYLOAD_LENGTH, payload_end-payload
.section .text
_start:
    /* The host runner supplies /dev/null as stdin; Helix models the same EOF device. */
    mov eax, 5 /* fstat */
    xor edi, edi
    lea rsi, [rip + stat_buffer]
    syscall
    test rax, rax
    jne failed
    mov eax, dword ptr [rip + stat_buffer + 24]
    and eax, 0xf000
    cmp eax, 0x2000 /* S_IFCHR */
    jne failed
    cmp qword ptr [rip + stat_buffer + 40], 259 /* /dev/null st_rdev */
    jne failed
    cmp qword ptr [rip + stat_buffer + 48], 0
    jne failed

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

    mov eax, 1 /* write known file contents */
    mov rdi, r12
    lea rsi, [rip + payload]
    mov edx, PAYLOAD_LENGTH
    syscall
    cmp rax, PAYLOAD_LENGTH
    jne failed

    mov eax, 5
    mov rdi, r12
    lea rsi, [rip + stat_buffer]
    syscall
    test rax, rax
    jne failed
    mov eax, dword ptr [rip + stat_buffer + 24]
    and eax, 0xf000
    cmp eax, 0x8000 /* S_IFREG */
    jne failed
    mov eax, dword ptr [rip + stat_buffer + 24]
    and eax, 0x0fff
    cmp eax, 0600
    jne failed
    cmp qword ptr [rip + stat_buffer + 16], 1 /* st_nlink */
    jne failed
    cmp qword ptr [rip + stat_buffer + 48], PAYLOAD_LENGTH /* st_size */
    jne failed
    mov r13, qword ptr [rip + stat_buffer] /* st_dev */
    mov r14, qword ptr [rip + stat_buffer + 8] /* st_ino */
    test r14, r14
    jz failed

    /* A duplicated open description has the same stat identity. */
    mov eax, 32 /* dup */
    mov rdi, r12
    syscall
    test rax, rax
    js failed
    mov rbx, rax
    mov eax, 5
    mov rdi, rbx
    lea rsi, [rip + duplicate_stat]
    syscall
    test rax, rax
    jne failed
    cmp r13, qword ptr [rip + duplicate_stat]
    jne failed
    cmp r14, qword ptr [rip + duplicate_stat + 8]
    jne failed

    /* Linux's unsigned-int fd ABI ignores the upper register bits. */
    mov eax, 5
    mov rdi, 0x100000000
    or rdi, r12
    lea rsi, [rip + duplicate_stat]
    syscall
    test rax, rax
    jne failed
    cmp r14, qword ptr [rip + duplicate_stat + 8]
    jne failed

    /* EBADF takes precedence over EFAULT when both fd and destination fail. */
    cld
    lea rdi, [rip + stat_buffer]
    mov eax, 0xa5a5a5a5
    mov ecx, STAT_BYTES / 4
    rep stosd
    mov eax, 5
    mov edi, 99
    lea rsi, [rip + stat_buffer]
    syscall
    cmp rax, -9 /* EBADF */
    jne failed
    cld
    lea rdi, [rip + stat_buffer]
    mov eax, 0xa5a5a5a5
    mov ecx, STAT_BYTES / 4
    repe scasd
    jne failed

    mov eax, 5
    mov edi, 99
    mov rsi, 0x4000040000
    syscall
    cmp rax, -9 /* EBADF before bad-buffer EFAULT */
    jne failed

    mov eax, 5
    mov rdi, r12
    mov rsi, 0x4000040000
    syscall
    cmp rax, -14 /* EFAULT for valid fd and invalid destination */
    jne failed

    /* Portable cross-page EFAULT: only the first page remains mapped. */
    mov eax, 9 /* mmap(NULL,8192,READ|WRITE,PRIVATE|ANON,-1,0) */
    xor edi, edi
    mov esi, 8192
    mov edx, 3
    mov r10d, 0x22
    mov r8, -1
    xor r9d, r9d
    syscall
    cmp rax, -4095
    jae failed
    mov r13, rax
    lea rdi, [r13 + 4096]
    mov esi, 4096
    mov eax, 11 /* munmap second page */
    syscall
    test rax, rax
    jne failed
    cld
    lea rdi, [r13 + 4032]
    mov rax, 0xa5a5a5a5a5a5a5a5
    mov ecx, 8
    rep stosq
    mov eax, 5
    mov rdi, r12
    lea rsi, [r13 + 4032]
    syscall
    cmp rax, -14
    jne failed

    /* Linux may copy a prefix on EFAULT. Helix additionally promises complete
       destination validation before copyout. The same binary selects that
       stronger assertion only for the exact compatibility-personality release. */
    mov eax, 63
    lea rdi, [rip + uname_buffer]
    syscall
    test rax, rax
    jne failed
    cld
    lea rsi, [rip + uname_buffer + 130]
    lea rdi, [rip + helix_release]
    mov ecx, helix_release_end-helix_release
    repe cmpsb
    jne prefix_check_done
    lea rdi, [r13 + 4032]
    mov rax, 0xa5a5a5a5a5a5a5a5
    mov ecx, 8
    repe scasq
    jne failed
prefix_check_done:
    mov eax, 11
    mov rdi, r13
    mov esi, 4096
    syscall
    test rax, rax
    jne failed

    mov eax, 3 /* close duplicate and original descriptors */
    mov rdi, rbx
    syscall
    test rax, rax
    jne failed
    mov eax, 3
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
    mov edi, 54
    syscall
    ud2
failed:
    mov eax, 231
    mov edi, 99
    syscall
    ud2

.section .rodata
helix_release: .asciz "1.0-next-linux-abi"
helix_release_end:
payload: .ascii "fstat-linux"
payload_end:
default_path: .asciz "/native/linux-fstat"
message:
    .ascii "LINUX FSTAT ELF: identity, type, size, EBADF and EFAULT PASS\n"
message_end:
.section .bss
.align 8
stat_buffer: .skip STAT_BYTES
duplicate_stat: .skip STAT_BYTES
uname_buffer: .skip 390
.section .note.GNU-stack,"",@progbits

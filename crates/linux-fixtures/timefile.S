/* Original Linux x86-64 fixture for gettimeofday and file truncation. */
.intel_syntax noprefix
.global _start
.equ PREFIX_LENGTH, prefix_end-prefix
.equ PRESERVED_LENGTH, 4
.equ ZERO_RANGE_LENGTH, 8
.section .text
_start:
    /* Select Helix-only profile checks by exact release, not host assumptions. */
    mov eax, 63 /* uname */
    lea rdi, [rip + uname_buffer]
    syscall
    test rax, rax
    jne failed
    xor r15d, r15d
    cld
    lea rsi, [rip + uname_buffer + 130] /* utsname.release */
    lea rdi, [rip + helix_release]
    mov ecx, helix_release_end-helix_release
    repe cmpsb
    jne release_ready
    mov r15d, 1
release_ready:

    /* gettimeofday returns signed seconds/useconds and zero timezone fields. */
    mov rax, 0xa5a5a5a5a5a5a5a5
    mov qword ptr [rip + timezone_buffer], rax
    mov eax, 96 /* gettimeofday(tv, tz) */
    lea rdi, [rip + timeval_buffer]
    lea rsi, [rip + timezone_buffer]
    syscall
    test rax, rax
    jne failed
    mov rax, qword ptr [rip + timeval_buffer]
    test rax, rax
    jle failed
    mov rax, qword ptr [rip + timeval_buffer + 8]
    test rax, rax
    js failed
    cmp rax, 1000000
    jae failed
    cmp qword ptr [rip + timezone_buffer], 0
    jne failed

    /* A null timeval requests timezone only; both null pointers are also valid. */
    mov rax, 0xa5a5a5a5a5a5a5a5
    mov qword ptr [rip + timezone_buffer], rax
    mov eax, 96
    xor edi, edi
    lea rsi, [rip + timezone_buffer]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + timezone_buffer], 0
    jne failed
    mov eax, 96
    xor edi, edi
    xor esi, esi
    syscall
    test rax, rax
    jne failed

    /* A bad timeval stops before timezone: preserve its sentinel output. */
    mov rax, 0xa5a5a5a5a5a5a5a5
    mov qword ptr [rip + timezone_buffer], rax
    mov eax, 96
    mov rdi, 0x4000040000
    lea rsi, [rip + timezone_buffer]
    syscall
    cmp rax, -14
    jne failed
    mov rax, 0xa5a5a5a5a5a5a5a5
    cmp qword ptr [rip + timezone_buffer], rax
    jne failed
    mov eax, 96
    xor edi, edi
    mov rsi, 0x4000040000
    syscall
    cmp rax, -14
    jne failed

    /* Raw Linux writes valid timeval before a later bad timezone returns EFAULT. */
    mov rax, 0x5a5a5a5a5a5a5a5a
    mov qword ptr [rip + timeval_buffer], rax
    mov qword ptr [rip + timeval_buffer + 8], rax
    mov eax, 96
    lea rdi, [rip + timeval_buffer]
    mov rsi, 0x4000040000
    syscall
    cmp rax, -14
    jne failed
    mov rax, qword ptr [rip + timeval_buffer]
    test rax, rax
    jle failed
    mov rax, 0x5a5a5a5a5a5a5a5a
    cmp qword ptr [rip + timeval_buffer], rax
    je failed
    mov rax, qword ptr [rip + timeval_buffer + 8]
    test rax, rax
    js failed
    cmp rax, 1000000
    jae failed

    /* The runner supplies the path as argv[1]; the default is guest-friendly. */
    lea rbx, [rip + default_path]
    cmp qword ptr [rsp], 2
    jb path_ready
    mov rbx, qword ptr [rsp + 16]
path_ready:
    mov qword ptr [rip + path_pointer], rbx

    mov eax, 257 /* openat(AT_FDCWD,path,O_RDWR|O_CREAT|O_TRUNC|O_CLOEXEC,0600) */
    mov rdi, -100
    mov rsi, rbx
    mov edx, 0x80242
    mov r10d, 0600
    syscall
    test rax, rax
    js failed
    mov r12, rax /* original open description */

    mov eax, 1 /* write known contents; original offset becomes eight */
    mov rdi, r12
    lea rsi, [rip + prefix]
    mov edx, PREFIX_LENGTH
    syscall
    cmp rax, PREFIX_LENGTH
    jne failed

    mov eax, 32 /* dup shares the original open description and offset */
    mov rdi, r12
    syscall
    test rax, rax
    js failed
    mov r13, rax

    /* A second open creates an independent open description at offset zero. */
    mov eax, 257
    mov rdi, -100
    mov rsi, qword ptr [rip + path_pointer]
    mov edx, 0x80002 /* O_RDWR|O_CLOEXEC */
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov r14, rax

    mov eax, 77 /* ftruncate(original, 4) */
    mov rdi, r12
    mov esi, 4
    syscall
    test rax, rax
    jne failed
    mov ebx, 4
    mov rdi, r12
    call check_size
    mov ebx, PREFIX_LENGTH
    mov rdi, r13
    call check_position /* truncating does not move either shared cursor */

    mov eax, 77 /* grow to twelve; the new tail must be zero-filled */
    mov rdi, r12
    mov esi, 12
    syscall
    test rax, rax
    jne failed
    mov ebx, 12
    mov rdi, r12
    call check_size
    mov ebx, PREFIX_LENGTH
    mov rdi, r12
    call check_position

    mov eax, 8 /* seek original to start and verify preserved prefix */
    mov rdi, r12
    xor esi, esi
    xor edx, edx /* SEEK_SET */
    syscall
    test rax, rax
    jne failed
    mov eax, 0
    mov rdi, r12
    lea rsi, [rip + io_buffer]
    mov edx, PRESERVED_LENGTH
    syscall
    cmp rax, PRESERVED_LENGTH
    jne failed
    cld
    lea rsi, [rip + prefix]
    lea rdi, [rip + io_buffer]
    mov ecx, PRESERVED_LENGTH
    repe cmpsb
    jne failed
    mov ebx, PRESERVED_LENGTH
    mov rdi, r13
    call check_position /* dup sees reads through the original descriptor */

    mov eax, 0 /* the truncated/regrown range [4,12) reads as zeros */
    mov rdi, r12
    lea rsi, [rip + io_buffer]
    mov edx, ZERO_RANGE_LENGTH
    syscall
    cmp rax, ZERO_RANGE_LENGTH
    jne failed
    cmp qword ptr [rip + io_buffer], 0
    jne failed
    mov ebx, 12
    mov rdi, r13
    call check_position

    mov ebx, 0
    mov rdi, r14
    call check_position /* independent open still starts at zero */
    mov eax, 8
    mov rdi, r14
    mov esi, 2
    xor edx, edx
    syscall
    cmp rax, 2
    jne failed
    mov ebx, 2
    mov rdi, r14
    call check_position
    mov ebx, 12
    mov rdi, r12
    call check_position /* seeking the independent fd leaves the original alone */
    mov rdi, r13
    call check_position

    mov eax, 5 /* fstat through the independent fd sees the shared file size */
    mov rdi, r14
    lea rsi, [rip + stat_buffer]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + stat_buffer + 48], 12
    jne failed

    mov eax, 3 /* close independent fd, then verify a closed fd reports EBADF */
    mov rdi, r14
    syscall
    test rax, rax
    jne failed
    mov eax, 77
    mov rdi, r14
    xor esi, esi
    syscall
    cmp rax, -9 /* EBADF */
    jne failed

    mov eax, 77
    mov edi, 99
    xor esi, esi
    syscall
    cmp rax, -9 /* invalid fd */
    jne failed
    mov eax, 77
    mov edi, 99
    mov rsi, -1
    syscall
    cmp rax, -22 /* negative length is EINVAL before fd lookup */
    jne failed
    mov eax, 77
    xor edi, edi /* stdin is not a truncatable regular file */
    xor esi, esi
    syscall
    cmp rax, -22 /* EINVAL */
    jne failed

    mov eax, 257 /* a regular file opened read-only rejects ftruncate */
    mov rdi, -100
    mov rsi, qword ptr [rip + path_pointer]
    mov edx, 0x80000
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov rbp, rax
    mov eax, 77
    mov rdi, rbp
    xor esi, esi
    syscall
    cmp rax, -22 /* EINVAL on Linux for a non-writable regular file */
    jne failed
    mov eax, 3
    mov rdi, rbp
    syscall
    test rax, rax
    jne failed

    mov eax, 257 /* directories are not truncatable either */
    mov rdi, -100
    lea rsi, [rip + current_directory]
    mov edx, 0x90000 /* O_RDONLY|O_DIRECTORY|O_CLOEXEC */
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov rbp, rax
    mov eax, 77
    mov rdi, rbp
    xor esi, esi
    syscall
    cmp rax, -22 /* EINVAL */
    jne failed
    mov eax, 3
    mov rdi, rbp
    syscall
    test rax, rax
    jne failed

    /* The profile accepts 4096 bytes; only Helix rejects 4097 with EFBIG. */
    mov eax, 77
    mov rdi, r12
    mov esi, 4096
    syscall
    test rax, rax
    jne failed
    mov ebx, 4096
    mov rdi, r12
    call check_size
    mov eax, 77
    mov rdi, r12
    mov esi, 4097
    syscall
    test r15, r15
    jz host_large_size
    cmp rax, -27 /* EFBIG on the exact Helix release */
    jne failed
    mov ebx, 4096
    mov rdi, r12
    call check_size
    jmp size_limit_done
host_large_size:
    test rax, rax
    jne failed
    mov ebx, 4097
    mov rdi, r12
    call check_size
size_limit_done:
    mov eax, 77 /* restore the bounded file size before unlink */
    mov rdi, r12
    mov esi, 12
    syscall
    test rax, rax
    jne failed
    mov ebx, 12
    mov rdi, r12
    call check_size

    mov eax, 87 /* unlink path while descriptors still hold the file open */
    mov rdi, qword ptr [rip + path_pointer]
    syscall
    test rax, rax
    jne failed
    mov eax, 257
    mov rdi, -100
    mov rsi, qword ptr [rip + path_pointer]
    mov edx, 0x80000
    xor r10d, r10d
    syscall
    cmp rax, -2 /* pathname is gone */
    jne failed

    mov eax, 77 /* the still-open file remains usable after unlink */
    mov rdi, r13
    mov esi, 3
    syscall
    test rax, rax
    jne failed
    mov ebx, 3
    mov rdi, r12
    call check_size
    mov eax, 8
    mov rdi, r12
    xor esi, esi
    xor edx, edx
    syscall
    test rax, rax
    jne failed
    mov eax, 0
    mov rdi, r12
    lea rsi, [rip + io_buffer]
    mov edx, 3
    syscall
    cmp rax, 3
    jne failed
    cmp dword ptr [rip + io_buffer], 0x00434241 /* "ABC\0" */
    jne failed

    mov eax, 3
    mov rdi, r12
    syscall
    test rax, rax
    jne failed
    mov eax, 77
    mov rdi, r12
    xor esi, esi
    syscall
    cmp rax, -9 /* closed original fd */
    jne failed
    mov eax, 3
    mov rdi, r13
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
    mov edi, 57
    syscall
    ud2

/* Helpers expect fd in rdi and expected size/position in ebx. */
check_size:
    mov eax, 5 /* fstat */
    lea rsi, [rip + stat_buffer]
    syscall
    test rax, rax
    jne failed
    cmp qword ptr [rip + stat_buffer + 48], rbx /* x86-64 st_size */
    jne failed
    ret

check_position:
    mov eax, 8 /* lseek(fd,0,SEEK_CUR) */
    xor esi, esi
    mov edx, 1
    syscall
    cmp rax, rbx
    jne failed
    ret

failed:
    mov eax, 231 /* exit_group */
    mov edi, 99
    syscall
    ud2

.section .rodata
helix_release: .asciz "1.0-next-linux-abi"
helix_release_end:
prefix: .ascii "ABCDEFGH"
prefix_end:
default_path: .asciz "/native/timefile"
current_directory: .asciz "."
message: .ascii "LINUX TIMEFILE ELF: gettimeofday and ftruncate descriptor semantics PASS\n"
message_end:
.section .bss
.align 8
timeval_buffer: .skip 16
timezone_buffer: .skip 8
uname_buffer: .skip 390
stat_buffer: .skip 144
io_buffer: .skip 16
path_pointer: .skip 8
.section .note.GNU-stack,"",@progbits

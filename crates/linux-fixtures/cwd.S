/* Original Linux x86-64 fixture for getcwd, chdir and relative openat paths. */
.intel_syntax noprefix
.global _start
.equ SAMPLE_LENGTH, sample_end-sample
.section .text
_start:
    /* Capture the initial absolute cwd, including Linux's NUL-inclusive size. */
    mov eax, 79 /* getcwd */
    lea rdi, [rip + initial_cwd]
    mov esi, 128
    syscall
    test rax, rax
    jle failed
    mov r12, rax
    cmp byte ptr [rip + initial_cwd], '/'
    jne failed
    mov rax, r12
    dec rax
    lea rdx, [rip + initial_cwd]
    cmp byte ptr [rdx + rax], 0
    jne failed

    /* A buffer shorter than even "/\0" must fail with ERANGE. */
    mov eax, 79
    lea rdi, [rip + current_cwd]
    mov esi, 1
    syscall
    cmp rax, -34 /* ERANGE */
    jne failed

    /* A sufficiently large request with an inaccessible destination is EFAULT. */
    mov eax, 79
    mov rdi, 0x4000040000
    mov esi, 128
    syscall
    cmp rax, -14 /* EFAULT */
    jne failed

    /* Place cwd exactly at a mapped page edge, then test a one-byte crossing. */
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
    mov rbx, rax
    lea rdi, [rbx + 4096]
    mov esi, 4096
    mov eax, 11 /* munmap the second page */
    syscall
    test rax, rax
    jne failed
    lea rdi, [rbx + 4096]
    sub rdi, r12
    mov eax, 79
    mov esi, r12d
    syscall
    cmp rax, r12 /* exact fit through the last mapped byte succeeds */
    jne failed
    lea rsi, [rip + initial_cwd]
    lea rdi, [rbx + 4096]
    sub rdi, r12
    mov rcx, r12
    cld
    repe cmpsb
    jne failed
    lea rdi, [rbx + 4096]
    sub rdi, r12
    inc rdi /* one byte of the requested copy now crosses into an unmapped page */
    mov eax, 79
    mov esi, r12d
    syscall
    cmp rax, -14 /* EFAULT */
    jne failed
    mov eax, 11 /* release the remaining mapped page */
    mov rdi, rbx
    mov esi, 4096
    syscall
    test rax, rax
    jne failed

    /* Form an absolute sample path from the current cwd for portable host runs. */
    cld
    lea rsi, [rip + initial_cwd]
    lea rdi, [rip + absolute_sample]
    mov rcx, r12
    dec rcx
    rep movsb
    lea rsi, [rip + absolute_sample_suffix]
    mov ecx, absolute_sample_suffix_end-absolute_sample_suffix
    rep movsb

    /* AT_FDCWD-relative open of the fixture's root-level sample file. */
    mov eax, 257 /* openat */
    mov edi, 0xffffff9c /* low32 is AT_FDCWD; upper32 is deliberately zero */
    lea rsi, [rip + sample_name]
    mov edx, 0x80000 /* O_RDONLY|O_CLOEXEC */
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov r14, rax
    call verify_sample
    mov eax, 3 /* close */
    mov rdi, r14
    syscall
    test rax, rax
    jne failed

    /* A nonzero high32 still truncates to AT_FDCWD in the low32. */
    mov eax, 257
    mov rdi, 0x1ffffff9c
    lea rsi, [rip + sample_name]
    mov edx, 0x80000
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov r14, rax
    call verify_sample
    mov eax, 3
    mov rdi, r14
    syscall
    test rax, rax
    jne failed

    /* An absolute path ignores an invalid dirfd, as on Linux. */
    mov eax, 257
    mov edi, 99
    lea rsi, [rip + absolute_sample]
    mov edx, 0x80000
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov r14, rax
    call verify_sample
    mov eax, 3
    mov rdi, r14
    syscall
    test rax, rax
    jne failed

    /* Build the exact expected cwd after the relative chdir("alpha"). */
    cld
    lea rsi, [rip + initial_cwd]
    lea rdi, [rip + expected_nested]
    mov rcx, r12
    dec rcx
    rep movsb
    mov byte ptr [rdi], '/'
    inc rdi
    lea rsi, [rip + alpha_suffix]
    mov ecx, alpha_suffix_end-alpha_suffix
    rep movsb
    mov r13, r12
    add r13, alpha_suffix_end-alpha_suffix

    lea rdi, [rip + alpha_name]
    mov eax, 80 /* chdir */
    syscall
    test rax, rax
    jne failed
    call verify_nested_cwd

    /* Failed chdir must leave the process cwd unchanged. */
    lea rdi, [rip + missing_name]
    mov eax, 80
    syscall
    cmp rax, -2 /* ENOENT */
    jne failed
    call verify_nested_cwd

    /* Invalid chdir input is EFAULT and also leaves cwd unchanged. */
    mov eax, 80
    mov rdi, 0x4000040000
    syscall
    cmp rax, -14 /* EFAULT */
    jne failed
    call verify_nested_cwd

    /* AT_FDCWD is relative to the changed cwd; ../sample reaches the root file. */
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + parent_sample_name]
    mov edx, 0x80000
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov r14, rax
    call verify_sample
    mov eax, 3
    mov rdi, r14
    syscall
    test rax, rax
    jne failed

    /* Open the current alpha cwd as a directory, then resolve ../sample there. */
    mov eax, 257
    mov rdi, -100
    lea rsi, [rip + current_directory_name]
    mov edx, 0x90000 /* O_RDONLY|O_DIRECTORY|O_CLOEXEC */
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov r15, rax
    mov eax, 257
    mov rdi, 0x100000000
    or rdi, r15 /* valid fd in low32; nonzero high32 must be ignored */
    lea rsi, [rip + parent_sample_name]
    mov edx, 0x80000
    xor r10d, r10d
    syscall
    test rax, rax
    js failed
    mov r14, rax
    call verify_sample
    mov eax, 3
    mov rdi, r14
    syscall
    test rax, rax
    jne failed
    mov eax, 3
    mov rdi, r15
    syscall
    test rax, rax
    jne failed

    /* Return to the initial cwd and verify canonicalization of alpha/.. . */
    lea rdi, [rip + parent_name]
    mov eax, 80
    syscall
    test rax, rax
    jne failed
    call verify_initial_cwd
    lea rdi, [rip + alpha_parent_name]
    mov eax, 80
    syscall
    test rax, rax
    jne failed
    call verify_initial_cwd

    /* Linux host roots differ; assert Helix's confinement only for its release. */
    mov eax, 63 /* uname */
    lea rdi, [rip + uname_buffer]
    syscall
    test rax, rax
    jne failed
    cld
    lea rsi, [rip + uname_buffer + 130] /* utsname.release */
    lea rdi, [rip + helix_release]
    mov ecx, helix_release_end-helix_release
    repe cmpsb
    jne confinement_check_done
    lea rdi, [rip + above_root_name]
    mov eax, 80
    syscall
    cmp rax, -13 /* EACCES: canonical path cannot climb above /native */
    jne failed
    call verify_initial_cwd
confinement_check_done:

    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, message_end-message
    syscall
    cmp rax, message_end-message
    jne failed
    mov eax, 60
    mov edi, 56
    syscall
    ud2

/* Check exact cwd bytes and NUL-inclusive count after chdir("alpha"). */
verify_nested_cwd:
    mov eax, 79
    lea rdi, [rip + current_cwd]
    mov esi, 128
    syscall
    cmp rax, r13
    jne failed
    cld
    lea rsi, [rip + expected_nested]
    lea rdi, [rip + current_cwd]
    mov rcx, r13
    repe cmpsb
    jne failed
    ret

/* Check exact original cwd bytes and NUL-inclusive count. */
verify_initial_cwd:
    mov eax, 79
    lea rdi, [rip + current_cwd]
    mov esi, 128
    syscall
    cmp rax, r12
    jne failed
    cld
    lea rsi, [rip + initial_cwd]
    lea rdi, [rip + current_cwd]
    mov rcx, r12
    repe cmpsb
    jne failed
    ret

/* Read and compare the entire known sample payload from fd r14. */
verify_sample:
    mov eax, 0 /* read */
    mov rdi, r14
    lea rsi, [rip + sample_buffer]
    mov edx, SAMPLE_LENGTH
    syscall
    cmp rax, SAMPLE_LENGTH
    jne failed
    cld
    lea rsi, [rip + sample]
    lea rdi, [rip + sample_buffer]
    mov ecx, SAMPLE_LENGTH
    repe cmpsb
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
sample: .ascii "cwd-sample"
sample_end:
sample_name: .asciz "sample"
absolute_sample_suffix: .asciz "/sample"
absolute_sample_suffix_end:
alpha_name: .asciz "alpha"
current_directory_name: .asciz "."
alpha_suffix: .asciz "alpha"
alpha_suffix_end:
missing_name: .asciz "cwd-missing"
parent_name: .asciz ".."
parent_sample_name: .asciz "../sample"
alpha_parent_name: .asciz "alpha/.."
above_root_name: .asciz "/native/../"
message:
    .ascii "LINUX CWD ELF: getcwd, chdir, and relative openat PASS\n"
message_end:
.section .bss
.align 8
initial_cwd: .skip 128
current_cwd: .skip 128
expected_nested: .skip 128
absolute_sample: .skip 160
sample_buffer: .skip SAMPLE_LENGTH
uname_buffer: .skip 390
.section .note.GNU-stack,"",@progbits

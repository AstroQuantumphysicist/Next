/* Original static x86-64 Linux getdents64 fixture; no libc dependency. */
.intel_syntax noprefix
.global _start
.section .text
_start:
    lea r15,[rip+default_path]
    cmp qword ptr [rsp],2
    jb path_ready
    mov r15,[rsp+16]
path_ready:
    mov eax,2
    mov rdi,r15
    mov esi,0x90000 /* O_DIRECTORY|O_CLOEXEC, read-only */
    xor edx,edx
    syscall
    test rax,rax
    js failed
    mov r12,rax
    mov eax,5
    mov rdi,r12
    lea rsi,[rip+stat]
    syscall
    test rax,rax
    jne failed
    mov eax,[rip+stat+24]
    and eax,0xf000
    cmp eax,0x4000
    jne failed
    /* Too small for even one header/name record; offset must not advance. */
    mov eax,217
    mov rdi,r12
    lea rsi,[rip+buffer]
    mov rdx,0x100000001 /* native syscall count is unsigned int */
    syscall
    cmp rax,-22
    jne failed
    mov eax,217
    mov rdi,r12
    lea rsi,[rip+buffer]
    mov edx,0x80000000
    syscall
    cmp rax,-22
    jne failed
    mov eax,217
    mov rdi,r12
    lea rsi,[rip+buffer]
    mov edx,0xffffffff
    syscall
    cmp rax,-22
    jne failed
    mov eax,217
    mov edi,-1
    xor esi,esi
    mov edx,4096
    syscall
    cmp rax,-9
    jne failed
    mov eax,217
    xor edi,edi /* EOF stdin is a character stream, never a directory */
    lea rsi,[rip+buffer]
    mov edx,4096
    syscall
    cmp rax,-20
    jne failed
    mov eax,217
    mov rdi,r12
    xor esi,esi
    mov edx,4096
    syscall
    cmp rax,-14
    jne failed
    /* dup shares the cookie; separate open starts independently. */
    mov eax,32
    mov rdi,r12
    syscall
    test rax,rax
    js failed
    mov r13,rax
    xor ebp,ebp
read_loop:
    mov eax,217
    mov rdi,0x100000000
    or rdi,r12
    lea rsi,[rip+buffer]
    mov edx,4096
    syscall
    test rax,rax
    js failed
    jz eof
    lea rbx,[rip+buffer]
    lea r14,[rbx+rax]
record:
    lea rax,[rbx+24]
    cmp rax,r14
    ja failed
    cmp qword ptr [rbx],0
    je failed
    movzx edx,word ptr [rbx+16]
    cmp edx,24
    jb failed
    test edx,7
    jne failed
    lea rsi,[rbx+rdx]
    cmp rsi,r14
    ja failed
    lea rax,[rbx+19]
name_loop:
    cmp rax,rsi
    jae failed
    cmp byte ptr [rax],0
    je name_done
    inc rax
    jmp name_loop
name_done:
    cmp byte ptr [rbx+19],0
    je failed
    cmp byte ptr [rbx+19],'.'
    jne next_record
    cmp byte ptr [rbx+20],0
    jne check_parent
    or ebp,1
    jmp next_record
check_parent:
    cmp byte ptr [rbx+20],'.'
    jne next_record
    cmp byte ptr [rbx+21],0
    jne next_record
    or ebp,2
next_record:
    mov rbx,rsi
    cmp rbx,r14
    jb record
    jmp read_loop
eof:
    cmp ebp,3
    jne failed
    mov eax,217
    mov rdi,r13
    xor esi,esi /* EOF must not touch even a NULL output */
    mov edx,4096
    syscall
    test rax,rax
    jne failed
    mov eax,217
    mov rdi,r13
    xor esi,esi
    xor edx,edx /* EOF with zero count also does not access NULL */
    syscall
    test rax,rax
    jne failed
    mov eax,217
    mov rdi,r13
    xor esi,esi
    mov edx,0xffffffff /* no directory actor at EOF */
    syscall
    test rax,rax
    jne failed
    /* An independent description has not reached EOF. */
    mov eax,2
    mov rdi,r15
    mov esi,0x90000
    xor edx,edx
    syscall
    test rax,rax
    js failed
    mov rbx,rax
    mov eax,217
    mov rdi,rbx
    lea rsi,[rip+buffer]
    mov edx,4096
    syscall
    test rax,rax
    jle failed
    mov eax,3
    mov rdi,rbx
    syscall
    /* Rewind through dup changes the original's shared description. */
    mov eax,8
    mov rdi,r13
    xor esi,esi
    xor edx,edx
    syscall
    test rax,rax
    jne failed
    /* A page-straddling fault cannot consume the shared cookie in Helix. */
    mov eax,9
    xor edi,edi
    mov esi,8192
    mov edx,3
    mov r10d,0x22
    mov r8,-1
    xor r9d,r9d
    syscall
    test rax,rax
    js failed
    mov r14,rax
    mov eax,11
    lea rdi,[r14+4096]
    mov esi,4096
    syscall
    test rax,rax
    jne failed
    /* Capacity crosses the guard, but the actual short result fits. */
    mov eax,217
    mov rdi,r12
    lea rsi,[r14+3072]
    mov edx,4096
    syscall
    test rax,rax
    jle failed
    cmp rax,1024
    ja failed
    mov eax,8
    mov rdi,r12
    xor esi,esi
    xor edx,edx
    syscall
    test rax,rax
    jne failed
    mov eax,217
    mov rdi,r12
    lea rsi,[r14+4080]
    mov edx,64
    syscall
    cmp rax,-14
    jne failed
    mov eax,8
    mov rdi,r12
    xor esi,esi
    mov edx,1 /* SEEK_CUR: fault did not consume the first record */
    syscall
    test rax,rax
    jne failed
    mov eax,217
    mov rdi,r12
    lea rsi,[rip+buffer]
    mov edx,4096
    syscall
    test rax,rax
    jle failed
    mov eax,11
    mov rdi,r14
    mov esi,4096
    syscall
    mov eax,3
    mov rdi,r12
    syscall
    mov eax,217
    mov rdi,r12
    lea rsi,[rip+buffer]
    mov edx,4096
    syscall
    cmp rax,-9
    jne failed
    mov eax,3
    mov rdi,r13
    syscall
    mov eax,1
    mov edi,1
    lea rsi,[rip+message]
    mov edx,message_end-message
    syscall
    mov eax,60
    mov edi,55
    syscall
failed:
    mov eax,60
    mov edi,155
    syscall
.section .rodata
message: .ascii "LINUX GETDENTS ELF: records, cursor, dup, EOF and faults PASS\n"
message_end:
default_path: .asciz "/native"
.section .bss
.balign 16
stat: .skip 144
buffer: .skip 4096

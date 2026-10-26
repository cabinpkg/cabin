/* The C preprocessor runs first, so ANSWER_VALUE and these
   conditionals resolve before the assembler sees the file. */
#if defined(__APPLE__)
#define SYMBOL _cabin_answer
#else
#define SYMBOL cabin_answer
#endif

    .text
    .globl SYMBOL
#if defined(__x86_64__)
SYMBOL:
    movl $ANSWER_VALUE, %eax
    ret
#if defined(__ELF__)
    .section .note.GNU-stack,"",@progbits
#endif
#elif defined(__aarch64__)
    .p2align 2
SYMBOL:
    mov w0, #ANSWER_VALUE
    ret
#if defined(__ELF__)
    .section .note.GNU-stack,"",%progbits
#endif
#else
#error "answer.S supports x86_64 and aarch64"
#endif

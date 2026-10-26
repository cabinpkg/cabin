; `ml64 /D` defines CABIN_ANSWER as a text macro.
.code
cabin_answer PROC
    mov eax, CABIN_ANSWER
    ret
cabin_answer ENDP
END

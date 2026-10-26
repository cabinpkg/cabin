; `ml64 /D` defines ANSWER_VALUE as a text macro.  MASM names are
; case-insensitive, so the define must not spell `cabin_answer`.
.code
cabin_answer PROC
    mov eax, ANSWER_VALUE
    ret
cabin_answer ENDP
END

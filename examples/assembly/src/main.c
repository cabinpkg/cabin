#include <stdio.h>

/* Defined in src/answer.S or src/answer.asm. */
int cabin_answer(void);

int main(void) {
    int answer = cabin_answer();
    printf("Assembled answer: %d\n", answer);
    return answer == 42 ? 0 : 1;
}

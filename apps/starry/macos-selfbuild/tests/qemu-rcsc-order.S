/* A single translated STLR/LDAR pair, followed by semihosted exit.
 * MTTCG must preserve RCsc StoreLoad ordering even without a second writer.
 * The checker inspects generated TCG, not a probabilistic race outcome.
 */
.section .text
.global _start
_start:
    adr x0, publication
    adr x1, observation
    mov x2, #1
    stlr x2, [x0]
    ldar x3, [x1]
    mov x0, #0x20
    adr x1, exit_arguments
    hlt #0xf000
    b .

.balign 8
publication:
    .quad 0
observation:
    .quad 0
exit_arguments:
    .quad 0x20026
    .quad 0

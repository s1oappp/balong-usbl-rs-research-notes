//! start.rs — векторы исключений ARM и sysInit тела загрузчика.
//!
//! ОРИГИНАЛ: sysInit @ 0x30004000 — первая функция тела. Подготавливает CPU
//! (CP15: TTBR, SCTLR, инвалидация кэшей по CLIDR), повторно пишет окно
//! SDRAM в системный блок и уходит в usrInit (boot_01.c / body.s
//! @0x30004000..0x30004148). Векторы исключений VxWorks ставит позже
//! (excVecInit/intVecBaseSet в sysStart).
//!
//! ЗДЕСЬ: первые 0x40 байт тела — стандартная таблица векторов ARM (точка
//! входа тела = вектор reset — контракт bootrom «вход тела = 0x30004000»
//! сохранён, VBAR указывает сюда). reset_handler (= sysInit) повторяет
//! существенные операции оригинала: MMU/кэши выключить, кэши и TLB
//! проинвалидировать (канонический проход по CLIDR/CCSIDR), FCSE/ContextID
//! в ноль, окно SDRAM подтвердить, .bss обнулить, стек поставить — и уйти
//! в rust_main (эквивалент usrInit → usrRoot → usrAppInit).

core::arch::global_asm! {
    // ================= Таблица векторов @ 0x30004000 =================
    // Классический ARM-паттерн ldr pc,[pc,#0x18] + таблица указателей.
    // Вектор 0 (reset) — точка входа тела по контракту bootrom.
    ".section .vectors, \"ax\"",
    ".align 5",
    ".global vectors",
    "vectors:",
    "    ldr pc, vec_reset",      // 0x00 reset      — вход тела (=sysInit)
    "    ldr pc, vec_undef",      // 0x04 undefined
    "    ldr pc, vec_swi",        // 0x08 software interrupt
    "    ldr pc, vec_pabt",       // 0x0C prefetch abort
    "    ldr pc, vec_dabt",       // 0x10 data abort
    "    ldr pc, vec_rsvd",       // 0x14 reserved
    "    ldr pc, vec_irq",        // 0x18 IRQ
    "    ldr pc, vec_fiq",        // 0x1C FIQ
    "vec_reset:  .word reset_handler",
    "vec_undef:  .word undef_handler",
    "vec_swi:    .word swi_handler",
    "vec_pabt:   .word pabt_handler",
    "vec_dabt:   .word dabt_handler",
    "vec_rsvd:   .word reset_handler",
    "vec_irq:    .word irq_handler",
    "vec_fiq:    .word fiq_handler",

    // ================= sysInit (reset_handler) =================
    ".section .text.entry, \"ax\"",
    ".align 2",
    ".global reset_handler",
    ".type reset_handler, %function",
    "reset_handler:",

    // --- CP15: как оригинальный sysInit (SCTLR/TTBR из DAT_30004158) ---
    "    mrc p15, #0, r0, c1, c0, #0",       // SCTLR
    "    bic r0, r0, #0x00000003",           // снять M (MMU) и A
    "    bic r0, r0, #0x00001000",           // снять I-кэш
    "    bic r0, r0, #0x00000004",           // снять D-кэш
    "    bic r0, r0, #0x00000800",           // снять branch predict
    "    bic r0, r0, #0x00000040",           // снять FE (SWP)
    "    mcr p15, #0, r0, c1, c0, #0",       // SCTLR := безопасное значение
    "    isb",

    // TTBR := 0 (оригинал: TTBR &= ~0xFF; здесь обнуляем для надёжности)
    "    mov r0, #0",
    "    mcr p15, #0, r0, c2, c0, #0",

    // Инвалидация TLB, prefetch, I-кэша (как sysInit + sysHwInit0)
    "    mcr p15, #0, r0, c8, c7, #0",       // TLBIALL
    "    mcr p15, #0, r0, c7, c5, #4",       // invalidate prefetch buffer
    "    mcr p15, #0, r0, c7, c5, #0",       // I-cache: invalidate all
    "    dsb",
    "    isb",

    // --- D-кэш: clean+invalidate по CLIDR/CCSIDR (канонический проход
    //     ARMv7-A, как в оригинальном sysInit @0x30004024..0x3000408C) ---
    "    mrc p15, #1, r0, c0, c0, #1",       // CLIDR
    "    ands r3, r0, #0x07000000",          // поле LoC
    "    mov r3, r3, lsr #23",               // уровней кэша данных
    "    beq .L_cache_done",                 // LoC == 0 — кэшей нет
    "    mov r10, #0",                       // уровень (кодировка <<1)
    ".L_level_loop:",
    "    add r2, r10, r10, lsr #1",          // shift = уровень*3
    "    mov r1, r0, lsr r2",                // поле ctype уровня
    "    and r1, r1, #7",
    "    cmp r1, #2",                        // 0=нет, 1=только I, 2+=D/unified
    "    blt .L_level_next",
    "    mcr p15, #2, r10, c0, c0, #0",      // CSSR := уровень
    "    isb",
    "    mrc p15, #1, r1, c0, c0, #1",       // CCSIDR
    "    and r2, r1, #7",                    // log2(слов в строке)/-3
    "    add r2, r2, #4",                    // set_shift = log2(байт строки)
    "    mov r4, #0x3FF",
    "    and r4, r4, r1, lsr #3",            // assoc-1
    "    clz r5, r4",                        // way_shift
    "    mov r7, #1",
    "    lsl r7, r7, r5",                    // way_step = 1<<way_shift
    "    lsl r11, r4, r5",                   // макс. way в позиции
    "    orr r11, r10, r11",                 // r11 = уровень | way<<way_shift
    "    movw r6, #0x7FFF",
    "    and r6, r6, r1, lsr #13",           // sets-1
    ".L_way_loop:",
    "    mov r9, r6",                        // текущий set = sets-1
    ".L_set_loop:",
    "    orr r0, r11, r9, lsl r2",           // уровень|way|set<<set_shift
    "    mcr p15, 0, r0, c7, c14, 2",         // DCCISW: clean+invalidate
    "    subs r9, r9, #1",
    "    bge .L_set_loop",
    "    subs r11, r11, r7",                 // следующая way
    "    bge .L_way_loop",
    ".L_level_next:",
    "    add r10, r10, #2",                  // следующий уровень
    "    cmp r3, r10",
    "    bgt .L_level_loop",
    ".L_cache_done:",
    "    dsb",

    // FCSE PID / Context ID := 0 (как оригинал: coproc_moveto_FCSE_PID(0),
    // coproc_moveto_Context_ID(0))
    "    mov r0, #0",
    "    mcr p15, #0, r0, c13, c0, #0",      // FCSEPID
    "    mcr p15, #0, r0, c13, c0, #1",      // CONTEXTIDR

    // VBAR := 0x30004000 (эквивалент intVecBaseSet/excVecInit оригинала)
    "    ldr r0, =0x30004000",
    "    mcr p15, #0, r0, c12, c0, #0",      // VBAR
    "    isb",

    // --- Окно SDRAM в системном блоке: как оригинальный sysInit ---
    // boot_01.c: *(DAT_30004164+0x470)=0x30000000; +0x474=0x3FF00000; +0x400|=1
    "    ldr r1, =0x90000000",
    "    ldr r2, =0x30000000",
    "    str r2, [r1, #0x470]",
    "    ldr r2, =0x3FF00000",
    "    str r2, [r1, #0x474]",
    "    ldr r2, [r1, #0x400]",
    "    orr r2, r2, #1",
    "    str r2, [r1, #0x400]",

    // --- Обнуление .bss (как sysStart → bzero в оригинале) ---
    "    ldr r0, =__bss_start",
    "    ldr r1, =__bss_end",
    "    mov r2, #0",
    ".L_bss_loop:",
    "    cmp r0, r1",
    "    strlo r2, [r0], #4",
    "    blo .L_bss_loop",

    // --- Стек главного цикла ---
    "    ldr sp, =__stack_top",

    // --- Переход в rust_main (ARM-режим; эквивалент usrInit) ---
    "    bl rust_main",

    // rust_main не возвращается; на всякий случай — сон до события
    ".L_sleep:",
    "    wfe",
    "    b .L_sleep",
    ".size reset_handler, . - reset_handler",

    // ================= Заглушки исключений =================
    // (оригинал разворачивает полноценные VxWorks-обработчики excVecInit;
    // для загрузчика достаточно безопасного зависания)
    ".section .text",
    ".align 2",
    ".global undef_handler",
    "undef_handler:",
    "    b undef_handler",
    ".global swi_handler",
    "swi_handler:",
    "    b swi_handler",
    ".global pabt_handler",
    "pabt_handler:",
    "    b pabt_handler",
    ".global dabt_handler",
    "dabt_handler:",
    "    b dabt_handler",
    ".global fiq_handler",
    "fiq_handler:",
    "    b fiq_handler",

    // ================= IRQ: сохранение контекста и диспетчер =================
    // VIC @0x08000000: IAR @+0x10C, EOI @+0x110 (FUN_30007f84/FUN_3000805c)
    ".global irq_handler",
    "irq_handler:",
    "    sub lr, lr, #4",                     // скорректировать возврат
    "    push {{lr}}",
    "    push {{r0, r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11, r12}}",
    "    mrs r0, spsr",
    "    push {{r0}}",
    "    mov fp, sp",
    "    bl irq_dispatch",                    // Rust-диспетчер (VIC)
    "    pop {{r0}}",
    "    msr spsr_cxsf, r0",
    "    pop {{r0, r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11, r12}}",
    "    pop {{lr}}",
    "    subs pc, lr, #0",                    // возврат с восстановлением CPSR
}
